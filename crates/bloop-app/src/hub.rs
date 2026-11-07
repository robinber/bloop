//! The settings hub: the settings files, the objects built from them, and the settings snapshot of the next generation.
//!
//! At each dictation key press and each time the status menu opens, the hub reads `config.toml`,
//! `dictionary.toml`, and the selected prompt when the correction is on. It compares each text with its last text.
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
use crate::prompts::{self, Prompt, PromptError};
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
    /// Why `dictionary.toml` is invalid. It is `None` when the dictionary is valid.
    pub(crate) dictionary: Option<dictionary::Invalid>,
    /// `true` after the provider refused the API key that `Hub::start` read.
    pub(crate) key_refused: bool,
    /// The built-in prompt names and the prompt file names, sorted and without duplicates.
    pub(crate) prompts: Vec<PromptName>,
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
    /// The selected prompt. It is `None` when the settings are invalid or the correction is off:
    /// then the hub does not read it.
    prompt: Option<Result<Prompt, PromptError>>,
}

impl Hub {
    /// Writes the settings template when it is missing, reads the files and the API keys, and builds the first snapshot.
    /// It never fails: the snapshot gives the error of each capture.
    ///
    /// `limit` is the time limit of each correction request.
    pub(crate) fn start(paths: Paths, secrets: &impl SecretStore, limit: Duration) -> Self {
        settings::prepare(&paths);
        let config = files::read_bounded(&paths.config, settings::LIMIT);
        let dictionary = files::read_bounded(&paths.dictionary, dictionary::LIMIT);
        let settings = parse_settings(&config);
        let keys = match &settings {
            Ok(settings) => Keys::read(settings, secrets),
            Err(_) => Keys::read(&Settings::default(), secrets),
        };
        let state = State {
            prompt: read_prompt(&settings, &paths.prompts),
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
            key_refused: Cell::new(false),
            inner: RefCell::new(inner),
        }
    }

    /// Reads the files again and returns the current snapshot. The snapshot is new only when a text changed.
    /// Returns `None` only when the hub is already borrowed, which is a bug.
    pub(crate) fn refresh(&self) -> Option<Rc<Snapshot>> {
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            tracing::error!("the settings hub is already in use, the refresh does not occur");
            return None;
        };
        inner.refresh(&self.paths, &self.keys, self.limit);
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
        inner.refresh(&self.paths, &self.keys, self.limit);
        let state = &inner.state;
        let prompt = match (&state.settings, &state.prompt) {
            (Ok(settings), Some(Err(error))) => Some((settings.refine.prompt.clone(), *error)),
            _ => None,
        };
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
            dictionary: state.vocabulary.as_ref().err().copied(),
            key_refused: self.key_refused.get(),
            prompts,
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
        if let Err(error) = settings::write(&self.paths.config, change) {
            tracing::warn!(?error, "the settings file is not changed");
        }
        self.refresh();
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
    /// Reads the files and builds a new snapshot when a text changed.
    fn refresh(&mut self, paths: &Paths, keys: &Keys, limit: Duration) {
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
        let prompt = read_prompt(&self.state.settings, &paths.prompts);
        if !config_changed && !dictionary_changed && prompt == self.state.prompt {
            return;
        }
        self.state.prompt = prompt;
        self.snapshot = Rc::new(build(&self.state, &mut self.chat, keys, limit));
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

/// Reads the selected prompt when the settings are valid and the correction is on. Else it returns `None`.
fn read_prompt(
    settings: &Result<Settings, settings::Invalid>,
    folder: &Path,
) -> Option<Result<Prompt, PromptError>> {
    let settings = settings
        .as_ref()
        .ok()
        .filter(|settings| settings.refine.enabled)?;
    Some(prompts::resolve(folder, &settings.refine.prompt))
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
        .and_then(|prompt| prompt.as_ref().ok());
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
    prompt: Option<&Result<Prompt, PromptError>>,
    chat: &mut Option<(ChatConfig, Arc<dyn ChatModel>)>,
    keys: &Keys,
    limit: Duration,
) -> Result<Correction, NoCorrection> {
    let prompt = match prompt {
        None => return Err(NoCorrection::Off),
        Some(Err(error)) => {
            tracing::error!(
                prompt = settings.refine.prompt.as_str(),
                ?error,
                "the prompt is not valid, each dictation with the correction on fails until it is fixed"
            );
            return Err(NoCorrection::InvalidPrompt);
        }
        Some(Ok(prompt)) => prompt,
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

/// The settings snapshots of the driver: the snapshot of the hub after a refresh.
pub(crate) struct HubSnapshots {
    hub: Rc<Hub>,
    /// The last snapshot, for a failed refresh. Before the first refresh, it fails each capture with `Internal`.
    last: Rc<Snapshot>,
}

impl HubSnapshots {
    pub(crate) fn new(hub: Rc<Hub>) -> Self {
        Self {
            hub,
            last: Rc::new(Snapshot {
                hold: HoldMs::default().duration(),
                speech: Err(AppError::Internal),
            }),
        }
    }
}

impl Snapshots for HubSnapshots {
    fn snapshot(&mut self) -> Rc<Snapshot> {
        if let Some(snapshot) = self.hub.refresh() {
            self.last = snapshot;
        }
        Rc::clone(&self.last)
    }
}

#[cfg(test)]
mod tests;
