//! Bloop executable: reads the settings, builds the components, and starts the run loop.

#![forbid(unsafe_code)]

mod dictionary;
mod driver;
mod files;
mod logging;
mod prompts;
mod settings;
mod worker;

use std::cell::Cell;
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bloop_audio::capture::Microphone;
use bloop_core::domain::LevelMeter;
use bloop_core::error::{AppError, ProviderError, SecretError};
use bloop_core::ports::platform::{EventSink, SecretStore};
use bloop_macos::MainThreadMarker;
use bloop_macos::app::App;
use bloop_macos::events::{self, MainSink};
use bloop_macos::keychain::Keychain;
use bloop_macos::keys::KeyTap;
use bloop_macos::menu::StatusMenu;
use bloop_macos::orb::OrbPanel;
use bloop_macos::paste::Paster;
use bloop_macos::permissions;
use bloop_macos::scheduler::MainScheduler;
use bloop_macos::sleep::SleepObserver;
use bloop_providers::{ChatConfig, build_chat_model, build_transcriber};
use secrecy::SecretString;
use tokio::runtime::Builder;

use crate::driver::Driver;
use crate::settings::PromptName;
use crate::worker::{Correction, Limits, NoCorrection, Spawner, Speech};

/// The time limit of each correction request: an initial value, not measured.
const CORRECTION_LIMIT: Duration = Duration::from_secs(8);

fn main() -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        return ExitCode::FAILURE;
    };
    if let Err(error) = logging::init() {
        eprintln!("bloop: {error:#}");
        return ExitCode::FAILURE;
    }
    tracing::info!("Bloop starts");
    let runtime = match Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .thread_name("bloop-worker")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "the worker runtime did not start");
            return ExitCode::FAILURE;
        }
    };

    let app = App::new(mtm);

    let missing = permissions::request_missing();
    if !missing.is_empty() {
        tracing::warn!(?missing, "permissions are missing");
    }
    let (settings, folder) = settings::load();
    let vocabulary = dictionary::load().map(|dictionary| dictionary.written_forms());
    let hold = settings.dictation.hold_ms.duration();
    let correct_on = folder.is_ok() && settings.refine.enabled;
    let key = Keychain.get(settings.transcriber.account());
    let chat_key = if settings.chat.account() == settings.transcriber.account() {
        key.clone()
    } else {
        Keychain.get(settings.chat.account())
    };
    let key_error = key.as_ref().err().copied();
    let prompt = &settings.refine.prompt;
    let speech = match (key, folder, vocabulary) {
        (Ok(key), Ok(folder), Ok(vocabulary)) => Ok(Arc::new(Speech {
            transcriber: build_transcriber(settings.transcriber, key),
            correction: correction(settings.chat, chat_key, &folder.prompts, prompt),
            language: settings.dictation.language.as_str().to_owned(),
            vocabulary,
            limits: Limits::default(),
        })),
        (Err(error), _, _) => {
            tracing::warn!(?error, "the API key is not available");
            Err(AppError::Provider(ProviderError::Unauthorized))
        }
        (Ok(_), Err(error), _) | (Ok(_), Ok(_), Err(error)) => Err(error),
    };
    let _sleep = SleepObserver::new(mtm);

    let sink: Arc<dyn EventSink> = Arc::new(MainSink);
    let correction_on = Rc::new(Cell::new(correct_on));
    let spawner = Spawner {
        runtime: runtime.handle().clone(),
        speech,
        events: Arc::clone(&sink),
        correction_on: Rc::clone(&correction_on),
    };
    let level = LevelMeter::default();
    let orb = OrbPanel::new(mtm, level.clone());
    let microphone = Microphone::new(level, sink, permissions::microphone_allowed);
    let mut driver = Driver::new(
        MainScheduler,
        orb,
        Paster::new(mtm),
        microphone,
        spawner,
        hold,
    );
    // Install the tap after the slow setup: macOS disables a tap that gets events before the run loop runs.
    let tap = KeyTap::install(mtm);
    if tap.is_none() {
        tracing::error!(
            "macOS refused the event tap, enable Input Monitoring and Accessibility, then open Bloop again"
        );
    }
    let _menu = StatusMenu::new(
        mtm,
        &missing,
        tap.is_some(),
        key_error,
        correct_on,
        Box::new(move |on| {
            correction_on.set(on);
            tracing::info!(on, "the correction toggle changed");
            settings::store(on);
        }),
    );
    events::set_handler(mtm, move |event| {
        driver.handle(event);
        if let Some(tap) = &tap {
            tap.set_capture_active(driver.capture_active());
        }
    });

    app.run();
    ExitCode::SUCCESS
}

/// Builds the correction steps with the prompt `name` of the prompts folder `folder`, or the built-in prompt.
/// Returns why the correction is not available, and logs it, when the prompt is not valid or the chat model is not available.
fn correction(
    config: ChatConfig,
    key: Result<SecretString, SecretError>,
    folder: &Path,
    name: &PromptName,
) -> Result<Correction, NoCorrection> {
    let prompt = match prompts::resolve(folder, name) {
        Ok(prompt) => prompt,
        Err(error) => {
            tracing::error!(
                prompt = name.as_str(),
                ?error,
                "the prompt is not valid, each dictation with the correction on fails until it is fixed and Bloop starts again"
            );
            return Err(NoCorrection::InvalidPrompt);
        }
    };
    let key = match key {
        Ok(key) => key,
        Err(error) => {
            tracing::warn!(?error, "the correction is not available: no API key");
            return Err(NoCorrection::ChatUnavailable);
        }
    };
    match build_chat_model(config, key, CORRECTION_LIMIT) {
        Ok(model) => {
            tracing::info!(
                prompt = prompt.name,
                source = ?prompt.source,
                "the correction is available"
            );
            Ok(Correction::new(model, prompt))
        }
        Err(error) => {
            tracing::warn!(
                ?error,
                "the correction is not available: the chat model did not start"
            );
            Err(NoCorrection::ChatUnavailable)
        }
    }
}
