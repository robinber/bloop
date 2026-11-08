//! `NSApplication` setup and run loop start.
//!
//! `Info.plist` sets `LSUIElement`. Thus the application runs in accessory mode, with no Dock icon.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSApplication;

/// The shared `AppKit` application.
#[derive(Debug)]
pub struct App {
    app: Retained<NSApplication>,
}

impl App {
    /// Gets the shared application. Call it before you create other `AppKit` objects.
    #[must_use]
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            app: NSApplication::sharedApplication(mtm),
        }
    }

    /// Starts the run loop. The "Quit Bloop" menu item stops it, so the caller can release its components.
    pub fn run(&self) {
        self.app.run();
    }
}
