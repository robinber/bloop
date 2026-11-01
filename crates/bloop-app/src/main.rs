//! Bloop executable: reads the settings, builds the components, and starts the run loop.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use bloop_macos::MainThreadMarker;
use bloop_macos::app::App;
use bloop_macos::menu::StatusMenu;

fn main() -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        return ExitCode::FAILURE;
    };
    let app = App::new(mtm);
    let _menu = StatusMenu::new(mtm);
    app.run();
    ExitCode::SUCCESS
}
