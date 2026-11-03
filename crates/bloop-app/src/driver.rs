//! Driver: gives each event to the core and executes the commands through the ports.

use std::collections::VecDeque;
use std::time::Duration;

use bloop_audio::buffer::{self, Reader};
use bloop_core::domain::{FinalText, Generation};
use bloop_core::ports::platform::{AudioInput, Overlay, Scheduler, TextOutput};
use bloop_core::session::{Command, Core, Event};

/// The text of each job until Phase 2 adds transcription.
const TEST_TEXT: &str = "Voilà le texte de test de Bloop : é, è, à, ç.";

/// The capacity of the audio queue of each capture, in samples: 20 s at 48 kHz.
const CAPACITY: usize = 960_000;

/// Owns the core and the ports. It operates on the main thread.
pub(crate) struct Driver<S, O, T, A> {
    core: Core,
    scheduler: S,
    overlay: O,
    output: T,
    input: A,
    active: Option<Active>,
    hold: Duration,
}

/// The capture that the microphone fills.
struct Active {
    generation: Generation,
    reader: Reader,
}

impl<S: Scheduler, O: Overlay, T: TextOutput, A: AudioInput> Driver<S, O, T, A> {
    /// `hold` is the time that the user holds the dictation key before the capture starts.
    pub(crate) fn new(scheduler: S, overlay: O, output: T, input: A, hold: Duration) -> Self {
        Self {
            core: Core::new(),
            scheduler,
            overlay,
            output,
            input,
            active: None,
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
            Command::StartCapture(generation) => return self.start_capture(generation),
            Command::RecordTarget(generation) => {
                self.output.record_target(generation);
            }
            Command::StopCaptureAndProcess(generation) => {
                return Some(self.stop_capture(generation));
            }
            Command::CancelCapture => {
                self.active = None;
                self.input.cancel();
            }
            Command::Insert(generation, text) => self.output.insert(generation, text),
            Command::CopyToClipboard(text) => self.output.copy(&text),
            Command::ShowOrb(view) => self.overlay.show(view),
            Command::CloseOrb => self.overlay.close(),
            Command::CancelWork(generation) => self.output.cancel(generation),
        }
        None
    }

    fn start_capture(&mut self, generation: Generation) -> Option<Event> {
        let (writer, reader) = buffer::new(CAPACITY);
        match self.input.start(generation, Box::new(writer)) {
            Ok(_) => {
                self.active = Some(Active { generation, reader });
                None
            }
            Err(error) => Some(Event::CaptureFailed(generation, error)),
        }
    }

    fn stop_capture(&mut self, generation: Generation) -> Event {
        let active = self
            .active
            .take()
            .filter(|active| active.generation == generation);
        if let Some(active) = &active {
            active.reader.end_mark().set();
        }
        let summary = match self.input.stop(generation) {
            Ok(summary) => summary,
            Err(error) => return Event::CaptureFailed(generation, error),
        };
        tracing::info!(duration = ?summary.duration, peak = summary.peak, "the capture ended");
        if let Some(error) = active.and_then(|active| active.reader.failure()) {
            tracing::warn!(?error, "the audio queue failed");
        }
        let text = FinalText::new(TEST_TEXT.to_owned());
        Event::TextReady(generation, text)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::cell::RefCell;
    use std::num::NonZeroU32;
    use std::time::Duration;

    use bloop_core::domain::{CaptureSummary, FinalText, Generation, TargetInfo};
    use bloop_core::error::AppError;
    use bloop_core::ports::platform::{AudioInput, AudioSink, Overlay, Scheduler, TextOutput};
    use bloop_core::session::{Event, InsertOutcome, OrbView};

    use super::{Driver, TEST_TEXT};

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

    #[derive(Debug, PartialEq, Eq)]
    enum InputCall {
        Start(Generation),
        Stop(Generation),
        Cancel,
    }

    #[derive(Default)]
    struct FakeInput {
        calls: Vec<InputCall>,
        start_error: Option<AppError>,
        stop_error: Option<AppError>,
    }

    impl AudioInput for FakeInput {
        fn start(
            &mut self,
            generation: Generation,
            _sink: Box<dyn AudioSink>,
        ) -> Result<NonZeroU32, AppError> {
            self.calls.push(InputCall::Start(generation));
            self.start_error.map_or(Ok(NonZeroU32::MIN), Err)
        }

        fn stop(&mut self, generation: Generation) -> Result<CaptureSummary, AppError> {
            self.calls.push(InputCall::Stop(generation));
            let summary = CaptureSummary {
                duration: Duration::ZERO,
                peak: 0.0,
            };
            self.stop_error.map_or(Ok(summary), Err)
        }

        fn cancel(&mut self) {
            self.calls.push(InputCall::Cancel);
        }
    }

    type TestDriver = Driver<FakeScheduler, FakeOverlay, FakeOutput, FakeInput>;

    fn driver() -> TestDriver {
        Driver::new(
            FakeScheduler::default(),
            FakeOverlay::default(),
            FakeOutput::default(),
            FakeInput::default(),
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
        let generation = driver.output.targets[0];
        assert_eq!(
            driver.input.calls,
            [InputCall::Start(generation), InputCall::Stop(generation)]
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
        assert_matches!(
            driver.input.calls[..],
            [InputCall::Start(_), InputCall::Cancel]
        );
    }

    #[test]
    fn a_failed_start_fails_the_capture() {
        let mut driver = driver();
        driver.input.start_error = Some(AppError::Microphone);
        driver.handle(Event::DictationKeyDown);

        fire_only_timer(&mut driver);
        driver.handle(Event::DictationKeyUp);

        assert!(!driver.capture_active());
        assert_eq!(
            driver.overlay.calls,
            [
                OrbCall::Show(OrbView::Listening),
                OrbCall::Show(OrbView::Error)
            ]
        );
        assert_eq!(driver.output.inserted, []);
        assert_matches!(
            driver.input.calls[..],
            [InputCall::Start(_), InputCall::Cancel]
        );
    }

    #[test]
    fn a_failed_stop_removes_the_job_without_a_paste() {
        let mut driver = driver();
        driver.input.stop_error = Some(AppError::Microphone);

        let failed = dictate(&mut driver);

        assert_eq!(driver.output.inserted, []);
        assert_eq!(driver.output.cancelled, [failed]);
        assert_eq!(
            driver.overlay.calls.last(),
            Some(&OrbCall::Show(OrbView::Error))
        );
        driver.input.stop_error = None;
        let next = dictate(&mut driver);
        assert_eq!(driver.output.inserted, [(next, test_text())]);
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
