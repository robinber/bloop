//! `SleepObserver`: sends `SystemSleep` to the core when the system is about to sleep.

#![expect(
    unsafe_code,
    reason = "objc2-app-kit gives the sleep notification name as an extern static, and objc2-foundation marks the block observer methods as unsafe"
)]

use std::ptr::NonNull;

use block2::RcBlock;
use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSWorkspace, NSWorkspaceWillSleepNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};

use crate::events::MainSink;

/// Observes `NSWorkspaceWillSleepNotification` and sends [`Event::SystemSleep`] through [`MainSink`].
///
/// The observer operates while this value exists. Keep the value until the run loop returns.
/// `Drop` removes the observer from the notification center of the shared `NSWorkspace`.
/// If `Drop` does not run, the observer stays registered. This is safe because the block captures nothing.
#[derive(Debug)]
#[must_use = "dropping a SleepObserver removes the sleep observer"]
pub struct SleepObserver {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl SleepObserver {
    /// Registers the observer on the notification center of the shared `NSWorkspace`.
    ///
    /// Call it on the main thread. The main operation queue runs the block, thus the block runs
    /// on the main thread.
    pub fn new(_mtm: MainThreadMarker) -> Self {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let queue = NSOperationQueue::mainQueue();
        let block = RcBlock::new(|_notification: NonNull<NSNotification>| {
            tracing::info!("the system will sleep");
            MainSink.send(Event::SystemSleep);
        });
        // SAFETY: AppKit sets this constant before `main` and never changes it.
        let name = unsafe { NSWorkspaceWillSleepNotification };
        // SAFETY: `obj` is `None`, thus no object type applies. The main queue runs the block on
        // the main thread. The block captures nothing, thus it is sendable. It only writes a log
        // line and sends an event through `MainSink`, which any thread can use and which cannot
        // panic. The center copies the block and keeps the copy until `Drop` removes the observer.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                Some(&queue),
                &block,
            )
        };
        Self { center, token }
    }
}

impl Drop for SleepObserver {
    fn drop(&mut self) {
        let token: &AnyObject = (*self.token).as_ref();
        // SAFETY: `token` is the observer object that `addObserverForName:object:queue:usingBlock:`
        // of `self.center` returned. `removeObserver:` expects that object.
        unsafe { self.center.removeObserver(token) };
    }
}
