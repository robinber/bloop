//! `OrbPanel`: the pill at the bottom of the screen that shows the voice level during a capture.

mod layers;
mod panel;

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use bloop_core::domain::LevelMeter;
use bloop_core::ports::platform::Overlay;
use bloop_core::session::OrbView;
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSPanel, NSWorkspace};

use self::layers::{BAR_COUNT, BAR_MIN, BAR_MOVE, BarHeights, Layers, Palette, SHRINK};

const TICK: Duration = Duration::from_millis(33);
const ERROR_TIME: Duration = Duration::from_secs(1);
const ERROR_LOUDNESS: f64 = 0.6;

/// The orb. It operates on the main thread.
///
/// A timer runs only while the orb is visible.
#[derive(Debug)]
pub struct OrbPanel {
    orb: Rc<RefCell<Orb>>,
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
            view: View::Hidden,
            epoch: 0,
            icon_owner: None,
            mtm,
        };
        Self {
            orb: Rc::new(RefCell::new(orb)),
        }
    }
}

impl Overlay for OrbPanel {
    fn show(&mut self, view: OrbView) {
        run(&self.orb, |orb| orb.show(&view));
    }

    fn close(&mut self) {
        run(&self.orb, Orb::close);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Hidden,
    Listening,
    Error,
    Closing,
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
    view: View,
    epoch: u64,
    icon_owner: Option<i32>,
    mtm: MainThreadMarker,
}

impl Orb {
    fn show(&mut self, view: &OrbView) -> Next {
        match view {
            OrbView::Listening => {
                self.open(View::Listening, Palette::Voice);
                self.bars.reset();
                Next::After(TICK, Self::tick)
            }
            OrbView::Error => {
                self.open(View::Error, Palette::Error);
                self.layers
                    .move_bars(&layers::resting_heights(ERROR_LOUDNESS));
                Next::After(ERROR_TIME, Self::close)
            }
            OrbView::Card(_) => {
                tracing::warn!("the orb cannot show the fallback card yet");
                Next::Done
            }
        }
    }

    fn open(&mut self, view: View, palette: Palette) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.view == View::Hidden {
            if let Some(scale) = panel::place(&self.panel, self.mtm) {
                self.layers.set_scale(scale);
                self.icon_owner = None;
            }
            self.layers.reset_bars();
            self.panel.orderFrontRegardless();
        }
        self.view = view;
        self.layers.set_palette(palette);
        self.update_icon();
        self.layers.open();
    }

    fn close(&mut self) -> Next {
        match self.view {
            View::Hidden | View::Closing => Next::Done,
            View::Listening | View::Error => {
                self.epoch = self.epoch.wrapping_add(1);
                self.view = View::Closing;
                self.layers.move_bars(&[BAR_MIN; BAR_COUNT]);
                Next::After(BAR_MOVE, Self::shrink)
            }
        }
    }

    fn shrink(&mut self) -> Next {
        self.layers.shrink();
        Next::After(SHRINK, Self::hide)
    }

    fn hide(&mut self) -> Next {
        self.panel.orderOut(None);
        self.view = View::Hidden;
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

/// Applies `step` to the orb, then schedules the next step that it returns.
fn run(shared: &Rc<RefCell<Orb>>, step: impl FnOnce(&mut Orb) -> Next) {
    let Ok(mut orb) = shared.try_borrow_mut() else {
        tracing::error!("the orb is busy");
        return;
    };
    let Next::After(delay, step) = step(&mut orb) else {
        return;
    };
    let target = MainThreadBound::new(Rc::downgrade(shared), orb.mtm);
    schedule(target, orb.epoch, delay, step);
}

/// Runs `step` on the main queue after `delay`, if the orb exists and its epoch is still `epoch`.
fn schedule(
    target: MainThreadBound<Weak<RefCell<Orb>>>,
    epoch: u64,
    delay: Duration,
    step: fn(&mut Orb) -> Next,
) {
    let Ok(when) = DispatchTime::try_from(delay) else {
        tracing::error!(?delay, "the delay is too long for a timer");
        return;
    };
    let queued = DispatchQueue::main().after(when, move || {
        target.get_on_main(|orb| {
            let Some(shared) = orb.upgrade() else {
                return;
            };
            let current = shared.try_borrow().is_ok_and(|orb| orb.epoch == epoch);
            if current {
                run(&shared, step);
            }
        });
    });
    if let Err(error) = queued {
        tracing::error!(?error, "the orb timer did not start");
    }
}
