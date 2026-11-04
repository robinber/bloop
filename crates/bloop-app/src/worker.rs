//! Transcription task of one generation: reads the audio queue, streams it to the provider,
//! and sends the result event.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use bloop_audio::buffer::{EndMark, Reader, Status};
use bloop_core::domain::{AudioChunk, Generation};
use bloop_core::error::{AppError, AudioQueueError, ProviderError};
use bloop_core::pipeline;
use bloop_core::ports::ai::{Transcriber, TranscriptionOptions, TranscriptionSession};
use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinHandle};

const POLL: Duration = Duration::from_millis(20);

/// The speech provider and the options of its sessions.
pub(crate) struct Speech {
    pub(crate) transcriber: Box<dyn Transcriber>,
    pub(crate) language: String,
    pub(crate) vocabulary: Vec<String>,
    pub(crate) limits: Limits,
}

/// The time limits of the provider operations. The worker applies each limit to one operation.
/// An operation that does not end within its limit fails with `ProviderError::Timeout`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The maximum duration of `start`: the connection and the setup of the session.
    pub(crate) connection: Duration,
    /// The maximum duration of each `send_audio`. It detects a stalled transport, not speech silence.
    pub(crate) inactivity: Duration,
    /// The maximum duration of `finish`. It counts from the `finish` call, after the backlog drained,
    /// because the provider messages stay inside the adapter.
    pub(crate) completion: Duration,
}

impl Default for Limits {
    /// Initial values, not measured: connection 5 s, inactivity 5 s, and completion 15 s.
    /// Phase 2 measurements replace them.
    fn default() -> Self {
        Self {
            connection: Duration::from_secs(5),
            inactivity: Duration::from_secs(5),
            completion: Duration::from_secs(15),
        }
    }
}

/// The transcription task of one generation. It sends one result event, unless it is dropped first.
///
/// A work task runs the transcription, and a report task sends its result event.
/// The report task converts a panic of the work task into `CaptureFailed` with
/// `AppError::Internal`, because the core applies `CaptureFailed` to a listening capture
/// and to a processing job.
#[derive(Debug)]
#[must_use = "dropping a Task cancels its transcription"]
pub(crate) struct Task {
    work: AbortHandle,
    report: JoinHandle<()>,
    end: EndMark,
}

impl Task {
    /// Opens a provider session and streams the samples of `reader`, at `rate` in Hz, to it.
    pub(crate) fn spawn(
        runtime: &Handle,
        generation: Generation,
        rate: NonZeroU32,
        reader: Reader,
        speech: Arc<Speech>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        let end = reader.end_mark();
        let work = runtime.spawn(transcribe(generation, rate, reader, speech));
        let abort = work.abort_handle();
        let report = runtime.spawn(async move {
            let event = match work.await {
                Ok(event) => event,
                Err(error) if error.is_cancelled() => return,
                Err(_) => {
                    tracing::error!(?generation, "the transcription task panicked");
                    Event::CaptureFailed(generation, AppError::Internal)
                }
            };
            events.send(event);
        });
        Self {
            work: abort,
            report,
            end,
        }
    }

    /// Ends the capture normally. Then the driver stops the audio input,
    /// and the work task finishes the session after the last samples.
    pub(crate) fn end_capture(&self) {
        self.end.set();
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        self.work.abort();
        self.report.abort();
    }
}

async fn transcribe(
    generation: Generation,
    rate: NonZeroU32,
    mut reader: Reader,
    speech: Arc<Speech>,
) -> Event {
    let session = match stream(rate, &mut reader, &speech).await {
        Ok(session) => session,
        Err(error) => return Event::CaptureFailed(generation, error),
    };
    match within(speech.limits.completion, session.finish()).await {
        Ok(transcript) => pipeline::run(generation, transcript),
        Err(error) => Event::WorkFailed(generation, AppError::Provider(error)),
    }
}

/// The samples of `reader` are at `rate`, in Hz.
async fn stream(
    rate: NonZeroU32,
    reader: &mut Reader,
    speech: &Speech,
) -> Result<Box<dyn TranscriptionSession>, AppError> {
    let options = TranscriptionOptions {
        language: &speech.language,
        vocabulary: &speech.vocabulary,
        sample_rate: rate,
    };
    let start = speech.transcriber.start(options);
    let mut session = watch(reader, speech.limits.connection, start).await?;
    let mut samples = Vec::new();
    loop {
        samples.clear();
        let status = reader.drain(&mut samples).map_err(AppError::AudioQueue)?;
        for chunk in AudioChunk::split(&samples, rate) {
            watch(reader, speech.limits.inactivity, session.send_audio(&chunk)).await?;
        }
        if status == Status::Ended {
            return Ok(session);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Runs a provider operation within `limit` and fails as soon as the queue fails,
/// so that a slow provider does not hide a full queue or a lost microphone.
async fn watch<T>(
    reader: &mut Reader,
    limit: Duration,
    operation: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, AppError> {
    tokio::select! {
        biased;
        result = within(limit, operation) => result.map_err(AppError::Provider),
        error = queue_failure(reader) => Err(AppError::AudioQueue(error)),
    }
}

/// Runs a provider operation and fails with `Timeout` when it does not end within `limit`.
/// The expired operation is dropped, which releases what it holds.
async fn within<T>(
    limit: Duration,
    operation: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    tokio::time::timeout(limit, operation)
        .await
        .unwrap_or(Err(ProviderError::Timeout))
}

async fn queue_failure(reader: &mut Reader) -> AudioQueueError {
    loop {
        if let Some(error) = reader.failure() {
            return error;
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::future;
    use std::num::NonZeroU32;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use bloop_audio::buffer::{self, Writer};
    use bloop_core::domain::{AudioChunk, Generation, Transcript};
    use bloop_core::error::{AppError, AudioQueueError, ProviderError};
    use bloop_core::ports::ai::{
        BoxFuture, Transcriber, TranscriptionOptions, TranscriptionSession,
    };
    use bloop_core::ports::platform::{AudioSink, EventSink};
    use bloop_core::session::{Command, Core, Event};
    use tokio::runtime::Handle;
    use tokio::time::sleep;

    use super::{Limits, Speech, Task};

    const TEXT: &str = "Bonjour à tous.";
    const CAPACITY: usize = 16_000;
    const RATE: NonZeroU32 = NonZeroU32::new(16_000).unwrap();
    const HOUR: Duration = Duration::from_secs(3_600);
    /// Limits that no test reaches, so that `Reply::Never` means no event within `settle`.
    const LONG: Limits = Limits {
        connection: HOUR,
        inactivity: HOUR,
        completion: HOUR,
    };

    #[derive(Debug, Clone, Copy, Default)]
    enum Reply {
        #[default]
        Ok,
        After(Duration),
        Fail(ProviderError),
        Never,
        Panic,
    }

    impl Reply {
        async fn wait(self) -> Result<(), ProviderError> {
            match self {
                Self::Ok => Ok(()),
                Self::After(delay) => {
                    sleep(delay).await;
                    Ok(())
                }
                Self::Fail(error) => Err(error),
                Self::Never => future::pending().await,
                Self::Panic => panic!("the fake provider panicked"),
            }
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct Script {
        start: Reply,
        send: Reply,
        finish: Reply,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Audio(Vec<u8>),
        Finish,
        Drop,
    }

    struct Recorder<T>(Mutex<Vec<T>>);

    impl<T> Default for Recorder<T> {
        fn default() -> Self {
            Self(Mutex::new(Vec::new()))
        }
    }

    impl<T> Recorder<T> {
        fn push(&self, item: T) {
            self.0.lock().unwrap().push(item);
        }

        fn take(&self) -> Vec<T> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl EventSink for Recorder<Event> {
        fn send(&self, event: Event) {
            self.push(event);
        }
    }

    struct FakeTranscriber {
        script: Script,
        log: Arc<Recorder<Call>>,
        rates: Arc<Recorder<NonZeroU32>>,
    }

    impl Transcriber for FakeTranscriber {
        fn start<'a>(
            &'a self,
            options: TranscriptionOptions<'a>,
        ) -> BoxFuture<'a, Result<Box<dyn TranscriptionSession>, ProviderError>> {
            self.rates.push(options.sample_rate);
            let session = FakeSession {
                script: self.script,
                log: Arc::clone(&self.log),
            };
            Box::pin(async move {
                self.script.start.wait().await?;
                Ok(Box::new(session) as Box<dyn TranscriptionSession>)
            })
        }
    }

    struct FakeSession {
        script: Script,
        log: Arc<Recorder<Call>>,
    }

    impl TranscriptionSession for FakeSession {
        fn send_audio<'a>(
            &'a mut self,
            chunk: &'a AudioChunk,
        ) -> BoxFuture<'a, Result<(), ProviderError>> {
            Box::pin(async move {
                self.script.send.wait().await?;
                self.log.push(Call::Audio(chunk.as_bytes().to_vec()));
                Ok(())
            })
        }

        fn finish(self: Box<Self>) -> BoxFuture<'static, Result<Transcript, ProviderError>> {
            Box::pin(async move {
                self.log.push(Call::Finish);
                self.script.finish.wait().await?;
                Ok(Transcript::new(TEXT.to_owned()))
            })
        }
    }

    impl Drop for FakeSession {
        fn drop(&mut self) {
            self.log.push(Call::Drop);
        }
    }

    struct Run {
        generation: Generation,
        task: Option<Task>,
        writer: Option<Writer>,
        log: Arc<Recorder<Call>>,
        rates: Arc<Recorder<NonZeroU32>>,
        events: Arc<Recorder<Event>>,
    }

    impl Run {
        fn start(script: Script, capacity: usize, limits: Limits) -> Self {
            Self::start_at(RATE, script, capacity, limits)
        }

        fn start_at(rate: NonZeroU32, script: Script, capacity: usize, limits: Limits) -> Self {
            let (writer, reader) = buffer::new(capacity);
            let log = Arc::new(Recorder::default());
            let rates = Arc::new(Recorder::default());
            let events = Arc::new(Recorder::default());
            let speech = Speech {
                transcriber: Box::new(FakeTranscriber {
                    script,
                    log: Arc::clone(&log),
                    rates: Arc::clone(&rates),
                }),
                language: "fr".to_owned(),
                vocabulary: Vec::new(),
                limits,
            };
            let generation = generation();
            let task = Task::spawn(
                &Handle::current(),
                generation,
                rate,
                reader,
                Arc::new(speech),
                Arc::clone(&events) as Arc<dyn EventSink>,
            );
            Self {
                generation,
                task: Some(task),
                writer: Some(writer),
                log,
                rates,
                events,
            }
        }

        fn push(&mut self, samples: &[f32]) -> Result<(), AudioQueueError> {
            self.writer.as_mut().expect("a writer").try_push(samples)
        }

        fn end_capture(&mut self) {
            self.task.as_ref().expect("a task").end_capture();
            self.writer = None;
        }

        fn only_event(&self) -> Event {
            let [event] = <[_; 1]>::try_from(self.events.take()).expect("one event");
            event
        }
    }

    fn generation() -> Generation {
        let commands = Core::new().handle(Event::DictationKeyDown);
        let [Command::StartHoldTimer(generation)] = commands.as_slice() else {
            panic!("{commands:?}");
        };
        *generation
    }

    fn samples(count: u16) -> Vec<f32> {
        (0..count).map(|index| f32::from(index) / 4_096.0).collect()
    }

    fn pcm(samples: &[f32]) -> Vec<u8> {
        AudioChunk::split(samples, RATE)
            .flat_map(|chunk| chunk.as_bytes().to_vec())
            .collect()
    }

    fn sent_audio(calls: &[Call]) -> Vec<u8> {
        calls
            .iter()
            .filter_map(|call| match call {
                Call::Audio(pcm) => Some(pcm.as_slice()),
                Call::Finish | Call::Drop => None,
            })
            .flatten()
            .copied()
            .collect()
    }

    /// Runs the task until it ends or waits for a reply that never comes.
    /// The paused clock advances only when all tasks wait.
    async fn settle() {
        sleep(Duration::from_secs(5)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn audio_reaches_the_provider_in_order_and_the_session_finishes_after_it() {
        let mut run = Run::start(Script::default(), CAPACITY, LONG);
        let audio = samples(4_000);

        run.push(&audio[..1_000]).unwrap();
        settle().await;
        let calls = run.log.take();
        assert_eq!(sent_audio(&calls), pcm(&audio[..1_000]));
        assert!(!calls.contains(&Call::Finish));
        assert!(run.events.take().is_empty());
        run.push(&audio[1_000..]).unwrap();
        run.end_capture();
        settle().await;

        let calls = run.log.take();
        assert_eq!(sent_audio(&calls), pcm(&audio[1_000..]));
        assert_matches!(
            calls.as_slice(),
            [.., Call::Audio(_), Call::Finish, Call::Drop]
        );
        assert_matches!(
            run.only_event(),
            Event::TextReady(generation, text) if generation == run.generation && text.as_str() == TEXT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_session_and_the_chunks_use_the_rate_of_the_microphone() {
        let rate = NonZeroU32::new(48_000).unwrap();
        let mut run = Run::start_at(rate, Script::default(), CAPACITY, LONG);

        run.push(&samples(4_800)).unwrap();
        run.end_capture();
        settle().await;

        assert_eq!(run.rates.take(), [rate]);
        assert_matches!(
            run.log.take().as_slice(),
            [Call::Audio(pcm), Call::Finish, Call::Drop] if pcm.len() == 9_600
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_connection_keeps_the_audio_of_the_capture() {
        let script = Script {
            start: Reply::After(Duration::from_secs(2)),
            ..Script::default()
        };
        let mut run = Run::start(script, CAPACITY, LONG);
        let audio = samples(3_000);

        run.push(&audio).unwrap();
        run.end_capture();
        settle().await;

        let calls = run.log.take();
        assert_eq!(sent_audio(&calls), pcm(&audio));
        assert_matches!(calls.as_slice(), [.., Call::Finish, Call::Drop]);
        assert_matches!(run.only_event(), Event::TextReady(..));
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_queue_fails_the_capture_without_finishing() {
        let mut run = Run::start(Script::default(), 8, LONG);

        run.push(&samples(4)).unwrap();
        assert_eq!(run.push(&samples(8)), Err(AudioQueueError::Full));
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Full))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_queue_fails_the_capture_during_a_slow_connection() {
        let script = Script {
            start: Reply::Never,
            ..Script::default()
        };
        let mut run = Run::start(script, 8, LONG);

        run.push(&samples(4)).unwrap();
        assert_eq!(run.push(&samples(8)), Err(AudioQueueError::Full));
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Full))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_overflow_after_the_end_mark_fails_the_capture() {
        let mut run = Run::start(Script::default(), 8, LONG);

        run.push(&samples(4)).unwrap();
        run.task.as_ref().expect("a task").end_capture();
        assert_eq!(run.push(&samples(8)), Err(AudioQueueError::Full));
        run.writer = None;
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Full))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_lost_microphone_fails_the_capture_without_finishing() {
        let mut run = Run::start(Script::default(), CAPACITY, LONG);

        run.push(&samples(100)).unwrap();
        settle().await;
        run.writer = None;
        settle().await;

        assert_matches!(run.log.take().as_slice(), [Call::Audio(_), Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Closed))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_lost_microphone_fails_the_capture_during_a_blocked_send() {
        let script = Script {
            send: Reply::Never,
            ..Script::default()
        };
        let mut run = Run::start(script, CAPACITY, LONG);

        run.push(&samples(100)).unwrap();
        settle().await;
        run.writer = None;
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Closed))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_provider_failure_while_streaming_fails_the_capture() {
        let failures = [
            Script {
                start: Reply::Fail(ProviderError::Unauthorized),
                ..Script::default()
            },
            Script {
                send: Reply::Fail(ProviderError::Network),
                ..Script::default()
            },
        ];
        for script in failures {
            let mut run = Run::start(script, CAPACITY, LONG);

            run.push(&samples(100)).unwrap();
            settle().await;

            assert_eq!(run.log.take(), [Call::Drop], "{script:?}");
            assert_matches!(
                run.only_event(),
                Event::CaptureFailed(
                    _,
                    AppError::Provider(ProviderError::Unauthorized | ProviderError::Network)
                )
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_provider_failure_after_the_key_release_fails_the_capture() {
        let script = Script {
            send: Reply::Fail(ProviderError::Network),
            ..Script::default()
        };
        let mut run = Run::start(script, CAPACITY, LONG);

        run.push(&samples(100)).unwrap();
        run.end_capture();
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(_, AppError::Provider(ProviderError::Network))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_capture_without_samples_finishes_once() {
        let mut run = Run::start(Script::default(), CAPACITY, LONG);

        run.end_capture();
        settle().await;

        assert_eq!(run.log.take(), [Call::Finish, Call::Drop]);
        assert_matches!(run.only_event(), Event::TextReady(..));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_finish_fails_the_job() {
        let script = Script {
            finish: Reply::Fail(ProviderError::Timeout),
            ..Script::default()
        };
        let mut run = Run::start(script, CAPACITY, LONG);

        run.push(&samples(100)).unwrap();
        run.end_capture();
        settle().await;

        assert_matches!(
            run.log.take().as_slice(),
            [Call::Audio(_), Call::Finish, Call::Drop]
        );
        assert_matches!(
            run.only_event(),
            Event::WorkFailed(_, AppError::Provider(ProviderError::Timeout))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_panic_in_the_provider_fails_the_capture() {
        let script = Script {
            send: Reply::Panic,
            ..Script::default()
        };
        let mut run = Run::start(script, CAPACITY, LONG);

        run.push(&samples(100)).unwrap();
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(generation, AppError::Internal) if generation == run.generation
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_task_releases_the_session_at_each_wait() {
        let waits = [
            Script {
                start: Reply::Never,
                ..Script::default()
            },
            Script {
                send: Reply::Never,
                ..Script::default()
            },
            Script {
                finish: Reply::Never,
                ..Script::default()
            },
        ];
        for script in waits {
            let mut run = Run::start(script, CAPACITY, LONG);

            run.push(&samples(100)).unwrap();
            run.end_capture();
            settle().await;
            assert!(!run.log.take().contains(&Call::Drop), "{script:?}");
            run.task = None;
            settle().await;

            assert_eq!(run.log.take(), [Call::Drop], "{script:?}");
            assert!(run.events.take().is_empty(), "{script:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_start_that_never_answers_fails_the_capture_after_the_connection_limit() {
        let script = Script {
            start: Reply::Never,
            ..Script::default()
        };
        let limits = Limits {
            connection: Duration::from_secs(2),
            ..LONG
        };
        let mut run = Run::start(script, CAPACITY, limits);

        run.push(&samples(100)).unwrap();
        sleep(Duration::from_millis(1_900)).await;
        assert_matches!(run.log.take().as_slice(), []);
        assert!(run.events.take().is_empty());
        sleep(Duration::from_millis(200)).await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(generation, AppError::Provider(ProviderError::Timeout))
                if generation == run.generation
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_send_that_never_answers_fails_the_capture_after_the_inactivity_limit() {
        let script = Script {
            send: Reply::Never,
            ..Script::default()
        };
        let limits = Limits {
            inactivity: Duration::from_secs(2),
            ..LONG
        };
        let mut run = Run::start(script, CAPACITY, limits);

        run.push(&samples(100)).unwrap();
        sleep(Duration::from_millis(1_900)).await;
        assert_matches!(run.log.take().as_slice(), []);
        assert!(run.events.take().is_empty());
        sleep(Duration::from_millis(200)).await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(generation, AppError::Provider(ProviderError::Timeout))
                if generation == run.generation
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_finish_that_never_answers_fails_the_job_after_the_completion_limit() {
        let script = Script {
            finish: Reply::Never,
            ..Script::default()
        };
        let limits = Limits {
            completion: Duration::from_secs(2),
            ..LONG
        };
        let mut run = Run::start(script, CAPACITY, limits);

        run.push(&samples(100)).unwrap();
        run.end_capture();
        sleep(Duration::from_millis(1_900)).await;
        assert_matches!(run.log.take().as_slice(), [Call::Audio(_), Call::Finish]);
        assert!(run.events.take().is_empty());
        sleep(Duration::from_millis(200)).await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::WorkFailed(generation, AppError::Provider(ProviderError::Timeout))
                if generation == run.generation
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_provider_within_each_limit_still_produces_the_transcript() {
        let slow = Reply::After(Duration::from_millis(1_900));
        let script = Script {
            start: slow,
            send: slow,
            finish: slow,
        };
        let limit = Duration::from_secs(2);
        let limits = Limits {
            connection: limit,
            inactivity: limit,
            completion: limit,
        };
        let mut run = Run::start(script, CAPACITY, limits);
        let audio = samples(4_800);

        run.push(&audio).unwrap();
        run.end_capture();
        sleep(Duration::from_secs(10)).await;

        let calls = run.log.take();
        assert_eq!(sent_audio(&calls), pcm(&audio));
        assert_matches!(
            calls.as_slice(),
            [
                Call::Audio(_),
                Call::Audio(_),
                Call::Audio(_),
                Call::Finish,
                Call::Drop
            ]
        );
        assert_matches!(
            run.only_event(),
            Event::TextReady(generation, text) if generation == run.generation && text.as_str() == TEXT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_inactivity_limit_applies_to_each_send_and_not_to_the_capture() {
        let script = Script {
            send: Reply::After(Duration::from_millis(600)),
            ..Script::default()
        };
        let limits = Limits {
            inactivity: Duration::from_secs(1),
            ..LONG
        };
        let mut run = Run::start(script, CAPACITY, limits);
        let audio = samples(6_400);

        run.push(&audio).unwrap();
        settle().await;
        let calls = run.log.take();
        assert_eq!(sent_audio(&calls), pcm(&audio));
        assert_matches!(
            calls.as_slice(),
            [
                Call::Audio(_),
                Call::Audio(_),
                Call::Audio(_),
                Call::Audio(_)
            ]
        );
        assert!(run.events.take().is_empty());
        run.end_capture();
        settle().await;

        assert_eq!(run.log.take(), [Call::Finish, Call::Drop]);
        assert_matches!(run.only_event(), Event::TextReady(..));
    }
}
