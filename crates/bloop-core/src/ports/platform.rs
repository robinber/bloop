//! Platform ports. The driver uses them on the main thread, except `EventSink`.

use std::time::Duration;

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
