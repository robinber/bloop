//! The settings hub: the settings files, the objects built from them, and the settings snapshot of the next generation.
//!
//! At each dictation key press and each time the status menu opens, the hub reads `config.toml`,
//! `dictionary.toml`, and one prompt when the correction is on. It compares each text with its last text.
//! At a key press, the prompt is the prompt whose list contains the frontmost application, else the selected prompt.
//! The lists are the `[refine.apps]` table of the file, or the default lists that `main` computes at start.
//! When the menu opens, it is the selected prompt.
//! It builds a new snapshot only after a change, and it builds the chat model again only when `[chat]` changes.
//! It reads the API keys one time at start, because a Keychain read can wait for a dialog.
//! It keeps the refused key state until the next start.
//!
//! The hub runs on the main thread only. A hub borrow contains only file I/O, parsing, object construction,
//! and `tracing` calls: no `AppKit` call, no `Scheduler::after`, no `MainSink::send`, and nothing that runs the run loop.
//! Each method returns its `Rc` after the borrow ends. Thus a failed borrow is a bug: the hub logs it and never panics.
//! The log contains file names, error kinds, lines, and nonsecret values. It never contains file content or prompt text.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bloop_core::error::{AppError, ProviderError, SecretError};
use bloop_core::ports::ai::ChatModel;
use bloop_core::ports::platform::SecretStore;
use bloop_core::session::Event;
use bloop_providers::{ChatConfig, build_chat_model, build_transcriber};
use secrecy::SecretString;

use crate::dictionary;
use crate::driver::Snapshots;
use crate::files::{self, FileError};
use crate::prompts::{self, AppLists, Prompt, PromptError};
use crate::settings::{self, Change, HoldMs, Paths, PromptName, Settings};
use crate::worker::{Correction, Limits, NoCorrection, Speech};

/// The settings of one generation: the hold time and the speech objects.
pub(crate) struct Snapshot {
    pub(crate) hold: Duration,
    /// The speech objects, or the error of each capture when speech is not available.
    pub(crate) speech: Result<Arc<Speech>, AppError>,
}

/// The state of the settings files for the status menu. It contains no file content.
pub(crate) struct View {
    /// The values of valid settings, or why `config.toml` is invalid.
    pub(crate) settings: Result<Summary, settings::Invalid>,
    /// The selected prompt and why it is not available. It is `None` when the prompt is valid or the correction is off.
    pub(crate) prompt: Option<(PromptName, PromptError)>,
    /// Each other prompt that a non-empty application list routes to, and why it is not available.
    /// It is empty when these prompts are valid or the correction is off.
    pub(crate) routed: Vec<(PromptName, PromptError)>,
    /// Why `dictionary.toml` is invalid. It is `None` when the dictionary is valid.
    pub(crate) dictionary: Option<dictionary::Invalid>,
    /// `true` after the provider refused the API key that `Hub::start` read.
    pub(crate) key_refused: bool,
    /// The built-in prompt names and the prompt file names, sorted and without duplicates.
    pub(crate) prompts: Vec<PromptName>,
    /// The applications of each prompt. It is empty when the settings are invalid.
    pub(crate) lists: AppLists,
}

/// The values of valid settings that the status menu shows.
pub(crate) struct Summary {
    /// The state of "Correct text".
    pub(crate) correction: bool,
    /// The selected prompt.
    pub(crate) prompt: PromptName,
    /// The correction model.
    pub(crate) model: String,
    /// The correction models to select from.
    pub(crate) models: Vec<String>,
    /// The hold time of the dictation key.
    pub(crate) hold: HoldMs,
}

/// Owns the settings state on the main thread. All its methods take `&self`, so that the owners share it as `Rc<Hub>`.
pub(crate) struct Hub {
    paths: Paths,
    /// The API keys of the start settings. They do not change until the next start.
    keys: Keys,
    /// The time limit of each correction request.
    limit: Duration,
    /// The application lists while `config.toml` has no `[refine.apps]` table.
    defaults: AppLists,
    /// `true` after the provider refused the key of the transcription. It stays until the next start.
    key_refused: Cell<bool>,
    inner: RefCell<Inner>,
}

struct Inner {
    /// The last read of `config.toml`, compared at each refresh.
    config: Result<Option<String>, FileError>,
    /// The last read of `dictionary.toml`, compared at each refresh.
    dictionary: Result<Option<String>, FileError>,
    state: State,
    /// The chat model and its settings. The hub uses it again while `[chat]` does not change.
    chat: Option<(ChatConfig, Arc<dyn ChatModel>)>,
    snapshot: Rc<Snapshot>,
}

/// The values of the files, or the error of each file.
struct State {
    /// The settings of `config`. The hub parses them again only when `config` changes.
    settings: Result<Settings, settings::Invalid>,
    /// The written forms of `dictionary`. The hub parses them again only when `dictionary` changes.
    vocabulary: Result<Vec<String>, dictionary::Invalid>,
    /// The prompt of the last refresh: its name, and the prompt or why it is not available.
    /// It is `None` when the settings are invalid or the correction is off: then the hub reads no prompt.
    prompt: Option<NamedPrompt>,
}

/// A prompt name, and the prompt or why it is not available.
type NamedPrompt = (PromptName, Result<Prompt, PromptError>);

/// How a refresh chose its prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The list of the prompt contains the frontmost application.
    App,
    /// No list contains the frontmost application: the prompt is the selected prompt.
    Default,
}

impl Route {
    fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Default => "default",
        }
    }
}

impl Hub {
    /// Writes the settings template when it is missing, reads the files and the API keys, and builds the first snapshot.
    /// It never fails: the snapshot gives the error of each capture.
    ///
    /// `limit` is the time limit of each correction request. `defaults` are the application lists while `config.toml`
    /// has no `[refine.apps]` table.
    pub(crate) fn start(
        paths: Paths,
        secrets: &impl SecretStore,
        limit: Duration,
        defaults: AppLists,
    ) -> Self {
        settings::prepare(&paths);
        let config = files::read_bounded(&paths.config, settings::LIMIT);
        let dictionary = files::read_bounded(&paths.dictionary, dictionary::LIMIT);
        let settings = parse_settings(&config);
        let keys = match &settings {
            Ok(settings) => Keys::read(settings, secrets),
            Err(_) => Keys::read(&Settings::default(), secrets),
        };
        let state = State {
            prompt: read_prompt(&settings, &paths.prompts, &defaults, None)
                .map(|(_, prompt)| prompt),
            settings,
            vocabulary: parse_dictionary(&dictionary),
        };
        let mut chat = None;
        let snapshot = Rc::new(build(&state, &mut chat, &keys, limit));
        let inner = Inner {
            config,
            dictionary,
            state,
            chat,
            snapshot,
        };
        Self {
            paths,
            keys,
            limit,
            defaults,
            key_refused: Cell::new(false),
            inner: RefCell::new(inner),
        }
    }

    /// Reads the files again and returns the snapshot of a dictation in the application `app`, a bundle identifier.
    /// The prompt is the prompt whose list contains `app`, else the selected prompt.
    /// The snapshot is new only when a text or the prompt changed.
    /// With the correction on, it logs the prompt name, how the hub chose it, and `app`.
    /// Returns `None` only when the hub is already borrowed, which is a bug.
    pub(crate) fn refresh(&self, app: Option<&str>) -> Option<Rc<Snapshot>> {
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            tracing::error!("the settings hub is already in use, the refresh does not occur");
            return None;
        };
        let route = inner.refresh(&self.paths, &self.keys, self.limit, &self.defaults, app);
        if let (Some(route), Some((name, _))) = (route, &inner.state.prompt) {
            tracing::info!(
                prompt = name.as_str(),
                route = route.as_str(),
                app,
                "the dictation prompt is chosen"
            );
        }
        Some(Rc::clone(&inner.snapshot))
    }

    /// Lists the prompt names, then reads the files again and returns their state for the status menu.
    /// Returns `None` only when the hub is already borrowed, which is a bug.
    pub(crate) fn view(&self) -> Option<View> {
        let prompts = prompts::names(&self.paths.prompts);
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            tracing::error!("the settings hub is already in use, the menu shows its last state");
            return None;
        };
        inner.refresh(&self.paths, &self.keys, self.limit, &self.defaults, None);
        let state = &inner.state;
        let prompt = match &state.prompt {
            Some((name, Err(error))) => Some((name.clone(), *error)),
            _ => None,
        };
        let routed = match &state.settings {
            Ok(settings) if settings.refine.enabled => {
                routed_errors(settings, &self.paths.prompts, &self.defaults)
            }
            _ => Vec::new(),
        };
        let lists = state
            .settings
            .as_ref()
            .map(|settings| prompts::app_lists(settings, &self.defaults))
            .unwrap_or_default();
        Some(View {
            settings: match &state.settings {
                Ok(settings) => Ok(Summary {
                    correction: settings.refine.enabled,
                    prompt: settings.refine.prompt.clone(),
                    model: settings.chat.model().to_owned(),
                    models: settings.chat.models().to_vec(),
                    hold: settings.dictation.hold_ms,
                }),
                Err(error) => Err(*error),
            },
            prompt,
            routed,
            dictionary: state.vocabulary.as_ref().err().copied(),
            key_refused: self.key_refused.get(),
            prompts,
            lists,
        })
    }

    /// Marks the API key as refused when `event` reports `Unauthorized` and `start` read the key.
    /// Without a key at start, the status menu shows the missing key instead.
    pub(crate) fn note(&self, event: &Event) {
        if refuses_key(event)
            && self.keys.transcriber_error().is_none()
            && !self.key_refused.replace(true)
        {
            tracing::warn!(
                "the provider refused the API key: add a valid key, then open Bloop again"
            );
        }
    }

    /// Writes `change` to `config.toml`, then refreshes. It never changes an invalid file.
    /// A refused write logs a warning. It never panics, because it runs in an `AppKit` callback.
    pub(crate) fn set(&self, change: &Change) {
        if let Err(error) = settings::write(&self.paths.config, change, &self.defaults) {
            tracing::warn!(?error, "the settings file is not changed");
        }
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            tracing::error!("the settings hub is already in use, the refresh does not occur");
            return;
        };
        inner.refresh(&self.paths, &self.keys, self.limit, &self.defaults, None);
    }

    /// Returns the error of the transcription API key that `start` read, for the status menu.
    pub(crate) fn key_error(&self) -> Option<SecretError> {
        self.keys.transcriber_error()
    }

    /// Returns `true` when the settings are valid and the correction is on, for the status menu.
    pub(crate) fn correction_on(&self) -> bool {
        let Ok(inner) = self.inner.try_borrow() else {
            tracing::error!("the settings hub is already in use, the correction state is off");
            return false;
        };
        inner
            .state
            .settings
            .as_ref()
            .is_ok_and(|settings| settings.refine.enabled)
    }
}

impl Inner {
    /// Reads the files and the prompt of the application `app`, and builds a new snapshot when a text
    /// or the prompt changed. Returns how it chose the prompt, or `None` when it read no prompt.
    fn refresh(
        &mut self,
        paths: &Paths,
        keys: &Keys,
        limit: Duration,
        defaults: &AppLists,
        app: Option<&str>,
    ) -> Option<Route> {
        let config = files::read_bounded(&paths.config, settings::LIMIT);
        let dictionary = files::read_bounded(&paths.dictionary, dictionary::LIMIT);
        let config_changed = config != self.config;
        let dictionary_changed = dictionary != self.dictionary;
        if config_changed {
            self.state.settings = parse_settings(&config);
            self.config = config;
        }
        if dictionary_changed {
            self.state.vocabulary = parse_dictionary(&dictionary);
            self.dictionary = dictionary;
        }
        let (route, prompt) =
            read_prompt(&self.state.settings, &paths.prompts, defaults, app).unzip();
        if config_changed || dictionary_changed || prompt != self.state.prompt {
            self.state.prompt = prompt;
            self.snapshot = Rc::new(build(&self.state, &mut self.chat, keys, limit));
        }
        route
    }
}

/// Returns `true` when `event` reports that the provider refused the API key.
/// Only the transcription sends `Unauthorized` in an event: a failed correction keeps the raw text.
fn refuses_key(event: &Event) -> bool {
    matches!(
        event,
        Event::CaptureFailed(_, AppError::Provider(ProviderError::Unauthorized))
            | Event::WorkFailed(_, AppError::Provider(ProviderError::Unauthorized))
    )
}

/// The API keys that `Hub::start` read: the key of the transcription account,
/// then the key of the chat account when it differs.
struct Keys(Vec<(&'static str, Result<SecretString, SecretError>)>);

impl Keys {
    fn read(settings: &Settings, secrets: &impl SecretStore) -> Self {
        let mut keys = Vec::new();
        for account in [settings.transcriber.account(), settings.chat.account()] {
            if keys.iter().all(|(read, _)| *read != account) {
                keys.push((account, secrets.get(account)));
            }
        }
        Self(keys)
    }

    /// Returns the key of `account`, or `NotFound` when `Hub::start` did not read it.
    fn get(&self, account: &str) -> Result<SecretString, SecretError> {
        self.0
            .iter()
            .find(|(read, _)| *read == account)
            .map_or(Err(SecretError::NotFound), |(_, key)| key.clone())
    }

    fn transcriber_error(&self) -> Option<SecretError> {
        self.0
            .first()
            .and_then(|(_, key)| key.as_ref().err().copied())
    }
}

/// Parses the text of `config.toml`. A missing file gives the default settings.
/// An invalid file logs its error, without its content.
fn parse_settings(text: &Result<Option<String>, FileError>) -> Result<Settings, settings::Invalid> {
    let settings = match text {
        Ok(Some(text)) => settings::parse(text),
        Ok(None) => Ok(Settings::default()),
        Err(error) => Err(settings::Invalid::from(*error)),
    };
    if let Err(error) = &settings {
        tracing::error!(
            file = "config.toml",
            ?error,
            "the settings file is invalid, each dictation fails until the file is fixed"
        );
    }
    settings
}

/// Parses the text of `dictionary.toml` and returns its written forms. A missing file gives an empty dictionary.
/// It logs the number of entries or the error, never the entries.
fn parse_dictionary(
    text: &Result<Option<String>, FileError>,
) -> Result<Vec<String>, dictionary::Invalid> {
    match text {
        Ok(Some(text)) => match dictionary::parse(text) {
            Ok(dictionary) => {
                tracing::info!(?dictionary, "the dictionary is ready");
                Ok(dictionary.written_forms())
            }
            Err(error) => Err(dictionary_error(error)),
        },
        Ok(None) => {
            tracing::info!("the dictionary file does not exist, the dictionary is empty");
            Ok(Vec::new())
        }
        Err(error) => Err(dictionary_error((*error).into())),
    }
}

fn dictionary_error(error: dictionary::Invalid) -> dictionary::Invalid {
    tracing::error!(
        file = "dictionary.toml",
        ?error,
        "the dictionary file is invalid, each dictation fails until the file is fixed"
    );
    error
}

/// Reads the prompt of the application `app` when the settings are valid and the correction is on:
/// the prompt whose list contains `app`, else the selected prompt. Else it returns `None`.
fn read_prompt(
    settings: &Result<Settings, settings::Invalid>,
    folder: &Path,
    defaults: &AppLists,
    app: Option<&str>,
) -> Option<(Route, NamedPrompt)> {
    let settings = settings
        .as_ref()
        .ok()
        .filter(|settings| settings.refine.enabled)?;
    let lists = prompts::app_lists(settings, defaults);
    let (route, name) = match prompts::route(&lists, app) {
        Some(name) => (Route::App, name),
        None => (Route::Default, &settings.refine.prompt),
    };
    Some((route, (name.clone(), prompts::resolve(folder, name))))
}

/// Returns why each prompt that a non-empty list routes to is not available, except the selected prompt.
fn routed_errors(
    settings: &Settings,
    folder: &Path,
    defaults: &AppLists,
) -> Vec<(PromptName, PromptError)> {
    prompts::app_lists(settings, defaults)
        .into_iter()
        .filter(|(name, apps)| !apps.is_empty() && *name != settings.refine.prompt)
        .filter_map(|(name, _)| {
            let error = prompts::resolve(folder, &name).err()?;
            Some((name, error))
        })
        .collect()
}

/// Builds the snapshot of `state`. It uses the chat model of `chat` again when its settings did not change.
/// It logs the nonsecret values of the snapshot.
fn build(
    state: &State,
    chat: &mut Option<(ChatConfig, Arc<dyn ChatModel>)>,
    keys: &Keys,
    limit: Duration,
) -> Snapshot {
    let Ok(settings) = &state.settings else {
        tracing::info!(
            "the new settings snapshot fails each dictation: the settings file is invalid"
        );
        return Snapshot {
            hold: HoldMs::default().duration(),
            speech: Err(AppError::InvalidSettings),
        };
    };
    let speech = match (&state.vocabulary, keys.get(settings.transcriber.account())) {
        (Err(_), _) => Err(AppError::InvalidSettings),
        (Ok(_), Err(error)) => {
            tracing::warn!(
                ?error,
                "the API key is not available, each dictation fails: add the key, then open Bloop again"
            );
            Err(AppError::Provider(ProviderError::Unauthorized))
        }
        (Ok(vocabulary), Ok(key)) => Ok(Arc::new(Speech {
            transcriber: build_transcriber(settings.transcriber.clone(), key),
            correction: correction(settings, state.prompt.as_ref(), chat, keys, limit),
            language: settings.dictation.language.as_str().to_owned(),
            vocabulary: vocabulary.clone(),
            limits: Limits::default(),
        })),
    };
    let prompt = state
        .prompt
        .as_ref()
        .and_then(|(_, prompt)| prompt.as_ref().ok());
    tracing::info!(
        hold_ms = settings.dictation.hold_ms.get(),
        language = settings.dictation.language.as_str(),
        transcriber = ?settings.transcriber,
        chat = ?settings.chat,
        correction = settings.refine.enabled,
        prompt = prompt.map(|prompt| prompt.name.as_str()),
        source = prompt.map(|prompt| tracing::field::debug(prompt.source)),
        error = speech.as_ref().err().map(tracing::field::debug),
        "the new settings snapshot is ready"
    );
    Snapshot {
        hold: settings.dictation.hold_ms.duration(),
        speech,
    }
}

/// Returns the correction of valid `settings` with `prompt`, or why it is not available.
fn correction(
    settings: &Settings,
    prompt: Option<&NamedPrompt>,
    chat: &mut Option<(ChatConfig, Arc<dyn ChatModel>)>,
    keys: &Keys,
    limit: Duration,
) -> Result<Correction, NoCorrection> {
    let prompt = match prompt {
        None => return Err(NoCorrection::Off),
        Some((name, Err(error))) => {
            tracing::error!(
                prompt = name.as_str(),
                ?error,
                "the prompt is not valid, each dictation with this prompt fails until it is fixed"
            );
            return Err(NoCorrection::InvalidPrompt);
        }
        Some((_, Ok(prompt))) => prompt,
    };
    let model = chat_model(&settings.chat, chat, keys, limit)?;
    Ok(Correction::new(model, prompt.clone()))
}

/// Returns the chat model of `config`. It builds the model only when `chat` has no model for `config`.
fn chat_model(
    config: &ChatConfig,
    chat: &mut Option<(ChatConfig, Arc<dyn ChatModel>)>,
    keys: &Keys,
    limit: Duration,
) -> Result<Arc<dyn ChatModel>, NoCorrection> {
    if let Some((built, model)) = chat.as_ref()
        && built == config
    {
        return Ok(Arc::clone(model));
    }
    let key = keys.get(config.account()).map_err(|error| {
        tracing::warn!(?error, "the correction is not available: no API key");
        NoCorrection::ChatUnavailable
    })?;
    let model = build_chat_model(config.clone(), key, limit).map_err(|error| {
        tracing::warn!(
            ?error,
            "the correction is not available: the chat model did not start"
        );
        NoCorrection::ChatUnavailable
    })?;
    *chat = Some((config.clone(), Arc::clone(&model)));
    Ok(model)
}

/// The settings snapshots of the driver: the snapshot of the hub after a refresh for the frontmost application.
pub(crate) struct HubSnapshots {
    hub: Rc<Hub>,
    /// Returns the bundle identifier of the frontmost application. It runs before the hub borrow.
    frontmost: fn() -> Option<String>,
    /// The last snapshot, for a failed refresh. Before the first refresh, it fails each capture with `Internal`.
    last: Rc<Snapshot>,
}

impl HubSnapshots {
    pub(crate) fn new(hub: Rc<Hub>, frontmost: fn() -> Option<String>) -> Self {
        Self {
            hub,
            frontmost,
            last: Rc::new(Snapshot {
                hold: HoldMs::default().duration(),
                speech: Err(AppError::Internal),
            }),
        }
    }
}

impl Snapshots for HubSnapshots {
    fn snapshot(&mut self) -> Rc<Snapshot> {
        let app = (self.frontmost)();
        if let Some(snapshot) = self.hub.refresh(app.as_deref()) {
            self.last = snapshot;
        }
        Rc::clone(&self.last)
    }
}

#[cfg(test)]
mod tests;
