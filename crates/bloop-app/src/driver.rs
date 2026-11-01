//! Driver: gives each event to the core and executes the commands through the ports.

use std::time::Duration;

use bloop_core::ports::platform::Scheduler;
use bloop_core::session::{Command, Core, Event};

/// Owns the core and the ports. It operates on the main thread.
pub(crate) struct Driver<S> {
    core: Core,
    scheduler: S,
    hold: Duration,
}

impl<S: Scheduler> Driver<S> {
    /// `hold` is the time that the user holds the dictation key before the capture starts.
    pub(crate) fn new(scheduler: S, hold: Duration) -> Self {
        Self {
            core: Core::new(),
            scheduler,
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
                Command::StartCapture(_)
                | Command::RecordTarget(_)
                | Command::StopCaptureAndProcess(_)
                | Command::CancelCapture
                | Command::CancelWork(_)
                | Command::Insert(..)
                | Command::CopyToClipboard(_)
                | Command::ShowOrb(_)
                | Command::CloseOrb => {}
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

    use bloop_core::ports::platform::Scheduler;
    use bloop_core::session::Event;

    use super::Driver;

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

    fn fire_only_timer(driver: &mut Driver<FakeScheduler>) {
        let timers = driver.scheduler.timers.take();
        let [(delay, event)] = <[_; 1]>::try_from(timers).expect("one timer");
        assert_eq!(delay, HOLD);
        assert_matches!(event, Event::HoldElapsed(_));
        driver.handle(event);
    }

    #[test]
    fn key_press_starts_the_hold_timer() {
        let mut driver = Driver::new(FakeScheduler::default(), HOLD);

        driver.handle(Event::DictationKeyDown);

        assert!(driver.capture_active());
        fire_only_timer(&mut driver);
        assert!(driver.capture_active());
    }

    #[test]
    fn key_release_ends_the_capture() {
        let mut driver = Driver::new(FakeScheduler::default(), HOLD);
        driver.handle(Event::DictationKeyDown);
        fire_only_timer(&mut driver);

        driver.handle(Event::DictationKeyUp);

        assert!(!driver.capture_active());
        assert!(driver.scheduler.timers.borrow().is_empty());
    }

    #[test]
    fn short_press_makes_the_timer_inactive() {
        let mut driver = Driver::new(FakeScheduler::default(), HOLD);
        driver.handle(Event::DictationKeyDown);

        driver.handle(Event::DictationKeyUp);
        fire_only_timer(&mut driver);

        assert!(!driver.capture_active());
    }
}
