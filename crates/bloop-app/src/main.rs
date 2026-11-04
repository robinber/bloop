//! Bloop executable: reads the settings, builds the components, and starts the run loop.

#![forbid(unsafe_code)]

mod driver;
mod logging;
mod worker;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use bloop_audio::capture::Microphone;
use bloop_core::domain::LevelMeter;
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
use bloop_providers::mistral::{self, TranscribeConfig};
use tokio::runtime::Builder;

use crate::driver::Driver;
use crate::worker::{Limits, Spawner, Speech};

const HOLD: Duration = Duration::from_millis(220);
const MODEL: &str = "voxtral-mini-transcribe-realtime-3-0";
const STREAMING_DELAY_MS: u32 = 480;
const LANGUAGE: &str = "fr";
const VOCABULARY: &[&str] = &[];

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
    let tap = KeyTap::install(mtm);
    if tap.is_none() {
        tracing::error!(
            "macOS refused the event tap, enable Input Monitoring and Accessibility, then open Bloop again"
        );
    }
    let key = Keychain.get(mistral::ACCOUNT);
    let _menu = StatusMenu::new(mtm, &missing, tap.is_some(), key.as_ref().err().copied());
    let speech = match key {
        Ok(key) => Some(Arc::new(Speech {
            transcriber: Box::new(mistral::transcribe::Transcriber::new(
                key,
                TranscribeConfig {
                    model: MODEL.to_owned(),
                    target_streaming_delay_ms: STREAMING_DELAY_MS,
                },
            )),
            language: LANGUAGE.to_owned(),
            vocabulary: VOCABULARY.iter().copied().map(str::to_owned).collect(),
            limits: Limits::default(),
        })),
        Err(error) => {
            tracing::warn!(?error, "the API key is not available");
            None
        }
    };
    let _sleep = SleepObserver::new(mtm);

    let sink: Arc<dyn EventSink> = Arc::new(MainSink);
    let spawner = Spawner {
        runtime: runtime.handle().clone(),
        speech,
        events: Arc::clone(&sink),
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
    events::set_handler(mtm, move |event| {
        driver.handle(event);
        if let Some(tap) = &tap {
            tap.set_capture_active(driver.capture_active());
        }
    });

    app.run();
    ExitCode::SUCCESS
}
