//! Platform ports. The driver uses them on the main thread, except `EventSink`.

use std::time::Duration;

use crate::domain::{FinalText, Generation, TargetInfo};
use crate::session::{Event, OrbView};

/// Sends events to the driver on the main thread. Any thread can use it.
pub trait EventSink: Send + Sync {
    /// Queues the event for the driver.
    fn send(&self, event: Event);
}

/// Sends events to the driver after a delay.
pub trait Scheduler {
    /// Sends `event` to the driver after `delay`.
    fn after(&self, delay: Duration, event: Event);
}

/// Shows the orb on the screen.
pub trait Overlay {
    /// Opens the orb with this view, or changes its current view.
    fn show(&mut self, view: OrbView);

    /// Closes the orb with its closing animation.
    fn close(&mut self);
}

/// Pastes the final text in the application that had the focus at the end of the capture.
pub trait TextOutput {
    /// Records the frontmost application and the input counters for the target check.
    fn record_target(&mut self, generation: Generation) -> TargetInfo;

    /// Pastes the text if the target check passes. Otherwise, puts the text in the clipboard.
    ///
    /// It returns immediately. The adapter sends `Event::InsertDone` later.
    fn insert(&mut self, generation: Generation, text: FinalText);

    /// Puts the text in the clipboard.
    fn copy(&mut self, text: &FinalText);

    /// Ends the insertion of this generation without a result: forgets its target, and ignores a later read.
    ///
    /// A paste command that the adapter already posted can still land.
    fn cancel(&mut self, generation: Generation);
}
