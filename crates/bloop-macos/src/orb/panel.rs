//! `NSPanel` of the orb: configuration and position.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSPanel, NSScreen, NSStatusWindowLevel, NSView,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect};
use objc2_quartz_core::CALayer;

use super::layers::PILL;

const BOTTOM_MARGIN: CGFloat = 16.0;

/// Creates the hidden panel that shows `root`.
///
/// The panel is borderless and nonactivating. Thus it never becomes the key window or the main window.
pub(super) fn new(mtm: MainThreadMarker, root: &CALayer) -> Retained<NSPanel> {
    let frame = CGRect::new(CGPoint::new(0.0, 0.0), PILL);
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setIgnoresMouseEvents(true);
    panel.setHidesOnDeactivate(false);
    panel.setLevel(NSStatusWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel.setAnimationBehavior(NSWindowAnimationBehavior::None);

    let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
    view.setLayer(Some(root));
    view.setWantsLayer(true);
    panel.setContentView(Some(&view));
    panel
}

/// Moves the panel to the bottom center of the screen that contains the mouse pointer,
/// inside the visible frame. Returns the scale factor of that screen.
pub(super) fn place(panel: &NSPanel, mtm: MainThreadMarker) -> Option<CGFloat> {
    let mouse = NSEvent::mouseLocation();
    let screen = NSScreen::screens(mtm)
        .iter()
        .find(|screen| contains(screen.frame(), mouse))
        .or_else(|| NSScreen::mainScreen(mtm))?;
    let visible = screen.visibleFrame();
    panel.setFrameOrigin(CGPoint::new(
        visible.mid().x - PILL.width / 2.0,
        visible.min().y + BOTTOM_MARGIN,
    ));
    Some(screen.backingScaleFactor())
}

fn contains(frame: CGRect, point: CGPoint) -> bool {
    let (min, max) = (frame.min(), frame.max());
    (min.x..=max.x).contains(&point.x) && (min.y..=max.y).contains(&point.y)
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::contains;

    #[test]
    fn a_screen_contains_the_points_on_its_edges() {
        let frame = CGRect::new(CGPoint::new(-1512.0, 0.0), CGSize::new(1512.0, 982.0));

        assert!(contains(frame, CGPoint::new(-1512.0, 0.0)));
        assert!(contains(frame, CGPoint::new(0.0, 982.0)));
        assert!(!contains(frame, CGPoint::new(0.5, 10.0)));
    }
}
