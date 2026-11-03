//! `OrbPanel`: the pill at the bottom of the screen that shows the voice level during a capture,
//! and the fallback card with the final text.

mod layers;
mod panel;

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use bloop_core::domain::LevelMeter;
use bloop_core::ports::platform::Overlay;
use bloop_core::session::OrbView;
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSPanel, NSWorkspace};
use objc2_foundation::NSString;

use self::layers::{
    BAR_COUNT, BAR_MIN, BAR_MOVE, BarHeights, Layers, Palette, SHRINK, Shape, TICK,
};
use crate::scheduler;

const ERROR_TIME: Duration = Duration::from_secs(1);
const CARD_TIME: Duration = Duration::from_secs(8);
const MORPH: Duration = Duration::from_millis(200);
const ERROR_LOUDNESS: f64 = 0.6;

/// The orb. It operates on the main thread.
///
/// A timer runs only while the orb is visible.
#[derive(Debug)]
pub struct OrbPanel {
    orb: Rc<RefCell<Orb>>,
    mtm: MainThreadMarker,
}

impl OrbPanel {
    /// Creates the hidden orb. In the listening view, the bars follow the level in `meter`.
    #[must_use]
    pub fn new(mtm: MainThreadMarker, meter: LevelMeter) -> Self {
        let layers = Layers::new();
        let panel = panel::new(mtm, layers.root());
        let orb = Orb {
            panel,
            layers,
            meter,
            bars: BarHeights::default(),
            state: State::default(),
            icon_owner: None,
            mtm,
        };
        Self {
            orb: Rc::new(RefCell::new(orb)),
            mtm,
        }
    }
}

impl Overlay for OrbPanel {
    fn show(&mut self, view: OrbView) {
        run(&self.orb, self.mtm, move |orb| orb.show(&view));
    }

    fn close(&mut self) {
        run(&self.orb, self.mtm, Orb::close);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Hidden,
    Listening,
    Error,
    Card,
    Closing,
}

/// How the orb shows a new view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// The panel is hidden. It shows the view at once.
    Open,
    /// The panel is visible. The pill changes into the view.
    Morph,
}

/// The view of the orb and the epoch of its timers.
///
/// Each view change starts a new epoch. A timer step runs only if its epoch is still current.
#[derive(Debug, Default)]
struct State {
    view: View,
    epoch: u64,
}

impl State {
    /// Changes to `view`. The timers of the previous view become stale.
    fn show(&mut self, view: View) -> Entry {
        self.epoch = self.epoch.wrapping_add(1);
        let entry = if self.view == View::Hidden {
            Entry::Open
        } else {
            Entry::Morph
        };
        self.view = view;
        entry
    }

    /// Starts to close. Returns `false`, and changes nothing, if the orb is hidden or closes already.
    fn close(&mut self) -> bool {
        match self.view {
            View::Hidden | View::Closing => false,
            View::Listening | View::Error | View::Card => {
                self.epoch = self.epoch.wrapping_add(1);
                self.view = View::Closing;
                true
            }
        }
    }

    /// Ends the close: the orb is hidden.
    fn hide(&mut self) {
        self.view = View::Hidden;
    }

    fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Tells if a timer that started in `epoch` is still for the current view.
    fn is_current(&self, epoch: u64) -> bool {
        self.epoch == epoch
    }
}

/// What the orb does after a step.
enum Next {
    Done,
    /// Runs the step after the delay, unless the orb changes its view before.
    After(Duration, fn(&mut Orb) -> Next),
}

#[derive(Debug)]
struct Orb {
    panel: Retained<NSPanel>,
    layers: Layers,
    meter: LevelMeter,
    bars: BarHeights,
    state: State,
    icon_owner: Option<i32>,
    mtm: MainThreadMarker,
}

impl Orb {
    fn show(&mut self, view: &OrbView) -> Next {
        match view {
            OrbView::Listening => {
                self.open(View::Listening, Shape::Pill, Palette::Voice);
                self.bars.reset();
                Next::After(TICK, Self::tick)
            }
            OrbView::Error => {
                self.open(View::Error, Shape::Pill, Palette::Error);
                self.layers
                    .move_bars(&layers::resting_heights(ERROR_LOUDNESS));
                Next::After(ERROR_TIME, Self::close)
            }
            OrbView::Card(text) => {
                let text = NSString::from_str(&layers::excerpt(text.as_str()));
                self.open(View::Card, Shape::Card(&text), Palette::Voice);
                Next::After(CARD_TIME, Self::close)
            }
        }
    }

    /// Shows the panel if it is hidden, then gives the pill the shape of `view`.
    fn open(&mut self, view: View, shape: Shape<'_>, palette: Palette) {
        match self.state.show(view) {
            Entry::Open => {
                if let Some(scale) = panel::place(&self.panel, self.mtm) {
                    self.layers.set_scale(scale);
                    self.icon_owner = None;
                }
                self.layers.reset_bars();
                self.layers.set_shape(shape, None);
                self.panel.orderFrontRegardless();
            }
            Entry::Morph => self.layers.set_shape(shape, Some(MORPH)),
        }
        self.layers.set_palette(palette);
        self.update_icon();
        self.layers.open();
    }

    fn close(&mut self) -> Next {
        if !self.state.close() {
            return Next::Done;
        }
        self.layers.move_bars(&[BAR_MIN; BAR_COUNT]);
        Next::After(BAR_MOVE, Self::shrink)
    }

    fn shrink(&mut self) -> Next {
        self.layers.shrink();
        Next::After(SHRINK, Self::hide)
    }

    fn hide(&mut self) -> Next {
        self.panel.orderOut(None);
        self.state.hide();
        Next::Done
    }

    fn tick(&mut self) -> Next {
        self.update_icon();
        let heights = self.bars.next(self.meter.get());
        self.layers.move_bars(&heights);
        Next::After(TICK, Self::tick)
    }

    /// Shows the icon of the frontmost application. The icon changes when the user activates another application.
    fn update_icon(&mut self) {
        let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return;
        };
        let owner = app.processIdentifier();
        if self.icon_owner != Some(owner) {
            self.icon_owner = Some(owner);
            self.layers.set_icon(app.icon().as_deref());
        }
    }
}

/// A weak reference to the orb, with the marker that a block on the main queue needs to use it.
type WeakOrb = MainThreadBound<(Weak<RefCell<Orb>>, MainThreadMarker)>;

/// Applies `step` to the orb, then schedules the next step that it returns.
///
/// If a step is running, `step` goes to the end of the main queue. Thus a step never runs
/// inside another step, and the orb loses no step.
fn run(
    shared: &Rc<RefCell<Orb>>,
    mtm: MainThreadMarker,
    step: impl FnOnce(&mut Orb) -> Next + Send + 'static,
) {
    let Ok(mut orb) = shared.try_borrow_mut() else {
        tracing::warn!("the orb is busy, the step goes to the end of the main queue");
        let target = MainThreadBound::new((Rc::downgrade(shared), mtm), mtm);
        DispatchQueue::main().exec_async(move || {
            target.get_on_main(|(orb, mtm)| {
                if let Some(shared) = orb.upgrade() {
                    run(&shared, *mtm, step);
                }
            });
        });
        return;
    };
    let Next::After(delay, step) = step(&mut orb) else {
        return;
    };
    let target = MainThreadBound::new((Rc::downgrade(shared), mtm), mtm);
    schedule(target, orb.state.epoch(), delay, step);
}

/// Runs `step` on the main queue after `delay`, if the orb exists and its epoch is still `epoch`.
fn schedule(target: WeakOrb, epoch: u64, delay: Duration, step: fn(&mut Orb) -> Next) {
    scheduler::run_after(delay, move || {
        target.get_on_main(|(orb, mtm)| {
            let Some(shared) = orb.upgrade() else {
                return;
            };
            let current = shared
                .try_borrow()
                .is_ok_and(|orb| orb.state.is_current(epoch));
            if current {
                run(&shared, *mtm, step);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{Entry, State, View};

    #[test]
    fn the_first_show_opens_the_panel_and_the_next_one_morphs_the_pill() {
        let mut state = State::default();

        assert_eq!(state.show(View::Listening), Entry::Open);
        assert_eq!(state.show(View::Card), Entry::Morph);
    }

    #[test]
    fn a_card_during_the_close_morphs_the_pill_and_stops_the_close() {
        let mut state = State::default();
        state.show(View::Listening);
        assert!(state.close());
        let closing = state.epoch();

        assert_eq!(state.show(View::Card), Entry::Morph);
        assert!(!state.is_current(closing));
    }

    #[test]
    fn a_second_close_changes_nothing() {
        let mut state = State::default();
        state.show(View::Error);
        assert!(state.close());
        let closing = state.epoch();

        assert!(!state.close());
        assert!(state.is_current(closing));
    }

    #[test]
    fn a_capture_during_the_card_stops_the_card_timer() {
        let mut state = State::default();
        state.show(View::Card);
        let card = state.epoch();

        assert_eq!(state.show(View::Listening), Entry::Morph);
        assert!(!state.is_current(card));
    }

    #[test]
    fn a_close_while_hidden_changes_nothing() {
        let mut state = State::default();
        let hidden = state.epoch();

        assert!(!state.close());
        assert!(state.is_current(hidden));
        assert_eq!(state.show(View::Listening), Entry::Open);
    }

    #[test]
    fn hide_ends_the_close_and_the_next_show_opens_the_panel() {
        let mut state = State::default();
        state.show(View::Listening);
        assert!(state.close());
        state.hide();

        assert_eq!(state.show(View::Error), Entry::Open);
    }
}
