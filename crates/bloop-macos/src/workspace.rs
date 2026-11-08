//! Opens a file or a folder, and reads the frontmost application, through `NSWorkspace`.
//!
//! The log gives no path, because a path contains the name of the user.

use std::path::Path;

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

/// Returns the bundle identifier of the frontmost application.
/// Returns `None` when no application is in front, when it has no bundle identifier, or when it is Bloop.
#[must_use]
pub fn frontmost_app() -> Option<String> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    if is_own_process(app.processIdentifier()) {
        return None;
    }
    app.bundleIdentifier().map(|id| id.to_string())
}

/// Returns `true` when `pid` is the process identifier of Bloop.
fn is_own_process(pid: i32) -> bool {
    u32::try_from(pid).is_ok_and(|pid| pid == std::process::id())
}

/// Opens the file at `path` in the application for its type.
/// Without such an application, it opens the folder of the file in Finder.
pub fn open_file(path: &Path) {
    let Some(text) = path.to_str() else {
        tracing::error!("the file path is not UTF-8, Bloop does not open it");
        return;
    };
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(text), false);
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        return;
    }
    let Some(folder) = path.parent() else {
        tracing::error!("no application opened the file, and the file has no folder");
        return;
    };
    tracing::info!("no application opened the file, Bloop opens its folder");
    open_folder(folder);
}

/// Opens the folder at `path` in Finder.
pub fn open_folder(path: &Path) {
    let Some(text) = path.to_str() else {
        tracing::error!("the folder path is not UTF-8, Bloop does not open it");
        return;
    };
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(text), true);
    if !NSWorkspace::sharedWorkspace().openURL(&url) {
        tracing::error!("Finder did not open the folder");
    }
}

#[cfg(test)]
mod tests {
    use super::is_own_process;

    #[test]
    fn only_the_process_identifier_of_bloop_is_its_own() {
        let own = i32::try_from(std::process::id()).unwrap();

        assert!(is_own_process(own));
        assert!(!is_own_process(own + 1));
        assert!(!is_own_process(-1));
    }
}
