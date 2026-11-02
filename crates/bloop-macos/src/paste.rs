//! `Paster`: target check, pasteboard, and Command-V. It operates on the main thread.
//!
//! Before the paste, Bloop saves the clipboard. The pasteboard item supplies its text only when
//! the target application reads it. 300 ms after this read, Bloop restores the saved clipboard if
//! nobody changed it, then reports `Landed`. Without a read within one second, the item becomes
//! a normal text item and the result is `Fallback(NotRead)`.
//!
//! A new paste first ends the previous transaction. If the target did not read the previous item,
//! Bloop restores the saved clipboard at that moment. Thus the new paste saves the content of the
//! user, not the item of Bloop.

#![expect(
    unsafe_code,
    reason = "the pasteboard data provider is an Objective-C class, and the secure input check is a C symbol"
)]

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use bloop_core::domain::{FinalText, Generation, TargetInfo};
use bloop_core::ports::platform::{EventSink, TextOutput};
use bloop_core::session::{Event, FallbackReason, InsertOutcome};
use dispatch2::MainThreadBound;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardType,
    NSRunningApplication, NSWorkspace,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventType,
};
use objc2_foundation::{NSArray, NSData, NSInteger, NSString, ns_string};

use self::clipboard::{
    SavedItem, owns_clipboard, restore, save, string_type, transient_type, write_text,
};
use crate::events::MainSink;
use crate::scheduler;

/// The value of `EventSourceUserData` in the events that Bloop posts.
const POSTED_EVENT: i64 = 0x0042_6C6F_6F70;
/// `kVK_ANSI_V`: the V key on the AZERTY and QWERTY layouts.
const KEY_V: u16 = 9;
const READ_TIME: Duration = Duration::from_secs(1);
const RESTORE_DELAY: Duration = Duration::from_millis(300);
const INPUT_EVENTS: [CGEventType; 4] = [
    CGEventType::KeyDown,
    CGEventType::LeftMouseDown,
    CGEventType::RightMouseDown,
    CGEventType::OtherMouseDown,
];

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    /// Returns a nonzero value while an application uses secure keyboard input, for example in a password field.
    ///
    /// The function has no parameters and only reads a system state. Thus a call cannot cause undefined behavior.
    safe fn IsSecureEventInputEnabled() -> u8;
}

/// Pastes the final texts. It implements [`TextOutput`] for the driver.
#[derive(Debug)]
pub struct Paster {
    mtm: MainThreadMarker,
    targets: Targets,
    provider: Option<Retained<TextProvider>>,
}

impl Paster {
    /// Creates the paste adapter. It sends `InsertDone` through [`MainSink`].
    #[must_use]
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            targets: Targets::default(),
            provider: None,
        }
    }

    fn paste(&mut self, generation: Generation, text: &FinalText) {
        let pasteboard = NSPasteboard::generalPasteboard();
        if let Some(previous) = self.provider.take() {
            previous.abandon(&pasteboard);
        }
        let saved = save(&pasteboard);
        let written = pasteboard.clearContents();
        let provider = TextProvider::new(self.mtm, generation, text, saved, written);
        let item = NSPasteboardItem::new();
        let marked = item.setDataProvider_forTypes(
            ProtocolObject::from_ref(&*provider),
            &NSArray::from_slice(&[string_type()]),
        ) && item.setData_forType(&NSData::new(), transient_type())
            && item.setData_forType(&NSData::new(), ns_string!("org.nspasteboard.ConcealedType"));

        self.provider = Some(provider.clone());
        let accepted = marked
            && pasteboard.writeObjects(&NSArray::from_slice(&[ProtocolObject::from_ref(&*item)]));
        if !accepted {
            tracing::error!("the clipboard refused the paste item");
            provider.give_up();
            return;
        }
        if !post_paste() {
            tracing::error!("Bloop could not create the paste command");
            provider.give_up();
            return;
        }
        after(READ_TIME, &provider, TextProvider::give_up);
    }
}

impl TextOutput for Paster {
    fn record_target(&mut self, generation: Generation) -> TargetInfo {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication();
        self.targets.record(generation, target_of(app.as_deref()));
        TargetInfo {
            bundle_id: app
                .as_ref()
                .and_then(|app| app.bundleIdentifier())
                .map(|id| id.to_string()),
            app_name: app
                .as_ref()
                .and_then(|app| app.localizedName())
                .map(|name| name.to_string()),
        }
    }

    fn insert(&mut self, generation: Generation, text: FinalText) {
        match check(self.targets.take(generation)) {
            Ok(()) => self.paste(generation, &text),
            Err(reason) => {
                tracing::info!(?reason, "the target check failed");
                self.copy(&text);
                MainSink.send(Event::InsertDone(
                    generation,
                    InsertOutcome::Fallback(reason),
                ));
            }
        }
    }

    fn copy(&mut self, text: &FinalText) {
        let pasteboard = NSPasteboard::generalPasteboard();
        write_text(&pasteboard, &NSString::from_str(text.as_str()));
    }

    fn cancel(&mut self, generation: Generation) {
        self.targets.remove(generation);
        if let Some(provider) = &self.provider {
            provider.cancel(generation);
        }
    }
}

/// The state at the end of a capture that the target check compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    process: Option<i32>,
    inputs: [u32; INPUT_EVENTS.len()],
}

fn current_target() -> Target {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication();
    target_of(app.as_deref())
}

fn target_of(app: Option<&NSRunningApplication>) -> Target {
    let inputs = INPUT_EVENTS.map(|event_type| {
        CGEventSource::counter_for_event_type(CGEventSourceStateID::HIDSystemState, event_type)
    });
    Target {
        process: app.map(NSRunningApplication::processIdentifier),
        inputs,
    }
}

/// The targets that wait for their paste, in session order.
#[derive(Debug, Default)]
struct Targets(VecDeque<(Generation, Target)>);

impl Targets {
    fn record(&mut self, generation: Generation, target: Target) {
        self.0.push_back((generation, target));
    }

    /// Removes and returns the target of `generation`.
    fn take(&mut self, generation: Generation) -> Option<Target> {
        let index = self
            .0
            .iter()
            .position(|(recorded, _)| *recorded == generation)?;
        self.0.remove(index).map(|(_, target)| target)
    }

    fn remove(&mut self, generation: Generation) {
        self.0.retain(|(recorded, _)| *recorded != generation);
    }
}

/// The target check: no secure input, the same frontmost application, and no key press or click since the record.
///
/// The counters include only hardware events. Thus the paste commands of Bloop do not change them.
fn check(recorded: Option<Target>) -> Result<(), FallbackReason> {
    if IsSecureEventInputEnabled() != 0 {
        return Err(FallbackReason::SecureInput);
    }
    if recorded == Some(current_target()) {
        Ok(())
    } else {
        Err(FallbackReason::TargetChanged)
    }
}

/// Posts Command-V: key code 9 with the Command flag only. The event tap ignores these events.
fn post_paste() -> bool {
    let Some(source) = CGEventSource::new(CGEventSourceStateID::Private) else {
        return false;
    };
    let [Some(down), Some(up)] =
        [true, false].map(|down| CGEvent::new_keyboard_event(Some(&source), KEY_V, down))
    else {
        return false;
    };
    for event in [&down, &up] {
        CGEvent::set_flags(Some(event), CGEventFlags::MaskCommand);
        mark_posted(event);
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(event));
    }
    true
}

fn mark_posted(event: &CGEvent) {
    CGEvent::set_integer_value_field(Some(event), CGEventField::EventSourceUserData, POSTED_EVENT);
}

/// Returns `true` for an event that Bloop posted.
pub(crate) fn is_posted(event: &CGEvent) -> bool {
    CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData) == POSTED_EVENT
}

/// Runs `action` on the provider after `delay`, on the main queue.
fn after(delay: Duration, provider: &Retained<TextProvider>, action: fn(&TextProvider)) {
    let target = MainThreadBound::new(provider.clone(), provider.mtm());
    scheduler::run_after(delay, move || {
        target.get_on_main(|provider| action(provider));
    });
}

/// The steps of one paste, without macOS types: wait for the read, wait for the restore, done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Reading,
    Restoring,
    Done,
}

/// The clipboard transaction of one paste.
#[derive(Debug, Clone, Copy)]
struct Transaction {
    phase: Phase,
    cancelled: bool,
}

impl Default for Transaction {
    fn default() -> Self {
        Self {
            phase: Phase::Reading,
            cancelled: false,
        }
    }
}

impl Transaction {
    /// The target read the text. Returns `true` if the restore delay must start.
    fn read(&mut self) -> bool {
        self.advance(Phase::Reading, Phase::Restoring)
    }

    /// The read time limit passed, or a new paste starts. Returns `true` if no read occurred:
    /// Bloop gives up the read.
    fn read_time_out(&mut self) -> bool {
        self.advance(Phase::Reading, Phase::Done)
    }

    /// The restore delay passed. Returns `true` if Bloop must restore the clipboard.
    fn restore_due(&mut self) -> bool {
        self.advance(Phase::Restoring, Phase::Done)
    }

    /// The job ended: the steps continue, but Bloop reports no result.
    fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// The result to report, or nothing after a cancel.
    fn result(self, outcome: InsertOutcome) -> Option<InsertOutcome> {
        (!self.cancelled).then_some(outcome)
    }

    fn advance(&mut self, from: Phase, to: Phase) -> bool {
        let current = self.phase == from;
        if current {
            self.phase = to;
        }
        current
    }
}

struct ProviderIvars {
    generation: Generation,
    text: Retained<NSString>,
    saved: Cell<Option<Vec<SavedItem>>>,
    written: NSInteger,
    transaction: Cell<Transaction>,
}

impl fmt::Debug for ProviderIvars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderIvars")
            .field("generation", &self.generation)
            .field("transaction", &self.transaction)
            .finish_non_exhaustive()
    }
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `TextProvider` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BloopTextProvider"]
    #[ivars = ProviderIvars]
    #[derive(Debug)]
    struct TextProvider;

    // SAFETY: `NSObject` implements all methods of `NSObjectProtocol`.
    unsafe impl NSObjectProtocol for TextProvider {}

    // SAFETY: the method is the only required method of the protocol. Its selector, argument
    // types (with a nullable pasteboard), and empty result match the `objc2-app-kit` declaration.
    unsafe impl NSPasteboardItemDataProvider for TextProvider {
        /// Gives the text to the target when it reads the item.
        ///
        /// Apple documents no thread for this callback. Bloop observed it on the main thread, and
        /// the `Cell` ivars and the timers on the main queue depend on that. A call on another
        /// thread logs an error that records the violation, then continues without a change.
        /// This check does not break the `MainThreadMarker` rule in `AGENTS.md`: it does not call
        /// `expect`, and AppKit gives no marker to its callback.
        #[unsafe(method(pasteboard:item:provideDataForType:))]
        fn provide_data_for_type(
            &self,
            _pasteboard: Option<&NSPasteboard>,
            item: &NSPasteboardItem,
            data_type: &NSPasteboardType,
        ) {
            if MainThreadMarker::new().is_none() {
                tracing::error!("AppKit called the text provider off the main thread");
            }
            if item.setString_forType(&self.ivars().text, data_type) {
                if self.step(Transaction::read) {
                    after(RESTORE_DELAY, &self.retain(), Self::restore_now);
                }
            } else {
                tracing::error!("the clipboard refused the text for the target");
            }
        }
    }
);

impl TextProvider {
    fn new(
        mtm: MainThreadMarker,
        generation: Generation,
        text: &FinalText,
        saved: Option<Vec<SavedItem>>,
        written: NSInteger,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ProviderIvars {
            generation,
            text: NSString::from_str(text.as_str()),
            saved: Cell::new(saved),
            written,
            transaction: Cell::new(Transaction::default()),
        });
        // SAFETY: the `init` method of `NSObject` has no requirements, and the ivars are set.
        unsafe { msg_send![super(this), init] }
    }

    /// Without a read, replaces the item with a normal text item and reports `NotRead`.
    ///
    /// If another application changed the clipboard, Bloop keeps that content.
    fn give_up(&self) {
        if self.step(Transaction::read_time_out) {
            drop(self.ivars().saved.take());
            let pasteboard = NSPasteboard::generalPasteboard();
            if owns_clipboard(&pasteboard, self.ivars().written) {
                write_text(&pasteboard, &self.ivars().text);
            }
            self.report(InsertOutcome::Fallback(FallbackReason::NotRead));
        }
    }

    /// Restores the saved clipboard after the read, then reports `Landed`.
    fn restore_now(&self) {
        if self.step(Transaction::restore_due) {
            let pasteboard = NSPasteboard::generalPasteboard();
            restore(&pasteboard, self.ivars().saved.take(), self.ivars().written);
            self.report(InsertOutcome::Landed);
        }
    }

    /// A new paste starts. Without a read, restores the saved clipboard now and reports nothing.
    ///
    /// After a read, the restore delay continues. Its restore then finds the clipboard of the new
    /// paste and keeps it.
    fn abandon(&self, pasteboard: &NSPasteboard) {
        if self.step(Transaction::read_time_out) {
            tracing::info!("a new paste starts before the target read the previous text");
            restore(pasteboard, self.ivars().saved.take(), self.ivars().written);
        }
    }

    /// Reports no result for `generation`. The read and the restore still follow their steps.
    fn cancel(&self, generation: Generation) {
        if self.ivars().generation == generation {
            self.step(|transaction| {
                transaction.cancel();
                false
            });
        }
    }

    fn step(&self, change: impl FnOnce(&mut Transaction) -> bool) -> bool {
        let mut transaction = self.ivars().transaction.get();
        let next = change(&mut transaction);
        self.ivars().transaction.set(transaction);
        next
    }

    fn report(&self, outcome: InsertOutcome) {
        let transaction = self.ivars().transaction.get();
        if let Some(outcome) = transaction.result(outcome) {
            MainSink.send(Event::InsertDone(self.ivars().generation, outcome));
        }
    }
}

mod clipboard;
#[cfg(test)]
mod tests;
