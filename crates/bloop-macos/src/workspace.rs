//! Opens a file or a folder through `NSWorkspace`.
//!
//! The log gives no path, because a path contains the name of the user.

use std::path::Path;

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

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
