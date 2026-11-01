//! Bloop executable: reads the settings, builds the components, and starts the run loop.

#![forbid(unsafe_code)]

mod driver;
mod logging;
mod test_level;

use std::process::ExitCode;
use std::time::Duration;

use bloop_core::domain::LevelMeter;
use bloop_macos::MainThreadMarker;
use bloop_macos::app::App;
use bloop_macos::events;
use bloop_macos::keys::KeyTap;
use bloop_macos::menu::StatusMenu;
use bloop_macos::orb::OrbPanel;
use bloop_macos::permissions;
use bloop_macos::scheduler::MainScheduler;

use crate::driver::Driver;
use crate::test_level::TestLevel;

const HOLD: Duration = Duration::from_millis(220);

fn main() -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        return ExitCode::FAILURE;
    };
    if let Err(error) = logging::init() {
        eprintln!("bloop: {error:#}");
        return ExitCode::FAILURE;
    }
    tracing::info!("Bloop starts");

    let app = App::new(mtm);
    let _menu = StatusMenu::new(mtm);

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

    let level = LevelMeter::default();
    let orb = OrbPanel::new(mtm, level.clone());
    let mut driver = Driver::new(MainScheduler, orb, TestLevel::new(level), HOLD);
    events::set_handler(mtm, move |event| {
        driver.handle(event);
        if let Some(tap) = &tap {
            tap.set_capture_active(driver.capture_active());
        }
    });

    app.run();
    ExitCode::SUCCESS
}
