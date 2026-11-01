//! Driver: gives each event to the core and executes the commands through the ports.

use std::collections::VecDeque;
use std::time::Duration;

use bloop_core::domain::FinalText;
use bloop_core::ports::platform::{Overlay, Scheduler, TextOutput};
use bloop_core::session::{Command, Core, Event};

use crate::test_level::TestLevel;

/// The text of each job until Phase 2 adds transcription.
const TEST_TEXT: &str = "Voilà le texte de test de Bloop : é, è, à, ç.";

/// Owns the core and the ports. It operates on the main thread.
pub(crate) struct Driver<S, O, T> {
    core: Core,
    scheduler: S,
    overlay: O,
    output: T,
    test_level: TestLevel,
    hold: Duration,
}

impl<S: Scheduler, O: Overlay, T: TextOutput> Driver<S, O, T> {
    /// `hold` is the time that the user holds the dictation key before the capture starts.
    pub(crate) fn new(
        scheduler: S,
        overlay: O,
        output: T,
        test_level: TestLevel,
        hold: Duration,
    ) -> Self {
        Self {
            core: Core::new(),
            scheduler,
            overlay,
            output,
            test_level,
            hold,
        }
    }

    /// Gives the event to the core, then the events that the commands produce, in order.
    pub(crate) fn handle(&mut self, event: Event) {
        let mut events = VecDeque::from([event]);
        while let Some(event) = events.pop_front() {
            tracing::info!(?event, "event");
            for command in self.core.handle(event) {
                tracing::info!(?command, "command");
                events.extend(self.execute(command));
            }
        }
    }

    pub(crate) fn capture_active(&self) -> bool {
        self.core.capture_active()
    }

    fn execute(&mut self, command: Command) -> Option<Event> {
        match command {
            Command::StartHoldTimer(generation) => {
                self.scheduler
                    .after(self.hold, Event::HoldElapsed(generation));
            }
            Command::StartCapture(_) => self.test_level.start(),
            Command::RecordTarget(generation) => {
                self.output.record_target(generation);
            }
            Command::StopCaptureAndProcess(generation) => {
                self.test_level.stop();
                let text = FinalText::new(TEST_TEXT.to_owned());
                return Some(Event::TextReady(generation, text));
            }
            Command::CancelCapture => self.test_level.stop(),
            Command::Insert(generation, text) => self.output.insert(generation, text),
            Command::CopyToClipboard(text) => self.output.copy(&text),
            Command::ShowOrb(view) => self.overlay.show(view),
            Command::CloseOrb => self.overlay.close(),
            Command::CancelWork(generation) => self.output.cancel(generation),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::cell::RefCell;
    use std::time::Duration;

    use bloop_core::domain::{FinalText, Generation, LevelMeter, TargetInfo};
    use bloop_core::ports::platform::{Overlay, Scheduler, TextOutput};
    use bloop_core::session::{Event, InsertOutcome, OrbView};

    use super::{Driver, TEST_TEXT};
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

    #[derive(Default)]
    struct FakeOutput {
        targets: Vec<Generation>,
        inserted: Vec<(Generation, FinalText)>,
        copied: Vec<FinalText>,
        cancelled: Vec<Generation>,
    }

    impl TextOutput for FakeOutput {
        fn record_target(&mut self, generation: Generation) -> TargetInfo {
            self.targets.push(generation);
            TargetInfo::default()
        }

        fn insert(&mut self, generation: Generation, text: FinalText) {
            self.inserted.push((generation, text));
        }

        fn copy(&mut self, text: &FinalText) {
            self.copied.push(text.clone());
        }

        fn cancel(&mut self, generation: Generation) {
            self.cancelled.push(generation);
        }
    }

    type TestDriver = Driver<FakeScheduler, FakeOverlay, FakeOutput>;

    fn driver() -> TestDriver {
        Driver::new(
            FakeScheduler::default(),
            FakeOverlay::default(),
            FakeOutput::default(),
            TestLevel::new(LevelMeter::default()),
            HOLD,
        )
    }

    fn test_text() -> FinalText {
        FinalText::new(TEST_TEXT.to_owned())
    }

    fn fire_only_timer(driver: &mut TestDriver) {
        let timers = driver.scheduler.timers.take();
        let [(delay, event)] = <[_; 1]>::try_from(timers).expect("one timer");
        assert_eq!(delay, HOLD);
        assert_matches!(event, Event::HoldElapsed(_));
        driver.handle(event);
    }

    fn dictate(driver: &mut TestDriver) -> Generation {
        driver.handle(Event::DictationKeyDown);
        fire_only_timer(driver);
        driver.handle(Event::DictationKeyUp);
        *driver.output.targets.last().expect("a recorded target")
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
        assert_eq!(driver.output.targets, [] as [Generation; 0]);
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
        assert_eq!(driver.output.inserted, []);
    }

    #[test]
    fn key_release_pastes_the_test_text_for_the_recorded_target() {
        let mut driver = driver();

        let generation = dictate(&mut driver);

        assert_eq!(driver.output.targets, [generation]);
        assert_eq!(driver.output.inserted, [(generation, test_text())]);
    }

    #[test]
    fn a_finished_paste_ends_the_job() {
        let mut driver = driver();

        for _ in 0..4 {
            let generation = dictate(&mut driver);
            driver.handle(Event::InsertDone(generation, InsertOutcome::Landed));
        }

        assert_eq!(driver.output.inserted.len(), 4);
        assert!(
            !driver
                .overlay
                .calls
                .contains(&OrbCall::Show(OrbView::Error))
        );
    }

    #[test]
    fn copy_last_result_copies_the_pasted_text() {
        let mut driver = driver();
        let generation = dictate(&mut driver);
        driver.handle(Event::InsertDone(generation, InsertOutcome::Landed));

        driver.handle(Event::CopyLastResult);

        assert_eq!(driver.output.copied, [test_text()]);
    }

    #[test]
    fn sleep_during_the_paste_cancels_the_insertion() {
        let mut driver = driver();
        let generation = dictate(&mut driver);

        driver.handle(Event::SystemSleep);

        assert_eq!(driver.output.cancelled, [generation]);
        driver.handle(Event::InsertDone(generation, InsertOutcome::Landed));
        assert_eq!(driver.output.inserted.len(), 1);
    }

    #[test]
    fn the_next_paste_waits_for_the_end_of_the_previous_one() {
        let mut driver = driver();
        let first = dictate(&mut driver);
        let second = dictate(&mut driver);

        assert_eq!(driver.output.inserted, [(first, test_text())]);

        driver.handle(Event::InsertDone(first, InsertOutcome::Landed));

        assert_eq!(
            driver.output.inserted,
            [(first, test_text()), (second, test_text())]
        );
    }
}
