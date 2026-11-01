//! Accessibility and Input Monitoring permission checks, and the System Settings links.
//!
//! The active event tap needs both permissions. Accessibility also lets Bloop post the paste
//! command. macOS can keep the old result until Bloop starts again.

use bloop_core::error::Permission;
use objc2_app_kit::NSWorkspace;
use objc2_core_graphics::{
    CGPreflightListenEventAccess, CGPreflightPostEventAccess, CGRequestListenEventAccess,
    CGRequestPostEventAccess,
};
use objc2_foundation::{NSString, NSURL};

/// Asks macOS for each missing permission and returns the permissions that are still missing.
///
/// macOS shows its request one time for each permission. After that, the user must enable
/// Bloop in System Settings.
#[must_use]
pub fn request_missing() -> Vec<Permission> {
    let mut missing = Vec::new();
    if !CGPreflightListenEventAccess() && !CGRequestListenEventAccess() {
        missing.push(Permission::InputMonitoring);
    }
    if !CGPreflightPostEventAccess() && !CGRequestPostEventAccess() {
        missing.push(Permission::Accessibility);
    }
    missing
}

/// Opens the System Settings pane where the user enables `permission` for Bloop.
pub(crate) fn open_settings(permission: Permission) {
    let pane = match permission {
        Permission::Microphone => "Privacy_Microphone",
        Permission::Accessibility => "Privacy_Accessibility",
        Permission::InputMonitoring => "Privacy_ListenEvent",
    };
    let address = NSString::from_str(&format!(
        "x-apple.systempreferences:com.apple.preference.security?{pane}"
    ));
    let opened = NSURL::URLWithString(&address)
        .is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
    if !opened {
        tracing::error!(?permission, "System Settings did not open");
    }
}
