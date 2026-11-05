//! Status item and menu: the status of Bloop, "Correct text", "Copy last result", and "Quit Bloop".

#![expect(
    unsafe_code,
    reason = "a menu item action is an Objective-C selector that the compiler cannot check, and its target is an Objective-C class"
)]

use std::cell::Cell;
use std::fmt;

use bloop_core::error::{Permission, SecretError};
use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlStateValue, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSString, ns_string};

use crate::events::MainSink;
use crate::permissions;

/// The Bloop item in the menu bar. The item stays in the menu bar while this value exists.
#[derive(Debug)]
pub struct StatusMenu {
    _item: Retained<NSStatusItem>,
    _target: Retained<MenuTarget>,
}

impl StatusMenu {
    /// Adds the Bloop item to the menu bar.
    ///
    /// The menu shows the status at start: the missing permissions and API key, or if the event tap operates.
    /// macOS can keep the old permission result until Bloop starts again.
    ///
    /// The item "Correct text" has a checkmark and is off at start.
    /// Each click changes it and gives the new state to `correct`, on the main thread.
    /// `correct` runs inside an `AppKit` callback, thus it must not panic.
    #[must_use]
    pub fn new(
        mtm: MainThreadMarker,
        missing: &[Permission],
        keys_ready: bool,
        key: Option<SecretError>,
        correct: Box<dyn Fn(bool)>,
    ) -> Self {
        let target = MenuTarget::new(mtm, Toggle::new(correct));
        let menu = NSMenu::new(mtm);
        for line in status(missing, keys_ready, key) {
            let item = match line {
                Line::Text(title) => menu_item(mtm, title, None, ""),
                Line::Settings(permission) => {
                    let item = menu_item(
                        mtm,
                        settings_title(permission),
                        Some(settings_action(permission)),
                        "",
                    );
                    set_target(&item, &target);
                    item
                }
            };
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let correction = menu_item(mtm, "Correct text", Some(sel!(toggleCorrection:)), "");
        correction.setState(check(false));
        set_target(&correction, &target);
        menu.addItem(&correction);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let copy = menu_item(mtm, "Copy last result", Some(sel!(copyLastResult:)), "");
        set_target(&copy, &target);
        menu.addItem(&copy);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, "Quit Bloop", Some(sel!(terminate:)), "q"));

        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        item.setMenu(Some(&menu));
        if let Some(button) = item.button(mtm) {
            let icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("waveform"),
                Some(ns_string!("Bloop")),
            );
            match icon {
                Some(icon) => button.setImage(Some(&icon)),
                None => button.setTitle(ns_string!("Bloop")),
            }
        }
        Self {
            _item: item,
            _target: target,
        }
    }
}

/// A line of the status at the top of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// Information only. `AppKit` disables an item without an action.
    Text(&'static str),
    /// Opens the System Settings pane of a missing permission.
    Settings(Permission),
}

fn status(missing: &[Permission], keys_ready: bool, key: Option<SecretError>) -> Vec<Line> {
    let mut lines: Vec<Line> = missing.iter().copied().map(Line::Settings).collect();
    lines.extend(key.map(|error| Line::Text(key_title(error))));
    if !lines.is_empty() {
        lines.push(Line::Text("Then open Bloop again"));
        lines
    } else if keys_ready {
        vec![Line::Text("Ready: hold Right Command to dictate")]
    } else {
        vec![Line::Text("Open Bloop again to use the dictation key")]
    }
}

fn settings_title(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "Allow Microphone access…",
        Permission::Accessibility => "Allow Accessibility access…",
        Permission::InputMonitoring => "Allow Input Monitoring…",
    }
}

fn key_title(error: SecretError) -> &'static str {
    match error {
        SecretError::NotFound => "Add the API key to Keychain",
        SecretError::Unavailable => "Allow Bloop to read the API key in Keychain",
    }
}

fn settings_action(permission: Permission) -> Sel {
    match permission {
        Permission::Microphone => sel!(openMicrophoneSettings:),
        Permission::Accessibility => sel!(openAccessibilitySettings:),
        Permission::InputMonitoring => sel!(openInputMonitoringSettings:),
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::alloc(mtm);
    // SAFETY: each action is a selector from `sel!`. `toggleCorrection:`, `copyLastResult:`, and the
    // settings selectors go to a `MenuTarget`, which implements them. `terminate:` has no target,
    // thus AppKit sends it through the responder chain to `NSApplication`, which implements it.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            item,
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

fn set_target(item: &NSMenuItem, target: &MenuTarget) {
    let target: &AnyObject = target;
    // SAFETY: `NSMenuItem.target` is a weak reference. `StatusMenu` keeps the target alive as long
    // as its status item, which owns the menu.
    unsafe { item.setTarget(Some(target)) };
}

/// The state of a menu item with a checkmark, and the function that gets each new state.
struct Toggle {
    on: Cell<bool>,
    changed: Box<dyn Fn(bool)>,
}

impl Toggle {
    /// Starts off.
    fn new(changed: Box<dyn Fn(bool)>) -> Self {
        Self {
            on: Cell::new(false),
            changed,
        }
    }

    /// Changes the state, gives the new state to the function, and returns it.
    fn flip(&self) -> bool {
        let on = !self.on.get();
        self.on.set(on);
        (self.changed)(on);
        on
    }
}

impl fmt::Debug for Toggle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Toggle")
            .field("on", &self.on)
            .finish_non_exhaustive()
    }
}

fn check(on: bool) -> NSControlStateValue {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `MenuTarget` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BloopMenuTarget"]
    #[ivars = Toggle]
    #[derive(Debug)]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(toggleCorrection:))]
        fn toggle_correction(&self, sender: Option<&AnyObject>) {
            let on = self.ivars().flip();
            let Some(item) = sender.and_then(AnyObject::downcast_ref::<NSMenuItem>) else {
                tracing::warn!("the correction toggle has no menu item to update");
                return;
            };
            item.setState(check(on));
        }

        #[unsafe(method(copyLastResult:))]
        fn copy_last_result(&self, _sender: Option<&AnyObject>) {
            MainSink.send(Event::CopyLastResult);
        }

        #[unsafe(method(openMicrophoneSettings:))]
        fn open_microphone_settings(&self, _sender: Option<&AnyObject>) {
            permissions::open_settings(Permission::Microphone);
        }

        #[unsafe(method(openAccessibilitySettings:))]
        fn open_accessibility_settings(&self, _sender: Option<&AnyObject>) {
            permissions::open_settings(Permission::Accessibility);
        }

        #[unsafe(method(openInputMonitoringSettings:))]
        fn open_input_monitoring_settings(&self, _sender: Option<&AnyObject>) {
            permissions::open_settings(Permission::InputMonitoring);
        }
    }

    // SAFETY: `NSObject` implements all methods of `NSObjectProtocol`.
    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, toggle: Toggle) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(toggle);
        // SAFETY: the `init` method of `NSObject` has no requirements.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use bloop_core::error::{Permission, SecretError};

    use super::{Line, Toggle, status};

    #[test]
    fn each_flip_changes_the_toggle_and_gives_the_new_state() {
        let states = Rc::new(RefCell::new(Vec::new()));
        let received = Rc::clone(&states);
        let toggle = Toggle::new(Box::new(move |on| received.borrow_mut().push(on)));

        assert!(states.borrow().is_empty());
        assert!(toggle.flip());
        assert!(!toggle.flip());
        assert!(toggle.flip());

        assert_eq!(*states.borrow(), [true, false, true]);
    }

    #[test]
    fn with_all_permissions_the_status_says_how_to_dictate() {
        assert_eq!(
            status(&[], true, None),
            [Line::Text("Ready: hold Right Command to dictate")]
        );
    }

    #[test]
    fn each_missing_permission_has_a_settings_item() {
        assert_eq!(
            status(
                &[Permission::InputMonitoring, Permission::Accessibility],
                false,
                None
            ),
            [
                Line::Settings(Permission::InputMonitoring),
                Line::Settings(Permission::Accessibility),
                Line::Text("Then open Bloop again"),
            ]
        );
    }

    #[test]
    fn a_refused_tap_without_missing_permissions_asks_for_a_restart() {
        assert_eq!(
            status(&[], false, None),
            [Line::Text("Open Bloop again to use the dictation key")]
        );
    }

    #[test]
    fn a_missing_key_asks_to_add_it() {
        assert_eq!(
            status(&[], true, Some(SecretError::NotFound)),
            [
                Line::Text("Add the API key to Keychain"),
                Line::Text("Then open Bloop again"),
            ]
        );
    }

    #[test]
    fn the_key_line_follows_the_missing_permissions() {
        assert_eq!(
            status(
                &[Permission::Accessibility],
                false,
                Some(SecretError::Unavailable)
            ),
            [
                Line::Settings(Permission::Accessibility),
                Line::Text("Allow Bloop to read the API key in Keychain"),
                Line::Text("Then open Bloop again"),
            ]
        );
    }
}
