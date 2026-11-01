//! `MainScheduler`: main queue timers.

use std::time::Duration;

use bloop_core::ports::platform::Scheduler;
use bloop_core::session::Event;
use dispatch2::{DispatchQueue, DispatchTime};

use crate::events;

/// Sends delayed events to the handler of [`events::set_handler`] on the main thread.
#[derive(Debug, Clone, Copy, Default)]
pub struct MainScheduler;

impl Scheduler for MainScheduler {
    fn after(&self, delay: Duration, event: Event) {
        let Ok(when) = DispatchTime::try_from(delay) else {
            tracing::error!(?delay, "the delay is too long for a timer");
            return;
        };
        if let Err(error) = DispatchQueue::main().after(when, move || events::deliver(event)) {
            tracing::error!(?error, "the timer did not start");
        }
    }
}
