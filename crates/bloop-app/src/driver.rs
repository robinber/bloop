//! Driver: gives each event to the core and executes the commands through the ports.

use std::time::Duration;

use bloop_core::ports::platform::{Overlay, Scheduler};
use bloop_core::session::{Command, Core, Event};

use crate::test_level::TestLevel;

/// Owns the core and the ports. It operates on the main thread.
pub(crate) struct Driver<S, O> {
    core: Core,
    scheduler: S,
    overlay: O,
    test_level: TestLevel,
    hold: Duration,
}

impl<S: Scheduler, O: Overlay> Driver<S, O> {
    /// `hold` is the time that the user holds the dictation key before the capture starts.
    pub(crate) fn new(scheduler: S, overlay: O, test_level: TestLevel, hold: Duration) -> Self {
        Self {
            core: Core::new(),
            scheduler,
            overlay,
            test_level,
            hold,
        }
    }

    pub(crate) fn handle(&mut self, event: Event) {
        tracing::info!(?event, "event");
        for command in self.core.handle(event) {
            tracing::info!(?command, "command");
            match command {
                Command::StartHoldTimer(generation) => {
                    self.scheduler
                        .after(self.hold, Event::HoldElapsed(generation));
                }
                Command::StartCapture(_) => self.test_level.start(),
                Command::StopCaptureAndProcess(_) | Command::CancelCapture => {
                    self.test_level.stop();
                }
                Command::ShowOrb(view) => self.overlay.show(view),
                Command::CloseOrb => self.overlay.close(),
                Command::RecordTarget(_)
                | Command::CancelWork(_)
                | Command::Insert(..)
                | Command::CopyToClipboard(_) => {}
            }
        }
    }

    pub(crate) fn capture_active(&self) -> bool {
        self.core.capture_active()
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::cell::RefCell;
    use std::time::Duration;

    use bloop_core::domain::LevelMeter;
    use bloop_core::ports::platform::{Overlay, Scheduler};
    use bloop_core::session::{Event, OrbView};

    use super::Driver;
    use crate::test_level::TestLevel;

    const HOLD: Duration = Duration::from_millis(220);

    #[derive(Default)]
    struct FakeScheduler {
        timers: RefCell<Vec<(Duration, Event)>>,
    }

    impl Scheduler for FakeScheduler {
        fn after(&self, delay: Duration, event: Event) {
            self.timers.borrow_mut().push((delay, event));
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum OrbCall {
        Show(OrbView),
        Close,
    }

    #[derive(Default)]
    struct FakeOverlay {
        calls: Vec<OrbCall>,
    }

    impl Overlay for FakeOverlay {
        fn show(&mut self, view: OrbView) {
            self.calls.push(OrbCall::Show(view));
        }

        fn close(&mut self) {
            self.calls.push(OrbCall::Close);
        }
    }

    fn driver() -> Driver<FakeScheduler, FakeOverlay> {
        Driver::new(
            FakeScheduler::default(),
            FakeOverlay::default(),
            TestLevel::new(LevelMeter::default()),
            HOLD,
        )
    }

    fn fire_only_timer(driver: &mut Driver<FakeScheduler, FakeOverlay>) {
        let timers = driver.scheduler.timers.take();
        let [(delay, event)] = <[_; 1]>::try_from(timers).expect("one timer");
        assert_eq!(delay, HOLD);
        assert_matches!(event, Event::HoldElapsed(_));
        driver.handle(event);
    }

    #[test]
    fn key_press_starts_the_hold_timer() {
        let mut driver = driver();

        driver.handle(Event::DictationKeyDown);

        assert!(driver.capture_active());
        assert_eq!(driver.overlay.calls, [] as [OrbCall; 0]);
        fire_only_timer(&mut driver);
        assert!(driver.capture_active());
        assert_eq!(driver.overlay.calls, [OrbCall::Show(OrbView::Listening)]);
    }

    #[test]
    fn key_release_ends_the_capture() {
        let mut driver = driver();
        driver.handle(Event::DictationKeyDown);
        fire_only_timer(&mut driver);

        driver.handle(Event::DictationKeyUp);

        assert!(!driver.capture_active());
        assert!(driver.scheduler.timers.borrow().is_empty());
        assert_eq!(
            driver.overlay.calls,
            [OrbCall::Show(OrbView::Listening), OrbCall::Close]
        );
    }

    #[test]
    fn short_press_makes_the_timer_inactive() {
        let mut driver = driver();
        driver.handle(Event::DictationKeyDown);

        driver.handle(Event::DictationKeyUp);
        fire_only_timer(&mut driver);

        assert!(!driver.capture_active());
        assert_eq!(driver.overlay.calls, [] as [OrbCall; 0]);
    }

    #[test]
    fn escape_closes_the_orb() {
        let mut driver = driver();
        driver.handle(Event::DictationKeyDown);
        fire_only_timer(&mut driver);

        driver.handle(Event::Escape);

        assert!(!driver.capture_active());
        assert_eq!(
            driver.overlay.calls,
            [OrbCall::Show(OrbView::Listening), OrbCall::Close]
        );
    }
}
