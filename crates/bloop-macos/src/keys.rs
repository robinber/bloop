//! Event tap for the dictation key, other input, and Escape.
//!
//! The tap runs on the main run loop. It passes all events to the applications, except Escape
//! during a capture. It blocks that Escape press completely: the key down, the key repeats, and
//! the key up. It sends the events of the state machine through [`MainSink`]. It ignores the
//! paste commands that Bloop posts.

#![expect(
    unsafe_code,
    reason = "the event tap calls a C callback with a context pointer that the compiler cannot check"
)]

use std::cell::{Cell, OnceCell};
use std::ffi::c_void;
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::{self, NonNull};
use std::rc::Rc;

use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use objc2::MainThreadMarker;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use crate::events::MainSink;
use crate::paste;

/// `kVK_RightCommand`.
const RIGHT_COMMAND: i64 = 0x36;
/// `kVK_Escape`.
const ESCAPE: i64 = 0x35;
/// `NX_DEVICERCMDKEYMASK`.
const RIGHT_COMMAND_DOWN: u64 = 0x10;
/// `NX_DEVICELCMDKEYMASK`.
const LEFT_COMMAND_DOWN: u64 = 0x08;
const OTHER_MODIFIERS: u64 = CGEventFlags::MaskShift.bits()
    | CGEventFlags::MaskControl.bits()
    | CGEventFlags::MaskAlternate.bits()
    | LEFT_COMMAND_DOWN;

const EVENTS: CGEventMask = mask(CGEventType::KeyDown)
    | mask(CGEventType::KeyUp)
    | mask(CGEventType::FlagsChanged)
    | mask(CGEventType::LeftMouseDown)
    | mask(CGEventType::RightMouseDown)
    | mask(CGEventType::OtherMouseDown);

const fn mask(event_type: CGEventType) -> CGEventMask {
    1 << event_type.0
}

/// The active event tap. It operates while this value exists.
#[derive(Debug)]
pub struct KeyTap {
    state: Rc<TapState>,
    source: CFRetained<CFRunLoopSource>,
}

impl KeyTap {
    /// Installs the tap on the main run loop.
    ///
    /// Returns `None` if macOS refuses the tap, for example because Input Monitoring is missing.
    #[must_use]
    pub fn install(_mtm: MainThreadMarker) -> Option<Self> {
        let state = Rc::new(TapState::default());
        let user_info = Rc::as_ptr(&state).cast_mut().cast::<c_void>();
        // SAFETY: `tap_callback` has the `CGEventTapCallBack` signature and does not unwind.
        // `user_info` points to `state`. The returned `KeyTap` keeps `state` alive, and its `Drop`
        // invalidates the tap first. On an early return below, the port is released with `state`,
        // and the callback never runs because no run loop has the source.
        let port = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                EVENTS,
                Some(tap_callback),
                user_info,
            )
        }?;
        let port = state.port.get_or_init(|| port);
        let source = CFMachPort::new_run_loop_source(None, Some(port), 0)?;
        let mode = common_modes()?;
        CFRunLoop::main()?.add_source(Some(&source), Some(mode));
        Some(Self { state, source })
    }

    /// Tells the tap if a capture is active. The tap blocks Escape only during a capture.
    pub fn set_capture_active(&self, active: bool) {
        self.state.capture_active.set(active);
    }
}

impl Drop for KeyTap {
    fn drop(&mut self) {
        self.source.invalidate();
        if let Some(port) = self.state.port.get() {
            port.invalidate();
        }
    }
}

fn common_modes() -> Option<&'static CFRunLoopMode> {
    // SAFETY: Core Foundation sets this constant before `main` and never changes it.
    unsafe { kCFRunLoopCommonModes }
}

/// # Safety
///
/// `user_info` must be null or point to a live `TapState` on the current thread.
unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `KeyTap::install` gives a pointer to the `TapState` that the `KeyTap` keeps alive
    // until it invalidates the tap. The main run loop calls this function, on the thread of the
    // `KeyTap`. The code creates only shared references to `TapState`.
    let state = unsafe { user_info.cast::<TapState>().as_ref() };
    // SAFETY: the tap gives a valid event that stays valid during this call.
    let event_ref = unsafe { event.as_ref() };
    let verdict = state.map_or(Verdict::Pass, |state| {
        panic::catch_unwind(AssertUnwindSafe(|| state.handle(event_type, event_ref)))
            .unwrap_or_else(|_| {
                tracing::error!("the event tap handler panicked, the event passes");
                Verdict::Pass
            })
    });
    match verdict {
        Verdict::Pass => event.as_ptr(),
        Verdict::Block => ptr::null_mut(),
    }
}

#[derive(Debug, Default)]
struct TapState {
    keys: Cell<Keys>,
    capture_active: Cell<bool>,
    port: OnceCell<CFRetained<CFMachPort>>,
}

impl TapState {
    fn handle(&self, event_type: CGEventType, event: &CGEvent) -> Verdict {
        if paste::is_posted(event) {
            return Verdict::Pass;
        }
        let mut keys = self.keys.get();
        let (verdict, signal) = match event_type {
            CGEventType::FlagsChanged => {
                let flags = CGEvent::flags(Some(event)).bits();
                (Verdict::Pass, keys.flags_changed(keycode(event), flags))
            }
            CGEventType::KeyDown => {
                keys.key_down(keycode(event), is_repeat(event), self.capture_active.get())
            }
            CGEventType::KeyUp => (keys.key_up(keycode(event)), None),
            CGEventType::LeftMouseDown
            | CGEventType::RightMouseDown
            | CGEventType::OtherMouseDown => (Verdict::Pass, keys.other_input()),
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                self.enable_again(event_type);
                (Verdict::Pass, None)
            }
            _ => (Verdict::Pass, None),
        };
        self.keys.set(keys);
        if let Some(signal) = signal {
            MainSink.send(signal);
        }
        verdict
    }

    fn enable_again(&self, reason: CGEventType) {
        tracing::warn!(?reason, "macOS disabled the event tap, enabling it again");
        if let Some(port) = self.port.get() {
            CGEvent::tap_enable(port, true);
        }
    }
}

fn keycode(event: &CGEvent) -> i64 {
    CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode)
}

fn is_repeat(event: &CGEvent) -> bool {
    CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventAutorepeat) != 0
}

/// What the tap does with an input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Pass,
    Block,
}

/// The key logic of the tap, without macOS types.
#[derive(Debug, Clone, Copy, Default)]
struct Keys {
    /// Right Command is down.
    held: bool,
    /// The tap blocked the key down of the current Escape press.
    escape_blocked: bool,
}

impl Keys {
    fn flags_changed(&mut self, keycode: i64, flags: u64) -> Option<Event> {
        if keycode != RIGHT_COMMAND {
            return self.other_input();
        }
        let down = flags & RIGHT_COMMAND_DOWN != 0;
        if down == self.held {
            return None;
        }
        self.held = down;
        if !down {
            Some(Event::DictationKeyUp)
        } else if flags & OTHER_MODIFIERS == 0 {
            Some(Event::DictationKeyDown)
        } else {
            None
        }
    }

    fn key_down(
        &mut self,
        keycode: i64,
        repeat: bool,
        capture_active: bool,
    ) -> (Verdict, Option<Event>) {
        if keycode != ESCAPE {
            return (Verdict::Pass, self.other_input());
        }
        if capture_active {
            self.escape_blocked = true;
            (Verdict::Block, Some(Event::Escape))
        } else if repeat && self.escape_blocked {
            (Verdict::Block, None)
        } else {
            self.escape_blocked = false;
            (Verdict::Pass, self.other_input())
        }
    }

    fn key_up(&mut self, keycode: i64) -> Verdict {
        if keycode == ESCAPE && mem::take(&mut self.escape_blocked) {
            Verdict::Block
        } else {
            Verdict::Pass
        }
    }

    fn other_input(self) -> Option<Event> {
        self.held.then_some(Event::OtherInputDuringHold)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use bloop_core::session::Event;
    use objc2_core_graphics::CGEventFlags;

    use super::{ESCAPE, Keys, LEFT_COMMAND_DOWN, RIGHT_COMMAND, RIGHT_COMMAND_DOWN, Verdict};

    const LEFT_COMMAND: i64 = 0x37;
    const LEFT_SHIFT: i64 = 0x38;
    const KEY_C: i64 = 0x08;
    const COMMAND: u64 = CGEventFlags::MaskCommand.bits();
    const SHIFT: u64 = CGEventFlags::MaskShift.bits();

    fn holding() -> Keys {
        let mut keys = Keys::default();
        assert_matches!(
            keys.flags_changed(RIGHT_COMMAND, COMMAND | RIGHT_COMMAND_DOWN),
            Some(Event::DictationKeyDown)
        );
        keys
    }

    #[test]
    fn right_command_alone_presses_and_releases_the_dictation_key() {
        let mut keys = holding();

        assert_matches!(
            keys.flags_changed(RIGHT_COMMAND, 0),
            Some(Event::DictationKeyUp)
        );
        assert_matches!(keys.key_down(KEY_C, false, false), (Verdict::Pass, None));
    }

    #[test]
    fn right_command_after_another_modifier_is_not_the_dictation_key() {
        let mut keys = Keys::default();

        let down = keys.flags_changed(RIGHT_COMMAND, SHIFT | COMMAND | RIGHT_COMMAND_DOWN);

        assert_matches!(down, None);
    }

    #[test]
    fn left_command_does_not_hide_the_right_command_release() {
        let mut keys = holding();
        let both = COMMAND | RIGHT_COMMAND_DOWN | LEFT_COMMAND_DOWN;

        assert_matches!(
            keys.flags_changed(LEFT_COMMAND, both),
            Some(Event::OtherInputDuringHold)
        );
        assert_matches!(
            keys.flags_changed(RIGHT_COMMAND, COMMAND | LEFT_COMMAND_DOWN),
            Some(Event::DictationKeyUp)
        );
    }

    #[test]
    fn input_during_the_hold_is_other_input_and_passes() {
        let mut keys = holding();

        assert_matches!(
            keys.key_down(KEY_C, false, true),
            (Verdict::Pass, Some(Event::OtherInputDuringHold))
        );
        assert_matches!(
            keys.flags_changed(LEFT_SHIFT, COMMAND | RIGHT_COMMAND_DOWN | SHIFT),
            Some(Event::OtherInputDuringHold)
        );
        assert_matches!(keys.other_input(), Some(Event::OtherInputDuringHold));
    }

    #[test]
    fn input_without_the_hold_sends_nothing() {
        let mut keys = Keys::default();

        assert_matches!(keys.key_down(KEY_C, false, false), (Verdict::Pass, None));
        assert_matches!(keys.flags_changed(LEFT_SHIFT, SHIFT), None);
        assert_matches!(keys.other_input(), None);
    }

    #[test]
    fn escape_is_blocked_only_during_a_capture() {
        assert_matches!(
            holding().key_down(ESCAPE, false, true),
            (Verdict::Block, Some(Event::Escape))
        );
        assert_matches!(
            holding().key_down(ESCAPE, false, false),
            (Verdict::Pass, Some(Event::OtherInputDuringHold))
        );
        assert_matches!(
            Keys::default().key_down(ESCAPE, false, false),
            (Verdict::Pass, None)
        );
        assert_matches!(
            Keys::default().key_down(ESCAPE, true, false),
            (Verdict::Pass, None)
        );
        assert_eq!(Keys::default().key_up(ESCAPE), Verdict::Pass);
    }

    #[test]
    fn a_blocked_escape_press_stays_blocked_after_the_capture() {
        let mut keys = holding();
        assert_matches!(
            keys.key_down(ESCAPE, false, true),
            (Verdict::Block, Some(Event::Escape))
        );

        assert_matches!(keys.key_down(ESCAPE, true, false), (Verdict::Block, None));
        assert_eq!(keys.key_up(ESCAPE), Verdict::Block);
        assert_matches!(
            keys.key_down(ESCAPE, false, false),
            (Verdict::Pass, Some(Event::OtherInputDuringHold))
        );
        assert_eq!(keys.key_up(ESCAPE), Verdict::Pass);
    }

    #[test]
    fn a_new_escape_press_ends_the_block_without_the_key_up() {
        let mut keys = Keys::default();
        assert_matches!(
            keys.key_down(ESCAPE, false, true),
            (Verdict::Block, Some(Event::Escape))
        );

        assert_matches!(keys.key_down(ESCAPE, false, false), (Verdict::Pass, None));
        assert_eq!(keys.key_up(ESCAPE), Verdict::Pass);
    }
}
