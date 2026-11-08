//! Platform ports and the menu model. The ports operate on the main thread, except `EventSink` and `AudioSink`.

use std::num::NonZeroU32;
use std::time::Duration;

use secrecy::SecretString;

use crate::domain::{CaptureSummary, FinalText, Generation, TargetInfo};
use crate::error::{AppError, AudioQueueError, SecretError};
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

/// Reads the API keys of the providers.
pub trait SecretStore {
    /// Returns the API key that the store keeps for `account`, the vendor name of the provider.
    ///
    /// # Errors
    ///
    /// `SecretError::NotFound` when the store has no key for `account`.
    /// `SecretError::Unavailable` when the store has the key but the read failed.
    fn get(&self, account: &str) -> Result<SecretString, SecretError>;
}

/// The identifier of a menu action: the index of the action in the list that `bloop-app` builds with the model.
/// It is valid only in that model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionId(pub usize);

/// The content of the status menu. The renderer adds "Quit Bloop" after the items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    /// The items, in display order.
    pub items: Vec<MenuItem>,
}

/// One item of the status menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// Information only. It has no action, thus `AppKit` disables it.
    Status {
        /// The text of the item.
        title: String,
    },
    /// An item that starts an action.
    Action {
        /// The text of the item.
        title: String,
        /// The action of a click.
        action: ActionId,
    },
    /// An item with a checkmark.
    Toggle {
        /// The text of the item.
        title: String,
        /// The action of a click.
        action: ActionId,
        /// `true` shows the checkmark.
        checked: bool,
    },
    /// An item with a submenu. A list of choices is a submenu of `Toggle` items.
    Submenu {
        /// The text of the item.
        title: String,
        /// The items of the submenu, in display order.
        items: Vec<MenuItem>,
    },
    /// A line between groups of items.
    Separator,
}

/// Gives the content of the status menu and receives its clicks. Main thread only.
///
/// `AppKit` calls both methods from Objective-C, thus they must not panic.
pub trait MenuSource {
    /// Returns the content of the menu. The renderer calls it each time the menu opens.
    fn model(&self) -> MenuModel;

    /// Starts the action of a click. `action` comes from the last model.
    fn activate(&self, action: ActionId);
}
