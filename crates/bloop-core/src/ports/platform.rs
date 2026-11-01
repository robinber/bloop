//! Platform ports. The driver uses them on the main thread, except `EventSink` and `AudioSink`.

use std::num::NonZeroU32;
use std::time::Duration;

use crate::domain::{CaptureSummary, FinalText, Generation, TargetInfo};
use crate::error::{AppError, AudioQueueError};
use crate::session::{Event, OrbView};

/// Sends events to the driver on the main thread. Any thread can use it.
pub trait EventSink: Send + Sync {
    /// Queues the event for the driver.
    fn send(&self, event: Event);
}

/// Receives the microphone samples on the audio thread.
pub trait AudioSink: Send {
    /// Copies mono samples at the native sample rate into the bounded queue of the capture.
    ///
    /// It does not allocate, block, or perform I/O.
    ///
    /// # Errors
    ///
    /// `AudioQueueError::Full` when the queue has no space for the samples.
    /// The queue then keeps an overflow mark, and the capture fails.
    fn try_push(&mut self, samples: &[f32]) -> Result<(), AudioQueueError>;
}

/// Captures the microphone.
pub trait AudioInput {
    /// Starts the capture and sends its samples to `sink`. It does not wait for the provider connection.
    ///
    /// Returns the native sample rate of the device, in Hz. The rate does not change during the capture.
    ///
    /// # Errors
    ///
    /// A missing permission or an unavailable microphone.
    fn start(
        &mut self,
        generation: Generation,
        sink: Box<dyn AudioSink>,
    ) -> Result<NonZeroU32, AppError>;

    /// Stops the capture after its last samples, drops the sink, and returns the measures of the capture.
    ///
    /// # Errors
    ///
    /// The microphone failed during the capture.
    fn stop(&mut self, generation: Generation) -> Result<CaptureSummary, AppError>;

    /// Stops the capture and drops the sink.
    fn cancel(&mut self);
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

    /// Ends the insertion of this generation without a result: forgets its target and sends no `InsertDone`.
    ///
    /// A paste command that the adapter already posted can still land. The clipboard restore still follows the read.
    fn cancel(&mut self, generation: Generation);
}
