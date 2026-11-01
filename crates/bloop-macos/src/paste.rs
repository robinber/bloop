//! `Paster`: target check, pasteboard, and Command-V. It operates on the main thread.
//!
//! The pasteboard item supplies its text only when the target application reads it.
//! This read is the `Landed` result. Without a read within one second, the item becomes
//! a normal text item and the result is `Fallback(NotRead)`.

#![expect(
    unsafe_code,
    reason = "the pasteboard data provider is an Objective-C class, and the string type and the secure input check are C symbols"
)]

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use bloop_core::domain::{FinalText, Generation, TargetInfo};
use bloop_core::ports::platform::{EventSink, TextOutput};
use bloop_core::session::{Event, FallbackReason, InsertOutcome};
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardType,
    NSPasteboardTypeString, NSWorkspace,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventType,
};
use objc2_foundation::{NSArray, NSData, NSString, ns_string};

use crate::events::MainSink;

/// The value of `EventSourceUserData` in the events that Bloop posts.
const POSTED_EVENT: i64 = 0x0042_6C6F_6F70;
/// `kVK_ANSI_V`: the V key on the AZERTY and QWERTY layouts.
const KEY_V: u16 = 9;
const READ_TIME: Duration = Duration::from_secs(1);
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
        let provider = TextProvider::new(self.mtm, generation, text);
        let item = NSPasteboardItem::new();
        let marked = item.setDataProvider_forTypes(
            ProtocolObject::from_ref(&*provider),
            &NSArray::from_slice(&[string_type()]),
        ) && item
            .setData_forType(&NSData::new(), ns_string!("org.nspasteboard.TransientType"))
            && item.setData_forType(&NSData::new(), ns_string!("org.nspasteboard.ConcealedType"));

        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        self.provider = Some(provider.clone());
        let written = marked
            && pasteboard.writeObjects(&NSArray::from_slice(&[ProtocolObject::from_ref(&*item)]));
        if !written {
            tracing::error!("the clipboard refused the paste item");
            provider.give_up();
            return;
        }
        if !post_paste() {
            tracing::error!("Bloop could not create the paste command");
            provider.give_up();
            return;
        }

        let Ok(when) = DispatchTime::try_from(READ_TIME) else {
            provider.give_up();
            return;
        };
        let provider = MainThreadBound::new(provider, self.mtm);
        let queued = DispatchQueue::main().after(when, move || {
            provider.get_on_main(|provider| provider.give_up());
        });
        if let Err(error) = queued {
            tracing::error!(?error, "the read timer did not start");
        }
    }
}

impl TextOutput for Paster {
    fn record_target(&mut self, generation: Generation) -> TargetInfo {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication();
        self.targets.record(generation, current_target());
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
        write_text(&NSString::from_str(text.as_str()));
        self.provider = None;
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
    let process = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier());
    let inputs = INPUT_EVENTS.map(|event_type| {
        CGEventSource::counter_for_event_type(CGEventSourceStateID::HIDSystemState, event_type)
    });
    Target { process, inputs }
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

fn string_type() -> &'static NSPasteboardType {
    // SAFETY: AppKit sets this constant before `main` and never changes it.
    unsafe { NSPasteboardTypeString }
}

/// Replaces the clipboard content with a normal text item.
fn write_text(text: &NSString) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    if !pasteboard.setString_forType(text, string_type()) {
        tracing::error!("the clipboard refused the text");
    }
}

struct ProviderIvars {
    generation: Generation,
    text: Retained<NSString>,
    pending: Cell<bool>,
}

impl fmt::Debug for ProviderIvars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderIvars")
            .field("generation", &self.generation)
            .field("pending", &self.pending)
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

    unsafe impl NSObjectProtocol for TextProvider {}

    unsafe impl NSPasteboardItemDataProvider for TextProvider {
        #[unsafe(method(pasteboard:item:provideDataForType:))]
        fn provide_data_for_type(
            &self,
            _pasteboard: Option<&NSPasteboard>,
            item: &NSPasteboardItem,
            data_type: &NSPasteboardType,
        ) {
            if item.setString_forType(&self.ivars().text, data_type) {
                self.finish(InsertOutcome::Landed);
            } else {
                tracing::error!("the clipboard refused the text for the target");
            }
        }
    }
);

impl TextProvider {
    fn new(mtm: MainThreadMarker, generation: Generation, text: &FinalText) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ProviderIvars {
            generation,
            text: NSString::from_str(text.as_str()),
            pending: Cell::new(true),
        });
        // SAFETY: the `init` method of `NSObject` has no requirements, and the ivars are set.
        unsafe { msg_send![super(this), init] }
    }

    /// Without a read, replaces the item with a normal text item and reports `NotRead`.
    fn give_up(&self) {
        if self.ivars().pending.get() {
            write_text(&self.ivars().text);
            self.finish(InsertOutcome::Fallback(FallbackReason::NotRead));
        }
    }

    /// Ignores a later read or time limit of `generation`.
    fn cancel(&self, generation: Generation) {
        if self.ivars().generation == generation {
            self.ivars().pending.set(false);
        }
    }

    /// Sends the result one time.
    fn finish(&self, outcome: InsertOutcome) {
        if self.ivars().pending.replace(false) {
            MainSink.send(Event::InsertDone(self.ivars().generation, outcome));
        }
    }
}

#[cfg(test)]
mod tests {
    use bloop_core::domain::Generation;
    use bloop_core::session::{Command, Core, Event};
    use objc2_core_graphics::CGEvent;

    use super::{KEY_V, Target, Targets, is_posted, mark_posted};

    fn generations(count: usize) -> Vec<Generation> {
        let mut core = Core::new();
        (0..count)
            .map(|_| {
                let [Command::StartHoldTimer(generation)] =
                    core.handle(Event::DictationKeyDown)[..]
                else {
                    panic!("one hold timer");
                };
                assert_eq!(core.handle(Event::DictationKeyUp), [] as [Command; 0]);
                generation
            })
            .collect()
    }

    fn target(process: i32) -> Target {
        Target {
            process: Some(process),
            inputs: [0; 4],
        }
    }

    #[test]
    fn take_returns_the_target_of_the_generation() {
        let [first, second] = generations(2)[..] else {
            panic!("two generations");
        };
        let mut targets = Targets::default();
        targets.record(first, target(1));
        targets.record(second, target(2));

        assert_eq!(targets.take(first), Some(target(1)));
        assert_eq!(targets.take(second), Some(target(2)));
        assert_eq!(targets.take(second), None);
    }

    #[test]
    fn take_keeps_the_other_targets() {
        let [waiting, pasted] = generations(2)[..] else {
            panic!("two generations");
        };
        let mut targets = Targets::default();
        targets.record(waiting, target(1));
        targets.record(pasted, target(2));

        assert_eq!(targets.take(pasted), Some(target(2)));
        assert_eq!(targets.take(waiting), Some(target(1)));
    }

    #[test]
    fn repeated_jobs_without_a_paste_keep_no_target() {
        let mut targets = Targets::default();

        for generation in generations(1_000) {
            targets.record(generation, target(1));
            targets.remove(generation);
        }

        assert_eq!(targets.0.len(), 0);
    }

    #[test]
    fn an_unknown_generation_keeps_the_targets() {
        let [recorded, unknown] = generations(2)[..] else {
            panic!("two generations");
        };
        let mut targets = Targets::default();
        targets.record(recorded, target(1));

        assert_eq!(targets.take(unknown), None);
        assert_eq!(targets.take(recorded), Some(target(1)));
    }

    #[test]
    fn only_marked_events_are_posted_by_bloop() {
        let event = CGEvent::new_keyboard_event(None, KEY_V, true).expect("keyboard event");
        assert!(!is_posted(&event));

        mark_posted(&event);

        assert!(is_posted(&event));
    }
}
