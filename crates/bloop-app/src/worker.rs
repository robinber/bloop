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
use tokio::time::Instant;

#[cfg(test)]
pub(crate) mod fakes;

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

/// Starts the transcription task of each capture.
pub(crate) struct Spawner {
    /// The runtime of the tasks.
    pub(crate) runtime: Handle,
    /// The speech provider, or `None` without an API key.
    pub(crate) speech: Option<Arc<Speech>>,
    /// Receives the result event of each task.
    pub(crate) events: Arc<dyn EventSink>,
}

impl Spawner {
    /// Opens a provider session and streams the samples of `reader`, at `rate` in Hz, to it.
    ///
    /// # Errors
    ///
    /// `Unauthorized` without an API key.
    pub(crate) fn spawn(
        &self,
        generation: Generation,
        rate: NonZeroU32,
        reader: Reader,
    ) -> Result<Task, ProviderError> {
        let speech = self.speech.clone().ok_or(ProviderError::Unauthorized)?;
        let events = Arc::clone(&self.events);
        let end = reader.end_mark();
        let work = self
            .runtime
            .spawn(transcribe(generation, rate, reader, speech));
        let abort = work.abort_handle();
        let report = self.runtime.spawn(async move {
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
        Ok(Task {
            work: abort,
            report,
            end,
        })
    }
}

async fn transcribe(
    generation: Generation,
    rate: NonZeroU32,
    mut reader: Reader,
    speech: Arc<Speech>,
) -> Event {
    let session = match stream(generation, rate, &mut reader, &speech).await {
        Ok(session) => session,
        Err(error) => return Event::CaptureFailed(generation, error),
    };
    let finishing = Instant::now();
    match within(speech.limits.completion, session.finish()).await {
        Ok(transcript) => {
            tracing::info!(
                ?generation,
                millis = finishing.elapsed().as_millis(),
                chars = transcript.as_str().chars().count(),
                "the transcript is complete"
            );
            pipeline::run(generation, transcript)
        }
        Err(error) => Event::WorkFailed(generation, AppError::Provider(error)),
    }
}

/// Streams the samples of `reader`, at `rate` in Hz, until the end of the capture.
///
/// During the capture, it sends only complete chunks and keeps the remaining samples for the next drain.
/// At the end of the capture, it also sends the last, shorter chunk.
async fn stream(
    generation: Generation,
    rate: NonZeroU32,
    reader: &mut Reader,
    speech: &Speech,
) -> Result<Box<dyn TranscriptionSession>, AppError> {
    let started = Instant::now();
    let options = TranscriptionOptions {
        language: &speech.language,
        vocabulary: &speech.vocabulary,
        sample_rate: rate,
    };
    let start = speech.transcriber.start(options);
    let mut session = watch(reader, speech.limits.connection, start).await?;
    let mut samples = Vec::new();
    let mut status = reader.drain(&mut samples).map_err(AppError::AudioQueue)?;
    tracing::info!(
        ?generation,
        millis = started.elapsed().as_millis(),
        backlog_samples = samples.len(),
        "the provider session opened"
    );
    let length = AudioChunk::length(rate).get();
    let mut chunks = 0_usize;
    let mut sent = 0_usize;
    loop {
        let ready = match status {
            Status::Open => samples.len() / length * length,
            Status::Ended => samples.len(),
        };
        for chunk in AudioChunk::split(&samples[..ready], rate) {
            watch(reader, speech.limits.inactivity, session.send_audio(&chunk)).await?;
            chunks += 1;
        }
        sent += ready;
        samples.drain(..ready);
        if status == Status::Ended {
            tracing::info!(
                ?generation,
                chunks,
                samples = sent,
                millis = started.elapsed().as_millis(),
                "the capture audio drained"
            );
            return Ok(session);
        }
        tokio::time::sleep(POLL).await;
        status = reader.drain(&mut samples).map_err(AppError::AudioQueue)?;
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
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Duration;

    use bloop_audio::buffer::{self, Writer};
    use bloop_core::domain::Generation;
    use bloop_core::error::{AppError, AudioQueueError, ProviderError};
    use bloop_core::ports::platform::AudioSink;
    use bloop_core::session::{Command, Core, Event};
    use tokio::time::sleep;

    use super::fakes::{
        Call, Fake, LONG, RATE, Recorder, Reply, Script, TEXT, pcm, samples, sent_audio, settle,
    };
    use super::{Limits, Task};

    const CAPACITY: usize = 16_000;

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
            let fake = Fake::default();
            let generation = generation();
            let task = fake
                .spawner(script, limits)
                .spawn(generation, rate, reader)
                .unwrap();
            let Fake { log, rates, events } = fake;
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

    #[tokio::test(start_paused = true)]
    async fn audio_reaches_the_provider_in_order_and_the_session_finishes_after_it() {
        let mut run = Run::start(Script::default(), CAPACITY, LONG);
        let audio = samples(4_000);

        run.push(&audio[..1_600]).unwrap();
        settle().await;
        let calls = run.log.take();
        assert_eq!(sent_audio(&calls), pcm(&audio[..1_600]));
        assert!(!calls.contains(&Call::Finish));
        assert!(run.events.take().is_empty());
        run.push(&audio[1_600..]).unwrap();
        run.end_capture();
        settle().await;

        let calls = run.log.take();
        assert_eq!(sent_audio(&calls), pcm(&audio[1_600..]));
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
    async fn the_capture_audio_goes_in_chunks_of_100_ms_and_the_end_sends_the_rest() {
        let mut run = Run::start(Script::default(), CAPACITY, LONG);
        let audio = samples(4_900);

        for part in audio.chunks(700) {
            run.push(part).unwrap();
            settle().await;
        }
        run.end_capture();
        settle().await;

        let calls = run.log.take();
        let lengths: Vec<usize> = calls
            .iter()
            .filter_map(|call| match call {
                Call::Audio(pcm) => Some(pcm.len()),
                Call::Finish | Call::Drop => None,
            })
            .collect();
        assert_eq!(lengths, [3_200, 3_200, 3_200, 200]);
        assert_eq!(sent_audio(&calls), pcm(&audio));
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

        assert_eq!(run.log.take(), [] as [Call; 0]);
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

        run.push(&samples(1_600)).unwrap();
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

        run.push(&samples(1_600)).unwrap();
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
            (
                Script {
                    start: Reply::Fail(ProviderError::Unauthorized),
                    ..Script::default()
                },
                vec![],
            ),
            (
                Script {
                    send: Reply::Fail(ProviderError::Network),
                    ..Script::default()
                },
                vec![Call::Drop],
            ),
        ];
        for (script, calls) in failures {
            let mut run = Run::start(script, CAPACITY, LONG);

            run.push(&samples(1_600)).unwrap();
            settle().await;

            assert_eq!(run.log.take(), calls, "{script:?}");
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

        run.push(&samples(1_600)).unwrap();
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop]);
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(generation, AppError::Internal) if generation == run.generation
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_task_releases_the_session_during_send_and_finish() {
        let waits = [
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

        assert_eq!(run.log.take(), [] as [Call; 0]);
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

        run.push(&samples(1_600)).unwrap();
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
