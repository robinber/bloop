//! Bloop executable: reads the settings, builds the components, and starts the run loop.

#![forbid(unsafe_code)]

mod dictionary;
mod driver;
mod logging;
mod prompts;
mod settings;
mod worker;

use std::cell::Cell;
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
use bloop_providers::{ChatConfig, TranscriberConfig, build_chat_model, build_transcriber};
use secrecy::SecretString;
use tokio::runtime::Builder;

use crate::driver::Driver;
use crate::worker::{Correction, Limits, Spawner, Speech};

const HOLD: Duration = Duration::from_millis(220);
const LANGUAGE: &str = "fr";
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
    let saved_correction = settings::load();
    let vocabulary = dictionary::load().map(|dictionary| dictionary.written_forms());
    let transcriber = TranscriberConfig::default();
    let chat = ChatConfig::default();
    let key = Keychain.get(transcriber.account());
    let chat_key = if chat.account() == transcriber.account() {
        key.clone()
    } else {
        Keychain.get(chat.account())
    };
    let key_error = key.as_ref().err().copied();
    let speech = match (key, saved_correction, vocabulary) {
        (Ok(key), Ok(_), Ok(vocabulary)) => Ok(Arc::new(Speech {
            transcriber: build_transcriber(transcriber, key),
            correction: correction(chat, chat_key),
            language: LANGUAGE.to_owned(),
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
    let correct_on = saved_correction.unwrap_or(false);
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
        HOLD,
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

/// Builds the correction steps with the default prompt.
/// Returns `None`, and logs why, when the chat model is not available.
fn correction(config: ChatConfig, key: Result<SecretString, SecretError>) -> Option<Correction> {
    let key = match key {
        Ok(key) => key,
        Err(error) => {
            tracing::warn!(?error, "the correction is not available: no API key");
            return None;
        }
    };
    match build_chat_model(config, key, CORRECTION_LIMIT) {
        Ok(model) => {
            tracing::info!(
                prompt = prompts::NATURAL_LIGHT.name,
                "the correction is available"
            );
            Some(Correction::new(model, &prompts::NATURAL_LIGHT))
        }
        Err(error) => {
            tracing::warn!(
                ?error,
                "the correction is not available: the chat model did not start"
            );
            None
        }
    }
}
