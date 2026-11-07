//! Driver: gives each event to the core and executes the commands through the ports.

use std::collections::VecDeque;
use std::rc::Rc;

use bloop_audio::buffer;
use bloop_core::domain::Generation;
use bloop_core::error::AppError;
use bloop_core::ports::platform::{AudioInput, Overlay, Scheduler, TextOutput};
use bloop_core::session::{Command, Core, Event};

use crate::hub::Snapshot;
use crate::worker::{Spawner, Task};

/// The capacity of the audio queue of each capture, in samples: 20 s at 48 kHz.
const CAPACITY: usize = 960_000;

/// Gives the settings snapshot of each new generation. It operates on the main thread.
pub(crate) trait Snapshots {
    /// Returns the current settings snapshot.
    fn snapshot(&mut self) -> Rc<Snapshot>;
}

/// Owns the core, the ports, and the transcription tasks. It operates on the main thread.
pub(crate) struct Driver<S, O, T, A, N> {
    core: Core,
    scheduler: S,
    overlay: O,
    output: T,
    input: A,
    spawner: Spawner,
    snapshots: N,
    /// The settings snapshot of the generation whose hold timer runs.
    pending: Option<(Generation, Rc<Snapshot>)>,
    active: Option<Active>,
    jobs: Vec<(Generation, Task)>,
}

/// The capture that the microphone fills, and its transcription task.
struct Active {
    generation: Generation,
    task: Task,
}

impl<S: Scheduler, O: Overlay, T: TextOutput, A: AudioInput, N: Snapshots> Driver<S, O, T, A, N> {
    /// `snapshots` gives the settings of each generation: the hold time and the speech objects.
    pub(crate) fn new(
        scheduler: S,
        overlay: O,
        output: T,
        input: A,
        spawner: Spawner,
        snapshots: N,
    ) -> Self {
        Self {
            core: Core::new(),
            scheduler,
            overlay,
            output,
            input,
            spawner,
            snapshots,
            pending: None,
            active: None,
            jobs: Vec::new(),
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
                let snapshot = self.snapshots.snapshot();
                self.scheduler
                    .after(snapshot.hold, Event::HoldElapsed(generation));
                self.pending = Some((generation, snapshot));
            }
            Command::StartCapture(generation) => return self.start_capture(generation),
            Command::RecordTarget(generation) => {
                self.output.record_target(generation);
            }
            Command::StopCaptureAndProcess(generation) => return self.stop_capture(generation),
            Command::CancelCapture => {
                self.active = None;
                self.input.cancel();
            }
            Command::Insert(generation, text) => {
                self.forget(generation);
                self.output.insert(generation, text);
            }
            Command::CopyToClipboard(text) => self.output.copy(&text),
            Command::ShowOrb(view) => self.overlay.show(view),
            Command::CloseOrb => self.overlay.close(),
            Command::CancelWork(generation) => {
                self.forget(generation);
                self.output.cancel(generation);
            }
        }
        None
    }

    /// Starts the microphone and the transcription task with the settings snapshot of `generation`.
    fn start_capture(&mut self, generation: Generation) -> Option<Event> {
        let Some((_, snapshot)) = self
            .pending
            .take()
            .filter(|(pending, _)| *pending == generation)
        else {
            tracing::error!(?generation, "the generation has no settings snapshot");
            return Some(Event::CaptureFailed(generation, AppError::Internal));
        };
        let (writer, reader) = buffer::new(CAPACITY);
        let task = self
            .input
            .start(generation, Box::new(writer))
            .and_then(|rate| {
                self.spawner
                    .spawn(generation, rate, reader, &snapshot.speech)
            });
        match task {
            Ok(task) => {
                self.active = Some(Active { generation, task });
                None
            }
            Err(error) => Some(Event::CaptureFailed(generation, error)),
        }
    }

    fn stop_capture(&mut self, generation: Generation) -> Option<Event> {
        let task = self
            .active
            .take()
            .filter(|active| active.generation == generation)
            .map(|active| active.task);
        if let Some(task) = &task {
            task.end_capture();
        }
        let summary = match self.input.stop(generation) {
            Ok(summary) => summary,
            Err(error) => return Some(Event::CaptureFailed(generation, error)),
        };
        tracing::info!(duration = ?summary.duration, peak = summary.peak, "the capture ended");
        let Some(task) = task else {
            return Some(Event::CaptureFailed(generation, AppError::Internal));
        };
        self.jobs.push((generation, task));
        None
    }

    /// Drops the transcription task of the job, which stops it if it still runs.
    fn forget(&mut self, generation: Generation) {
        self.jobs.retain(|(job, _)| *job != generation);
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::cell::RefCell;
    use std::num::NonZeroU32;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use bloop_core::domain::{CaptureSummary, FinalText, Generation, TargetInfo};
    use bloop_core::error::{AppError, ProviderError};
    use bloop_core::ports::platform::{AudioInput, AudioSink, Overlay, Scheduler, TextOutput};
    use bloop_core::session::{Command, Core, Event, InsertOutcome, OrbView};
    use tokio::time::sleep;

    use super::{Driver, Snapshots};
    use crate::hub::Snapshot;
    use crate::worker::Speech;
    use crate::worker::fakes::{
        Answer, CORRECTED, Call, Fake, LONG, RATE, Reply, Script, TEXT, pcm, samples, sent_audio,
        settle,
    };

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

    /// Keeps the sink of the capture until `stop` or `cancel`, as the microphone does.
    #[derive(Default)]
    struct FakeInput {
        calls: Vec<InputCall>,
        sink: Option<Box<dyn AudioSink>>,
        start_error: Option<AppError>,
        stop_error: Option<AppError>,
    }

    impl AudioInput for FakeInput {
        fn start(
            &mut self,
            generation: Generation,
            sink: Box<dyn AudioSink>,
        ) -> Result<NonZeroU32, AppError> {
            self.calls.push(InputCall::Start(generation));
            if let Some(error) = self.start_error {
                return Err(error);
            }
            self.sink = Some(sink);
            Ok(RATE)
        }

        fn stop(&mut self, generation: Generation) -> Result<CaptureSummary, AppError> {
            self.calls.push(InputCall::Stop(generation));
            self.sink = None;
            let summary = CaptureSummary {
                duration: Duration::ZERO,
                peak: 0.0,
            };
            self.stop_error.map_or(Ok(summary), Err)
        }

        fn cancel(&mut self) {
            self.calls.push(InputCall::Cancel);
            self.sink = None;
        }
    }

    /// Gives `next` to each new generation. A test replaces it to change the settings.
    struct FakeSnapshots {
        next: Rc<Snapshot>,
    }

    impl Snapshots for FakeSnapshots {
        fn snapshot(&mut self) -> Rc<Snapshot> {
            Rc::clone(&self.next)
        }
    }

    type TestDriver = Driver<FakeScheduler, FakeOverlay, FakeOutput, FakeInput, FakeSnapshots>;

    /// Returns a driver whose transcription tasks run on the current runtime, with their fake provider.
    /// Its settings have the hold time `HOLD` and the correction off.
    fn driver() -> (TestDriver, Fake) {
        let fake = Fake::default();
        let snapshots = FakeSnapshots {
            next: snapshot(HOLD, Ok(fake.speech(Script::default(), LONG))),
        };
        let driver = Driver::new(
            FakeScheduler::default(),
            FakeOverlay::default(),
            FakeOutput::default(),
            FakeInput::default(),
            fake.spawner(),
            snapshots,
        );
        (driver, fake)
    }

    fn snapshot(hold: Duration, speech: Result<Speech, AppError>) -> Rc<Snapshot> {
        Rc::new(Snapshot {
            hold,
            speech: speech.map(Arc::new),
        })
    }

    /// Gives `speech` and the hold time `HOLD` to the next generations.
    fn set_speech(driver: &mut TestDriver, speech: Result<Speech, AppError>) {
        driver.snapshots.next = snapshot(HOLD, speech);
    }

    fn transcript() -> FinalText {
        FinalText::new(TEXT.to_owned())
    }

    fn fire_only_timer(driver: &mut TestDriver) {
        let timers = driver.scheduler.timers.take();
        let [(delay, event)] = <[_; 1]>::try_from(timers).expect("one timer");
        assert_eq!(delay, HOLD);
        assert_matches!(event, Event::HoldElapsed(_));
        driver.handle(event);
    }

    /// Runs the transcription tasks until they wait, then gives their events to the driver.
    async fn deliver(driver: &mut TestDriver, fake: &Fake) {
        settle().await;
        for event in fake.events.take() {
            driver.handle(event);
        }
    }

    /// Holds the dictation key past the threshold and releases it. Returns the generation of the new job.
    fn release(driver: &mut TestDriver) -> Generation {
        driver.handle(Event::DictationKeyDown);
        fire_only_timer(driver);
        driver.handle(Event::DictationKeyUp);
        *driver.output.targets.last().expect("a recorded target")
    }

    async fn dictate(driver: &mut TestDriver, fake: &Fake) -> Generation {
        let generation = release(driver);
        deliver(driver, fake).await;
        generation
    }

    /// Runs the tasks until the correction request of the job waits for its answer.
    async fn until_the_correction_waits() {
        sleep(Duration::from_millis(10)).await;
    }

    fn text(text: &str) -> FinalText {
        FinalText::new(text.to_owned())
    }

    #[tokio::test(start_paused = true)]
    async fn key_press_starts_the_hold_timer() {
        let (mut driver, _) = driver();

        driver.handle(Event::DictationKeyDown);

        assert!(driver.capture_active());
        assert_eq!(driver.overlay.calls, [] as [OrbCall; 0]);
        fire_only_timer(&mut driver);
        assert!(driver.capture_active());
        assert_eq!(driver.overlay.calls, [OrbCall::Show(OrbView::Listening)]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_hold_timer_uses_the_hold_time_of_the_snapshot_at_the_key_press() {
        let (mut driver, fake) = driver();
        let hold = Duration::from_millis(400);
        driver.snapshots.next = snapshot(hold, Ok(fake.speech(Script::default(), LONG)));

        driver.handle(Event::DictationKeyDown);
        set_speech(&mut driver, Err(AppError::InvalidSettings));

        let timers = driver.scheduler.timers.take();
        let [(delay, Event::HoldElapsed(generation))] = timers[..] else {
            panic!("{timers:?}");
        };
        assert_eq!(delay, hold);
        driver.handle(Event::HoldElapsed(generation));
        assert_eq!(driver.input.calls, [InputCall::Start(generation)]);
        assert!(driver.active.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_capture_without_its_settings_snapshot_fails_before_the_microphone_starts() {
        let (mut driver, _) = driver();
        let commands = Core::new().handle(Event::DictationKeyDown);
        let [Command::StartHoldTimer(generation)] = commands[..] else {
            panic!("{commands:?}");
        };

        assert_matches!(
            driver.start_capture(generation),
            Some(Event::CaptureFailed(failed, AppError::Internal)) if failed == generation
        );
        assert_eq!(driver.input.calls, [] as [InputCall; 0]);
        assert!(driver.active.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn key_release_ends_the_capture() {
        let (mut driver, _) = driver();
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

    #[tokio::test(start_paused = true)]
    async fn escape_closes_the_orb_and_drops_the_session() {
        let (mut driver, fake) = driver();
        driver.handle(Event::DictationKeyDown);
        fire_only_timer(&mut driver);
        settle().await;

        driver.handle(Event::Escape);
        settle().await;

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
        assert_eq!(fake.log.take(), [Call::Drop]);
        assert!(fake.events.take().is_empty());
        assert!(driver.active.is_none() && driver.jobs.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_start_fails_the_capture() {
        let (mut driver, _) = driver();
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

    #[tokio::test(start_paused = true)]
    async fn a_missing_key_fails_the_capture_and_cancels_the_microphone() {
        let (mut driver, _) = driver();
        set_speech(
            &mut driver,
            Err(AppError::Provider(ProviderError::Unauthorized)),
        );
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
        assert_matches!(
            driver.input.calls[..],
            [InputCall::Start(_), InputCall::Cancel]
        );
        assert!(driver.active.is_none() && driver.jobs.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalid_dictionary_fails_the_capture_and_cancels_the_microphone() {
        let (mut driver, fake) = driver();
        set_speech(&mut driver, Err(AppError::InvalidSettings));
        driver.handle(Event::DictationKeyDown);

        fire_only_timer(&mut driver);
        driver.handle(Event::DictationKeyUp);
        settle().await;

        assert!(!driver.capture_active());
        assert_eq!(
            driver.overlay.calls,
            [
                OrbCall::Show(OrbView::Listening),
                OrbCall::Show(OrbView::Error)
            ]
        );
        assert_matches!(
            driver.input.calls[..],
            [InputCall::Start(_), InputCall::Cancel]
        );
        assert!(driver.active.is_none() && driver.jobs.is_empty());
        assert_eq!(fake.log.take(), [] as [Call; 0]);
        assert!(fake.events.take().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_stop_removes_the_job_without_a_paste() {
        let (mut driver, fake) = driver();
        driver.input.stop_error = Some(AppError::Microphone);

        let failed = dictate(&mut driver, &fake).await;

        assert_eq!(driver.output.inserted, []);
        assert_eq!(driver.output.cancelled, [failed]);
        assert_eq!(
            driver.overlay.calls.last(),
            Some(&OrbCall::Show(OrbView::Error))
        );
        driver.input.stop_error = None;
        let next = dictate(&mut driver, &fake).await;
        assert_eq!(driver.output.inserted, [(next, transcript())]);
    }

    #[tokio::test(start_paused = true)]
    async fn key_release_streams_the_audio_and_pastes_the_transcript_for_the_recorded_target() {
        let (mut driver, fake) = driver();
        let audio = samples(4_000);
        driver.handle(Event::DictationKeyDown);
        fire_only_timer(&mut driver);

        let sink = driver.input.sink.as_mut().expect("a started capture");
        sink.try_push(&audio).unwrap();
        driver.handle(Event::DictationKeyUp);
        deliver(&mut driver, &fake).await;

        assert_eq!(sent_audio(&fake.log.take()), pcm(&audio));
        assert_eq!(fake.rates.take(), [RATE]);
        let generation = driver.output.targets[0];
        assert_eq!(driver.output.targets, [generation]);
        assert_eq!(driver.output.inserted, [(generation, transcript())]);
        assert!(driver.jobs.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_finish_removes_the_job_and_the_next_job_still_pastes() {
        let (mut driver, fake) = driver();
        let failing = Script {
            finish: Reply::Fail(ProviderError::Network),
            ..Script::default()
        };
        set_speech(&mut driver, Ok(fake.speech(failing, LONG)));

        let failed = dictate(&mut driver, &fake).await;

        assert_eq!(fake.log.take(), [Call::Finish, Call::Drop]);
        assert_eq!(driver.output.inserted, []);
        assert_eq!(driver.output.cancelled, [failed]);
        assert_eq!(
            driver.overlay.calls.last(),
            Some(&OrbCall::Show(OrbView::Error))
        );
        assert!(driver.jobs.is_empty());
        set_speech(&mut driver, Ok(fake.speech(Script::default(), LONG)));
        let next = dictate(&mut driver, &fake).await;
        assert_eq!(driver.output.inserted, [(next, transcript())]);
    }

    #[tokio::test(start_paused = true)]
    async fn sleep_while_the_transcript_is_pending_drops_the_session() {
        let (mut driver, fake) = driver();
        let pending = Script {
            finish: Reply::Never,
            ..Script::default()
        };
        set_speech(&mut driver, Ok(fake.speech(pending, LONG)));
        let generation = dictate(&mut driver, &fake).await;
        assert_eq!(fake.log.take(), [Call::Finish]);

        driver.handle(Event::SystemSleep);
        settle().await;

        assert_eq!(fake.log.take(), [Call::Drop]);
        assert!(fake.events.take().is_empty());
        assert_eq!(driver.output.cancelled, [generation]);
        assert_eq!(driver.output.inserted, []);
        assert!(driver.jobs.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_finished_paste_ends_the_job() {
        let (mut driver, fake) = driver();

        for _ in 0..4 {
            let generation = dictate(&mut driver, &fake).await;
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

    #[tokio::test(start_paused = true)]
    async fn copy_last_result_copies_the_pasted_text() {
        let (mut driver, fake) = driver();
        let generation = dictate(&mut driver, &fake).await;
        driver.handle(Event::InsertDone(generation, InsertOutcome::Landed));

        driver.handle(Event::CopyLastResult);

        assert_eq!(driver.output.copied, [transcript()]);
    }

    #[tokio::test(start_paused = true)]
    async fn sleep_during_the_paste_cancels_the_insertion() {
        let (mut driver, fake) = driver();
        let generation = dictate(&mut driver, &fake).await;

        driver.handle(Event::SystemSleep);

        assert_eq!(driver.output.cancelled, [generation]);
        driver.handle(Event::InsertDone(generation, InsertOutcome::Landed));
        assert_eq!(driver.output.inserted.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_settings_change_after_the_key_press_applies_only_to_later_generations() {
        let (mut driver, fake) = driver();

        driver.handle(Event::DictationKeyDown);
        set_speech(
            &mut driver,
            Ok(fake.corrected_speech(Script::default(), LONG)),
        );
        fire_only_timer(&mut driver);
        driver.handle(Event::DictationKeyUp);
        deliver(&mut driver, &fake).await;
        let first = driver.output.targets[0];
        driver.handle(Event::InsertDone(first, InsertOutcome::Landed));

        fake.chat.answer(Answer {
            reply: Reply::After(Duration::from_secs(1)),
            ..Answer::default()
        });
        let second = release(&mut driver);
        until_the_correction_waits().await;
        assert_eq!(fake.chat.open(), 1);
        set_speech(&mut driver, Ok(fake.speech(Script::default(), LONG)));
        deliver(&mut driver, &fake).await;
        driver.handle(Event::InsertDone(second, InsertOutcome::Landed));

        let third = dictate(&mut driver, &fake).await;

        assert_eq!(
            driver.output.inserted,
            [
                (first, transcript()),
                (second, text(CORRECTED)),
                (third, transcript())
            ]
        );
        assert_eq!(fake.chat.sent().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_job_during_its_correction_gives_no_late_text_and_no_paste() {
        let cancels: [fn(Generation) -> Event; 2] = [
            |generation| Event::CaptureFailed(generation, AppError::Microphone),
            |_| Event::SystemSleep,
        ];
        for cancel in cancels {
            let (mut driver, fake) = driver();
            set_speech(
                &mut driver,
                Ok(fake.corrected_speech(Script::default(), LONG)),
            );
            fake.chat.answer(Answer {
                reply: Reply::Never,
                ..Answer::default()
            });
            let generation = release(&mut driver);
            until_the_correction_waits().await;
            assert_eq!(fake.chat.open(), 1);

            driver.handle(cancel(generation));
            settle().await;

            assert_eq!(fake.chat.open(), 0);
            assert!(fake.events.take().is_empty());
            assert_eq!(driver.output.cancelled, [generation]);
            assert_eq!(driver.output.inserted, []);
            assert!(driver.jobs.is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn corrections_that_end_in_reverse_order_paste_once_each_in_session_order() {
        let (mut driver, fake) = driver();
        set_speech(
            &mut driver,
            Ok(fake.corrected_speech(Script::default(), LONG)),
        );
        for (delay, reply) in [(2, "Premier."), (1, "Second.")] {
            fake.chat.answer(Answer {
                reply: Reply::After(Duration::from_secs(delay)),
                text: reply,
                ..Answer::default()
            });
        }

        let first = release(&mut driver);
        until_the_correction_waits().await;
        let second = release(&mut driver);
        settle().await;
        let events = fake.events.take();
        assert_matches!(
            events.as_slice(),
            [Event::TextReady(early, _), Event::TextReady(late, _)]
                if *early == second && *late == first
        );
        for event in events {
            driver.handle(event);
        }

        assert_eq!(driver.output.inserted, [(first, text("Premier."))]);
        driver.handle(Event::InsertDone(first, InsertOutcome::Landed));
        driver.handle(Event::InsertDone(second, InsertOutcome::Landed));
        assert_eq!(
            driver.output.inserted,
            [(first, text("Premier.")), (second, text("Second."))]
        );
        assert!(driver.jobs.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_next_paste_waits_for_the_end_of_the_previous_one() {
        let (mut driver, fake) = driver();
        let first = dictate(&mut driver, &fake).await;
        let second = dictate(&mut driver, &fake).await;

        assert_eq!(driver.output.inserted, [(first, transcript())]);

        driver.handle(Event::InsertDone(first, InsertOutcome::Landed));

        assert_eq!(
            driver.output.inserted,
            [(first, transcript()), (second, transcript())]
        );
    }
}
