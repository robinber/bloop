//! Status item and menu. Each time the menu opens, it shows the model of a `MenuSource`, then "Quit Bloop".

#![expect(
    unsafe_code,
    reason = "a menu item action is an Objective-C selector that the compiler cannot check, its target is an Objective-C class, and the menu delegate implements an Objective-C protocol"
)]

use std::fmt;

use bloop_core::ports::platform::{ActionId, MenuItem, MenuSource};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    AnyThread as _, DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    sel,
};
use objc2_app_kit::{
    NSControlStateValue, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu,
    NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSString, ns_string};

/// The Bloop item in the menu bar. The item stays in the menu bar while this value exists.
///
/// The menu delegate and the item targets are weak references to the target, which this value keeps.
#[derive(Debug)]
pub struct StatusMenu {
    _item: Retained<NSStatusItem>,
    _target: Retained<MenuTarget>,
}

impl StatusMenu {
    /// Adds the Bloop item to the menu bar.
    ///
    /// Each time the menu opens, it shows `source.model()`, then a separator and "Quit Bloop".
    /// A click on an item gives its action to `source.activate`.
    #[must_use]
    pub fn new(mtm: MainThreadMarker, source: Box<dyn MenuSource>) -> Self {
        let target = MenuTarget::new(mtm, Source(source));
        let menu = NSMenu::new(mtm);
        target.render(&menu);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));

        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        item.setMenu(Some(&menu));
        if let Some(button) = item.button(mtm) {
            match status_icon() {
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

const STATUS_ICON_SVG: &[u8] = include_bytes!("../../../assets/branding/status-icon.svg");

/// Returns the Bloop logo as a template image. `AppKit` draws it in the color of the menu bar.
/// The SVG file gives the size in points.
fn status_icon() -> Option<Retained<NSImage>> {
    let icon = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(STATUS_ICON_SVG))?;
    icon.setTemplate(true);
    icon.setAccessibilityDescription(Some(ns_string!("Bloop")));
    Some(icon)
}

/// Returns the tag of the menu item for `action`: the action plus 1. Thus the default tag 0 never starts an action.
/// Returns `None` when the tag does not fit in `isize`.
fn tag_of(action: ActionId) -> Option<isize> {
    isize::try_from(action.0).ok()?.checked_add(1)
}

/// Returns the action of the menu item tag `tag`. Returns `None` for the tag 0 and for a negative tag.
fn action_of(tag: isize) -> Option<ActionId> {
    let index = tag.checked_sub(1)?;
    usize::try_from(index).ok().map(ActionId)
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::alloc(mtm);
    // SAFETY: each action is a selector from `sel!`. `menuAction:` goes to a `MenuTarget`, which
    // implements it. `stop:` has no target, thus AppKit sends it through the responder chain
    // to `NSApplication`, which implements it.
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

fn check(on: bool) -> NSControlStateValue {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

/// The source of the menu content. `Debug` shows nothing of the source.
struct Source(Box<dyn MenuSource>);

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source").finish_non_exhaustive()
    }
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `MenuTarget` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BloopMenuTarget"]
    #[ivars = Source]
    #[derive(Debug)]
    struct MenuTarget;

    impl MenuTarget {
        /// Gives the action of the clicked item to the source. A tag without an action logs an error.
        #[unsafe(method(menuAction:))]
        fn menu_action(&self, sender: Option<&AnyObject>) {
            let Some(item) = sender.and_then(AnyObject::downcast_ref::<NSMenuItem>) else {
                tracing::error!("a menu action has no menu item, the click has no effect");
                return;
            };
            let tag = item.tag();
            let Some(action) = action_of(tag) else {
                tracing::error!(tag, "the menu item has no action, the click has no effect");
                return;
            };
            self.ivars().0.activate(action);
        }
    }

    // SAFETY: `NSObject` implements all methods of `NSObjectProtocol`.
    unsafe impl NSObjectProtocol for MenuTarget {}

    // SAFETY: all methods of the protocol are optional. The selector, the argument type, and the
    // empty result of `menuNeedsUpdate:` match the `objc2-app-kit` declaration.
    unsafe impl NSMenuDelegate for MenuTarget {
        /// Shows the current model before the menu opens.
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.render(menu);
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, source: Source) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(source);
        // SAFETY: the `init` method of `NSObject` has no requirements.
        unsafe { msg_send![super(this), init] }
    }

    /// Replaces the items of `menu` with the model of the source, a separator, and "Quit Bloop".
    fn render(&self, menu: &NSMenu) {
        let mtm = self.mtm();
        let model = self.ivars().0.model();
        menu.removeAllItems();
        for item in &model.items {
            menu.addItem(&self.item(mtm, item));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, "Quit Bloop", Some(sel!(stop:)), "q"));
    }

    fn item(&self, mtm: MainThreadMarker, item: &MenuItem) -> Retained<NSMenuItem> {
        match item {
            MenuItem::Status { title } => menu_item(mtm, title, None, ""),
            MenuItem::Action { title, action } => self.action_item(mtm, title, *action),
            MenuItem::Toggle {
                title,
                action,
                checked,
            } => {
                let item = self.action_item(mtm, title, *action);
                item.setState(check(*checked));
                item
            }
            MenuItem::Submenu {
                title,
                items,
                checked,
            } => {
                let submenu = NSMenu::new(mtm);
                for child in items {
                    submenu.addItem(&self.item(mtm, child));
                }
                let item = menu_item(mtm, title, None, "");
                item.setSubmenu(Some(&submenu));
                item.setState(check(*checked));
                item
            }
            MenuItem::Separator => NSMenuItem::separatorItem(mtm),
        }
    }

    /// Returns an item that gives `action` to the source. Without a valid tag, the item has no action.
    fn action_item(
        &self,
        mtm: MainThreadMarker,
        title: &str,
        action: ActionId,
    ) -> Retained<NSMenuItem> {
        let Some(tag) = tag_of(action) else {
            tracing::error!(
                ?action,
                "the menu action has no tag, the item has no action"
            );
            return menu_item(mtm, title, None, "");
        };
        let item = menu_item(mtm, title, Some(sel!(menuAction:)), "");
        item.setTag(tag);
        set_target(&item, self);
        item
    }
}

#[cfg(test)]
mod tests {
    use bloop_core::ports::platform::ActionId;

    use super::{action_of, status_icon, tag_of};

    #[test]
    fn the_status_icon_is_a_template_image_of_menu_bar_size() {
        let icon = status_icon().unwrap();
        let size = icon.size();

        assert!(icon.isTemplate());
        assert!((size.width - 13.3).abs() < 1e-6, "{}", size.width);
        assert!((size.height - 16.0).abs() < 1e-6, "{}", size.height);
    }

    #[test]
    fn the_tag_is_the_action_plus_one_and_gives_the_action_back() {
        for index in [0, 1, 41] {
            let tag = tag_of(ActionId(index)).unwrap();

            assert_eq!(tag, isize::try_from(index).unwrap() + 1);
            assert_eq!(action_of(tag), Some(ActionId(index)));
        }
    }

    #[test]
    fn the_tag_0_and_negative_tags_give_no_action() {
        for tag in [0, -1, isize::MIN] {
            assert_eq!(action_of(tag), None, "{tag}");
        }
    }

    #[test]
    fn an_action_without_a_tag_that_fits_gives_no_tag() {
        let last = usize::try_from(isize::MAX).unwrap();

        assert_eq!(tag_of(ActionId(last - 1)), Some(isize::MAX));
        assert_eq!(tag_of(ActionId(last)), None);
        assert_eq!(tag_of(ActionId(usize::MAX)), None);
        assert_eq!(action_of(isize::MAX), Some(ActionId(last - 1)));
    }
}
