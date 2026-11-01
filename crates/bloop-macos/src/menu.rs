//! Status item and menu.

#![expect(
    unsafe_code,
    reason = "a menu item action is an Objective-C selector that the compiler cannot check"
)]

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::ns_string;

/// The Bloop item in the menu bar. The item stays in the menu bar while this value exists.
#[derive(Debug)]
pub struct StatusMenu {
    _item: Retained<NSStatusItem>,
}

impl StatusMenu {
    /// Adds the Bloop item to the menu bar, with a menu that contains "Quit Bloop".
    #[must_use]
    pub fn new(mtm: MainThreadMarker) -> Self {
        let menu = NSMenu::new(mtm);
        menu.addItem(&quit_item(mtm));

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
        Self { _item: item }
    }
}

fn quit_item(mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let title = ns_string!("Quit Bloop");
    let key = ns_string!("q");
    let action = sel!(terminate:);
    let item = NSMenuItem::alloc(mtm);
    // SAFETY: `sel!` gives a valid selector. The item has no target, thus AppKit sends the action
    // through the responder chain to `NSApplication`, which implements `terminate:`.
    unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(item, title, Some(action), key) }
}
