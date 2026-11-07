//! Input Monitoring, Accessibility, and Microphone permission checks, and the System Settings links.
//!
//! The active event tap needs Input Monitoring and Accessibility. Accessibility also lets Bloop
//! post the paste command. macOS can keep the old result until Bloop starts again.

#![expect(
    unsafe_code,
    reason = "objc2-av-foundation marks the microphone authorization methods and the audio media type as unsafe"
)]

use block2::RcBlock;
use bloop_core::error::Permission;
use objc2::runtime::Bool;
use objc2_app_kit::NSWorkspace;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_graphics::{
    CGPreflightListenEventAccess, CGPreflightPostEventAccess, CGRequestListenEventAccess,
    CGRequestPostEventAccess,
};
use objc2_foundation::{NSString, NSURL};

/// Asks macOS for each missing permission and returns the permissions that are still missing.
///
/// macOS shows its request one time for each permission. After that, the user must enable
/// Bloop in System Settings. The microphone request does not wait for the answer, thus the
/// microphone is not missing until the user denies access.
#[must_use]
pub fn request_missing() -> Vec<Permission> {
    let mut missing = Vec::new();
    if !CGPreflightListenEventAccess() && !CGRequestListenEventAccess() {
        missing.push(Permission::InputMonitoring);
    }
    if !CGPreflightPostEventAccess() && !CGRequestPostEventAccess() {
        missing.push(Permission::Accessibility);
    }
    if !microphone_allowed() {
        missing.push(Permission::Microphone);
    }
    missing
}

/// Reads the microphone authorization of Bloop.
///
/// If the user did not answer yet, it shows the request of macOS and returns `true` without a wait.
/// It returns `false` if the user denied access or a restriction blocks it.
#[must_use]
pub fn microphone_allowed() -> bool {
    // SAFETY: `AVMediaTypeAudio` is a constant of AVFoundation that nothing writes.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        tracing::error!("AVFoundation has no audio media type");
        return false;
    };
    // SAFETY: the method accepts `AVMediaTypeAudio`, thus it raises no exception.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::Authorized => true,
        AVAuthorizationStatus::NotDetermined => {
            let handler = RcBlock::new(|granted: Bool| {
                tracing::info!(
                    granted = granted.as_bool(),
                    "the user answered the microphone request"
                );
            });
            // SAFETY: the method accepts `AVMediaTypeAudio`. It copies the block before it returns,
            // thus the block outlives `handler`. AVFoundation calls the handler on any dispatch
            // queue: it captures nothing and only writes a log line, which cannot panic.
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler);
            }
            true
        }
        status => {
            tracing::warn!(?status, "microphone access is not allowed");
            false
        }
    }
}

/// Opens the System Settings pane where the user enables `permission` for Bloop.
pub fn open_settings(permission: Permission) {
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
