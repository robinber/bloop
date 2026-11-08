//! Opens a file or a folder, reads the frontmost application, the open applications, and the names of applications
//! through `NSWorkspace`, and asks for an application in a file panel.
//!
//! The log gives no path, because a path contains the name of the user.

use std::path::Path;

use objc2::{MainThreadMarker, available};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSModalResponseOK, NSOpenPanel, NSWorkspace,
};
use objc2_foundation::{NSBundle, NSString, NSURL, ns_string};

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

/// Returns the name of the installed application with the bundle identifier `id`: its file name without `.app`.
/// Returns `None` when macOS finds no application with this identifier.
#[must_use]
pub fn app_name(id: &str) -> Option<String> {
    let url = NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(id))?;
    let file = url.lastPathComponent()?.to_string();
    let name = file.strip_suffix(".app").unwrap_or(&file);
    (!name.is_empty()).then(|| name.to_owned())
}

/// Returns the bundle identifier and the name of each open application with the regular activation policy,
/// except Bloop. The name is the localized name of the application, else its bundle identifier.
/// An application without a bundle identifier is left out.
#[must_use]
pub fn running_apps() -> Vec<(String, String)> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| {
            app.activationPolicy() == NSApplicationActivationPolicy::Regular
                && !is_own_process(app.processIdentifier())
        })
        .filter_map(|app| {
            let id = app.bundleIdentifier()?.to_string();
            let name = app
                .localizedName()
                .map_or_else(|| id.clone(), |name| name.to_string());
            Some((id, name))
        })
        .collect()
}

/// Asks for an application in a modal file panel on `/Applications`, and returns its bundle identifier.
///
/// Bloop comes to the front for the panel. After the panel, the application that was in front before comes back.
/// The main run loop runs in the modal mode until the panel closes, so main queue events and the event tap still run.
/// Returns `None` when the user cancels, or when the chosen file is not an application with a bundle identifier.
#[must_use]
pub fn choose_app(mtm: MainThreadMarker) -> Option<String> {
    let previous = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .filter(|app| !is_own_process(app.processIdentifier()));
    activate(mtm);
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setDirectoryURL(Some(&NSURL::fileURLWithPath_isDirectory(
        ns_string!("/Applications"),
        true,
    )));
    let response = panel.runModal();
    if let Some(app) = previous
        && !app.activateWithOptions(NSApplicationActivationOptions::empty())
    {
        tracing::info!("the application in front before the panel did not come back");
    }
    if response != NSModalResponseOK {
        tracing::info!("the application panel closed without a choice");
        return None;
    }
    let url = panel.URL()?;
    if url
        .pathExtension()
        .is_none_or(|extension| extension.to_string() != "app")
    {
        tracing::warn!("the chosen file is not an application, Bloop ignores it");
        return None;
    }
    let id = NSBundle::bundleWithURL(&url).and_then(|bundle| bundle.bundleIdentifier());
    if id.is_none() {
        tracing::warn!("the chosen application has no bundle identifier, Bloop ignores it");
    }
    id.map(|id| id.to_string())
}

/// Makes Bloop the active application, so that its panel comes to the front.
fn activate(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    if available!(macos = 14.0) {
        app.activate();
    } else {
        #[expect(
            deprecated,
            reason = "`activate` needs macOS 14, and the bundle declares no minimum macOS version"
        )]
        app.activateIgnoringOtherApps(true);
    }
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
    use super::{app_name, is_own_process, running_apps};

    #[test]
    fn the_name_of_an_installed_application_is_its_file_name_without_app() {
        assert_eq!(app_name("com.apple.finder").as_deref(), Some("Finder"));
        assert_eq!(app_name("com.example.bloop.absent"), None);
    }

    #[test]
    fn each_open_application_has_a_bundle_identifier_and_a_name() {
        for (id, name) in running_apps() {
            assert_ne!(id, "");
            assert_ne!(name, "", "{id}");
        }
    }

    #[test]
    fn only_the_process_identifier_of_bloop_is_its_own() {
        let own = i32::try_from(std::process::id()).unwrap();

        assert!(is_own_process(own));
        assert!(!is_own_process(own + 1));
        assert!(!is_own_process(-1));
    }
}
