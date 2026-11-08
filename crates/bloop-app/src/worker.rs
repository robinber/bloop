//! Transcription task of one generation: reads the audio queue, streams it to the provider,
//! applies the text steps, and sends the result event.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use bloop_audio::buffer::{EndMark, Reader, Status};
use bloop_audio::convert::{self, ConvertError, Converter};
use bloop_core::domain::{AudioChunk, Generation};
use bloop_core::error::{AppError, AudioQueueError, ProviderError};
use bloop_core::pipeline::Pipeline;
use bloop_core::ports::ai::{ChatModel, Transcriber, TranscriptionOptions, TranscriptionSession};
use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use bloop_core::steps;
use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::Instant;

use crate::prompts::Prompt;

#[cfg(test)]
pub(crate) mod fakes;

const POLL: Duration = Duration::from_millis(20);

/// The speech provider, the options of its sessions, and the text steps of its transcripts.
pub(crate) struct Speech {
    pub(crate) transcriber: Box<dyn Transcriber>,
    /// The correction steps, or why they are not available.
    pub(crate) correction: Result<Correction, NoCorrection>,
    pub(crate) language: String,
    pub(crate) vocabulary: Vec<String>,
    pub(crate) limits: Limits,
}

impl Speech {
    /// Returns the correction steps when they are available.
    /// Logs the choice of the job, without text.
    fn text_steps(&self, generation: Generation) -> Option<&Pipeline> {
        let correction = self.correction.as_ref().ok();
        tracing::info!(
            ?generation,
            correction = correction.is_some(),
            prompt = correction.map(|correction| correction.prompt.as_str()),
            "the text steps start"
        );
        if let Err(reason @ NoCorrection::ChatUnavailable) = &self.correction {
            tracing::warn!(
                ?generation,
                ?reason,
                "the correction is not available, the job keeps the raw transcript"
            );
        }
        correction.map(|correction| &correction.pipeline)
    }
}

/// The text steps of a job with the correction on, and the name of their prompt.
/// The prompt text is only in the steps.
pub(crate) struct Correction {
    prompt: String,
    pipeline: Pipeline,
}

impl Correction {
    /// The correction steps of `bloop_core::steps::correction` with `prompt`.
    pub(crate) fn new(model: Arc<dyn ChatModel>, prompt: Prompt) -> Self {
        Self {
            prompt: prompt.name,
            pipeline: Pipeline::new(steps::correction(model, prompt.text)),
        }
    }
}

/// Why the correction steps are not available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoCorrection {
    /// The "Correct text" menu item is off. A job keeps the raw transcript.
    Off,
    /// The chat model is not available, for example without an API key.
    /// A job with the correction on keeps the raw transcript.
    ChatUnavailable,
    /// The selected prompt is missing or not valid. With the correction on, each capture fails with `InvalidSettings`.
    InvalidPrompt,
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
    /// Operation limits: connection 5 s, inactivity 5 s, and completion 15 s.
    /// These are configured bounds, not measured latencies.
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
    pub(crate) runtime: Handle,
    /// Receives the result event of each task.
    pub(crate) events: Arc<dyn EventSink>,
}

impl Spawner {
    /// Opens a provider session of `speech` and streams the samples of `reader`, at `rate` in Hz, to it.
    /// `speech` comes from the settings snapshot of the generation. Thus a settings change never affects the task.
    /// The task applies the correction to its transcript when `speech` has it.
    ///
    /// # Errors
    ///
    /// The error of `speech` when speech is not available: for example,
    /// `Provider(Unauthorized)` without an API key, or `InvalidSettings` with an invalid dictionary.
    /// `InvalidSettings` when the correction is on and the selected prompt is not valid.
    pub(crate) fn spawn(
        &self,
        generation: Generation,
        rate: NonZeroU32,
        reader: Reader,
        speech: &Result<Arc<Speech>, AppError>,
    ) -> Result<Task, AppError> {
        let speech = speech.clone()?;
        if matches!(speech.correction, Err(NoCorrection::InvalidPrompt)) {
            tracing::warn!(
                ?generation,
                "the prompt is not valid, the capture with the correction on fails"
            );
            return Err(AppError::InvalidSettings);
        }
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
            let raw = Pipeline::new(Vec::new());
            let pipeline = speech.text_steps(generation).unwrap_or(&raw);
            let output = pipeline.run(generation, transcript).await;
            for report in &output.report {
                tracing::info!(
                    ?generation,
                    step = report.step,
                    result = ?report.result,
                    "a text step ended"
                );
            }
            output.event
        }
        Err(error) => Event::WorkFailed(generation, AppError::Provider(error)),
    }
}

/// Streams the samples of `reader`, at `rate` in Hz, until the end of the capture.
///
/// It converts the samples to `convert::RATE` and sends them at that rate.
/// During the capture, it sends only complete chunks and keeps the remaining samples for the next drain.
/// At the end of the capture, it flushes the converter and also sends the last, shorter chunk.
async fn stream(
    generation: Generation,
    rate: NonZeroU32,
    reader: &mut Reader,
    speech: &Speech,
) -> Result<Box<dyn TranscriptionSession>, AppError> {
    let started = Instant::now();
    let failed = move |error| conversion_failed(generation, &error);
    let mut converter = Converter::new(rate).map_err(failed)?;
    let options = TranscriptionOptions {
        language: &speech.language,
        vocabulary: &speech.vocabulary,
        sample_rate: convert::RATE,
    };
    let start = speech.transcriber.start(options);
    let mut session = watch(reader, speech.limits.connection, start).await?;
    let mut captured = Vec::new();
    let mut status = reader.drain(&mut captured).map_err(AppError::AudioQueue)?;
    tracing::info!(
        ?generation,
        millis = started.elapsed().as_millis(),
        backlog_samples = captured.len(),
        backlog_hz = rate.get(),
        "the provider session opened"
    );
    let length = AudioChunk::length(convert::RATE).get();
    let limit = speech.limits.inactivity;
    let mut samples = Vec::new();
    let mut chunks = 0_usize;
    let mut sent = 0_usize;
    loop {
        converter.push(&captured, &mut samples).map_err(failed)?;
        captured.clear();
        if status == Status::Ended {
            break;
        }
        let ready = samples.len() / length * length;
        chunks += send(&mut *session, reader, limit, &samples[..ready]).await?;
        sent += ready;
        samples.drain(..ready);
        tokio::time::sleep(POLL).await;
        status = reader.drain(&mut captured).map_err(AppError::AudioQueue)?;
    }
    converter.flush(&mut samples).map_err(failed)?;
    chunks += send(&mut *session, reader, limit, &samples).await?;
    sent += samples.len();
    tracing::info!(
        ?generation,
        chunks,
        samples = sent,
        samples_hz = convert::RATE.get(),
        millis = started.elapsed().as_millis(),
        "the capture audio drained"
    );
    Ok(session)
}

/// Sends `samples`, at `convert::RATE`, in chunks, and returns the number of chunks.
async fn send(
    session: &mut dyn TranscriptionSession,
    reader: &mut Reader,
    limit: Duration,
    samples: &[f32],
) -> Result<usize, AppError> {
    let mut chunks = 0;
    for chunk in AudioChunk::split(samples, convert::RATE) {
        watch(reader, limit, session.send_audio(&chunk)).await?;
        chunks += 1;
    }
    Ok(chunks)
}

/// Logs a conversion failure, without audio, and fails the capture with `Internal`:
/// the converter fails only through a bug.
fn conversion_failed(generation: Generation, error: &ConvertError) -> AppError {
    tracing::warn!(?generation, ?error, "the audio conversion failed");
    AppError::Internal
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
mod tests;
