//! Bloop executable: reads the settings, builds the components, and starts the run loop.

#![forbid(unsafe_code)]

mod dictionary;
mod driver;
mod files;
mod hub;
mod logging;
mod menu;
mod prompts;
mod settings;
mod worker;

use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bloop_audio::capture::Microphone;
use bloop_core::domain::LevelMeter;
use bloop_core::ports::platform::EventSink;
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
use tokio::runtime::Builder;

use crate::driver::Driver;
use crate::hub::{Hub, HubSnapshots};
use crate::menu::{AppMenu, Facts};
use crate::settings::Paths;
use crate::worker::Spawner;

/// The time limit of each correction request, including its response body.
const CORRECTION_LIMIT: Duration = Duration::from_secs(8);

fn main() -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("bloop: main does not run on the main thread");
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
    let Some(paths) = Paths::in_home() else {
        tracing::error!("the home folder is unknown");
        return ExitCode::FAILURE;
    };
    let hub = Rc::new(Hub::start(paths.clone(), &Keychain, CORRECTION_LIMIT));
    let _sleep = SleepObserver::new(mtm);

    let sink: Arc<dyn EventSink> = Arc::new(MainSink);
    let spawner = Spawner {
        runtime: runtime.handle().clone(),
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
        HubSnapshots::new(Rc::clone(&hub)),
    );
    // Install the tap after the slow setup: macOS disables a tap that gets events before the run loop runs.
    let tap = KeyTap::install(mtm);
    if tap.is_none() {
        tracing::error!(
            "macOS refused the event tap, enable Input Monitoring and Accessibility, then open Bloop again"
        );
    }
    let facts = Facts {
        missing,
        tap: tap.is_some(),
        key: hub.key_error(),
    };
    let _menu = StatusMenu::new(mtm, Box::new(AppMenu::new(Rc::clone(&hub), facts, paths)));
    events::set_handler(mtm, move |event| {
        hub.note(&event);
        driver.handle(event);
        if let Some(tap) = &tap {
            tap.set_capture_active(driver.capture_active());
        }
    });

    app.run();
    events::clear_handler(mtm);
    ExitCode::SUCCESS
}
