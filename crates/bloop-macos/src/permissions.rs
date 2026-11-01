//! Accessibility and Input Monitoring permission checks.
//!
//! The active event tap needs both permissions. Accessibility also lets Bloop post the paste
//! command. macOS can keep the old result until Bloop starts again.

use bloop_core::error::Permission;
use objc2_core_graphics::{
    CGPreflightListenEventAccess, CGPreflightPostEventAccess, CGRequestListenEventAccess,
    CGRequestPostEventAccess,
};

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
