# Rust contracts

Status: revision 4. `bloop-core` implements the session state machine, `Generation`, `FinalText`, the existing error enums,
and the `EventSink` and `Scheduler` ports.
The audio and streaming signatures below are Phase 2 proposals without implementation.
The text step signatures are Phase 3 proposals. `AudioQueueError` is a proposed addition to the error types.
Configurable capture duration and its warning are Phase 2 requirements. The current state machine already accepts `CaptureLimitReached`.

All types and traits in this document are in `bloop-core`.
The [code structure](STRUCTURE.md) gives the file for each type.

Finish Phase 1 with fixed text before implementing audio.
Streaming is the only STT path. Every speech provider implements the same session lifecycle.

## Session state machine

The core is a pure state machine. It receives events and returns commands.
The core does not call macOS APIs, start threads, or read the clock.
`Core::handle(&mut self, event: Event) -> Vec<Command>` is the only entry point for events.
`Core::capture_active(&self) -> bool` tells the event tap when to block Escape.
`Capture`, `JobStatus`, and `Job` are private to `session.rs`. Only the core creates a `Generation`.

```rust
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation(pub(crate) u64);

enum Capture {
    Idle,
    Pending(Generation),
    Listening(Generation),
}

enum JobStatus {
    Processing,
    Ready(FinalText),
    Inserting(FinalText),
}

struct Job {
    generation: Generation,
    status: JobStatus,
}

pub struct Core {
    capture: Capture,
    jobs: VecDeque<Job>,
    next_generation: u64,
    last_result: Option<FinalText>,
    pending_card: Option<FinalText>,
}

pub enum Event {
    DictationKeyDown,
    DictationKeyUp,
    OtherInputDuringHold,
    Escape,
    HoldElapsed(Generation),
    CaptureLimitReached(Generation),
    CaptureFailed(Generation, AppError),
    TextReady(Generation, FinalText),
    NothingHeard(Generation),
    WorkFailed(Generation, AppError),
    InsertDone(Generation, InsertOutcome),
    CopyLastResult,
    SystemSleep,
}

pub enum Command {
    StartHoldTimer(Generation),
    StartCapture(Generation),
    RecordTarget(Generation),
    StopCaptureAndProcess(Generation),
    CancelCapture,
    CancelWork(Generation),
    Insert(Generation, FinalText),
    CopyToClipboard(FinalText),
    ShowOrb(OrbView),
    CloseOrb,
}

pub enum OrbView {
    Listening,
    Error,
    Card(FinalText),
}

pub enum InsertOutcome {
    Landed,
    Fallback(FallbackReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    TargetChanged,
    SecureInput,
    NotRead,
}
```

## Domain data

```rust
use std::{
    sync::{atomic::AtomicU32, Arc},
    time::Duration,
};

pub struct AudioChunk(Vec<u8>);

pub struct CaptureSummary {
    pub duration: Duration,
    pub peak: f32,
}

#[derive(Clone, Default)]
pub struct LevelMeter(Arc<AtomicU32>);

#[derive(Clone, Default)]
pub struct TargetInfo {
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
}

pub struct Transcript(String);

#[derive(Clone, PartialEq, Eq)]
pub struct FinalText(String);
```

`AudioChunk` contains 16 kHz mono signed 16-bit little-endian PCM, without a file header.
Its constructor checks for nonempty data, whole samples, and the selected maximum chunk size.
`AudioChunk::as_bytes` exposes the validated bytes. Only the worker creates these chunks.
`CaptureSummary` contains the duration and peak level. It contains no recording.
`FinalText::new` creates a final text and `FinalText::as_str` reads it. `Transcript` uses the same shape.
Audio and text types use a redacting `Debug` implementation.

## AI ports

```rust
use std::{future::Future, pin::Pin};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct TranscriptionOptions<'a> {
    pub language: &'a str,
    pub vocabulary: &'a [String],
}

pub trait Transcriber: Send + Sync {
    fn start<'a>(&'a self, options: TranscriptionOptions<'a>)
        -> BoxFuture<'a, Result<Box<dyn TranscriptionSession>, ProviderError>>;
}

pub trait TranscriptionSession: Send + 'static {
    fn send_audio<'a>(&'a mut self, chunk: &'a AudioChunk)
        -> BoxFuture<'a, Result<(), ProviderError>>;

    fn finish(self: Box<Self>)
        -> BoxFuture<'static, Result<Transcript, ProviderError>>;

    fn cancel(self: Box<Self>)
        -> BoxFuture<'static, Result<(), ProviderError>>;
}

pub struct ChatRequest<'a> {
    pub system: &'a str,
    pub user: &'a str,
}

pub trait ChatModel: Send + Sync {
    fn complete<'a>(&'a self, request: ChatRequest<'a>)
        -> BoxFuture<'a, Result<String, ProviderError>>;
}

pub struct StepContext {
    pub language: String,
    pub target: TargetInfo,
}

pub trait TextStep: Send + Sync {
    fn name(&self) -> &'static str;

    fn apply<'a>(&'a self, text: &'a str, context: &'a StepContext)
        -> BoxFuture<'a, Result<String, StepError>>;
}
```

The traits return `BoxFuture`. Thus they are dyn-compatible, and the settings can select a provider at run time.
Each returned future requires an allocation. Audio chunks and these futures are created outside the audio callback.
Vendor options, for example `reasoning_effort`, stay in the provider settings. They are not in the requests.

### Transcription lifecycle

- `start` opens a connection and applies the options before the first audio chunk.
- The returned session owns its connection and settings. It does not borrow the options or the `Transcriber`.
- One task sends chunks in order. Successful `send_audio` means the chunk was sent, not that transcription is complete.
- The adapter reads provider messages while it sends audio. Its receive task and buffers belong to the session.
- `finish` consumes the session, ends audio input, and waits for one complete transcript.
- `cancel` consumes the session and closes it without a transcript.
- Partial provider results stay inside the adapter. They never enter the text pipeline or insertion queue.
- Dropping the session, or its pending `start`, `finish`, or `cancel` future, releases local resources and stops owned tasks.
- Dropping `send_audio` stops that send operation. Cancellation also drops the session; Bloop never retries the interrupted chunk.
- The driver applies connection, inactivity, and completion limits. Cancellation must also work during any pending operation.
- Bloop does not reconnect or replay audio automatically. No adapter can replace streaming with a complete-file upload.

The driver checks the generation before accepting any result. The provider does not call the core or perform insertion.

## Platform ports

```rust
use secrecy::SecretString;

pub trait EventSink: Send + Sync {
    fn send(&self, event: Event);
}

pub trait AudioSink: Send {
    fn try_push(&mut self, samples: &[f32], sample_rate: u32)
        -> Result<(), AudioQueueError>;
}

pub trait AudioInput {
    fn start(&mut self, generation: Generation, sink: Box<dyn AudioSink>)
        -> Result<(), AppError>;
    fn stop(&mut self, generation: Generation) -> Result<CaptureSummary, AppError>;
    fn cancel(&mut self);
}

pub trait Overlay {
    fn show(&mut self, view: OrbView);
    fn close(&mut self);
}

pub trait TextOutput {
    fn record_target(&mut self, generation: Generation) -> TargetInfo;
    fn insert(&mut self, generation: Generation, text: FinalText);
    fn copy(&mut self, text: &FinalText);
}

pub trait Scheduler {
    fn after(&self, delay: Duration, event: Event);
}

pub trait SecretStore {
    fn get(&self, account: &str) -> Option<SecretString>;
}
```

- Main-thread ports have no `Send` bound. `EventSink` and `AudioSink` transfer data across threads.
- `AudioInput::start` starts capture without waiting for the provider connection.
- `AudioSink` receives mono samples at the native sample rate. The sample rate stays fixed during the capture.
- `try_push` copies borrowed samples into preallocated bounded storage. It does not allocate, block, or perform I/O.
- The worker receives samples from that queue, converts them incrementally, and calls `send_audio`.
- A full queue fails the capture. An unexpected closed queue also fails it. Expected closure during cancellation produces no second error.
- `TextOutput::insert` returns immediately. The adapter sends `Event::InsertDone` through the `EventSink` later.
- The audio adapter writes the level into a `LevelMeter`. The overlay reads the same `LevelMeter`.
- The app supplies the validated maximum duration when it builds the audio adapter. The duration does not change during a capture.
- At the sample limit, the audio adapter stops adding samples and sends `CaptureLimitReached` with the generation.
- `AudioInput::stop` closes the queue producer after the final samples and returns the capture summary.
- The worker drains accepted samples and flushes the converter before `finish`. Reaching the duration limit is not a failure.
- `AudioInput::cancel` stops capture. The driver also cancels the stream and discards queued samples for that generation.
- `SecretString` does not show its value in `Debug` output.

Phase 2 must extend the event and orb contracts to show a warning before the capture limit.
The warning must carry the capture generation. A warning for a finished capture has no effect.
The warning must keep recording active and must not create a job or change keyboard focus.

## Errors

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    Network,
    Timeout,
    Http(u16),
    Unauthorized,
    InvalidResponse,
}

#[derive(Debug)]
pub enum StepError {
    Provider(ProviderError),
    Rejected(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Microphone,
    Accessibility,
    InputMonitoring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioQueueError {
    Full,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppError {
    PermissionMissing(Permission),
    Microphone,
    AudioQueue(AudioQueueError),
    Provider(ProviderError),
}
```

`AudioQueueError` and `AppError::AudioQueue` are Phase 2 additions. The other error variants already exist.
Adapters map protocol and transport failures to these categories without recording response bodies or credentials.
`CaptureFailed` carries a failure during capture. `WorkFailed` carries a failure after capture.
If capture ends before a queued `CaptureFailed` arrives, the core applies it to the matching processing job.
Before reporting a failure, the driver stops audio and network work for that generation, even if capture already ended.
Neither event retries transcription or permits partial insertion.

## Pipeline

```rust
pub struct Pipeline {
    steps: Vec<Box<dyn TextStep>>,
}
```

`Pipeline::run(&self, generation: Generation, transcript: Transcript, context: StepContext) -> Event` is an `async fn`.

The driver starts this pipeline only after `finish` returns the complete transcript.
For short or silent captures, the driver cancels the stream and sends `NothingHeard` after job creation.
The limits for duration and level use `CaptureSummary`. They are not provider options.

1. If the complete transcript is empty, `run` returns `NothingHeard`.
2. `run` applies the steps in order. If a step fails, the text before that step continues to the next step.
3. `run` returns `TextReady` with the final text.

Phase 2 uses no text steps and returns the complete raw transcript.
Phase 3 adds correction. Raw fallback preserves a complete transcript when correction fails; it never accepts a failed STT session.

| Step | Behavior |
| --- | --- |
| `LlmRefine` | Sends the text to a `ChatModel` with a prompt. Skips a text with fewer than 4 words. Rejects a result shorter than 50% or longer than 160% of the input. |
| `Trim` | Removes line breaks at the start and at the end. |

## Required behavior

| Rule | Requirement |
| --- | --- |
| Generation | `DictationKeyDown` with the capture in `Idle` creates a new generation. The core ignores events for an unknown generation. |
| One capture | `DictationKeyDown` with the capture outside `Idle` has no effect. Running jobs do not block a new capture. |
| Job limit | If 3 jobs exist (processing, ready, or pasting) when `HoldElapsed` arrives, the capture does not start and the core shows the error view. A combination during `Pending` stays silent. |
| Short press | `DictationKeyUp` in `Pending` returns the capture to `Idle` and emits no command. A later `HoldElapsed` for that generation has no effect. |
| Combination | `OtherInputDuringHold` in `Pending` returns the capture to `Idle` and emits no command. In `Listening`, it emits `CancelCapture` and `CloseOrb`. |
| Key release | `DictationKeyUp` in `Listening` emits `RecordTarget`, `StopCaptureAndProcess`, and `CloseOrb`, and adds a job. |
| Capture limit | `CaptureLimitReached` for the active capture has the same effect as key release. Duplicate limit events and a later key release create no second job. |
| Order | The core emits `Insert` only for the first job in the queue. |
| Last result | The core keeps the text of the last `Insert` for `CopyLastResult`. |
| Escape | `Escape` has the same effect as `OtherInputDuringHold`. It has no effect on jobs. |
| Sleep | `SystemSleep` cancels the capture and all jobs. If a job already posted its paste command, `CancelWork` cannot remove the text. The core ignores its later `InsertDone`. |
| Nothing heard | `NothingHeard` removes the job. The core emits no orb command. |
| Error | `CaptureFailed` during `Listening` emits `CancelCapture` and shows the error view. For a processing job, `CaptureFailed` has the same effect as `WorkFailed`. This covers stop errors and failures that arrive after release. `WorkFailed` removes the job, and shows the error view only when no capture is active. |
| Fallback | `InsertDone` with `Fallback` shows the card with the text of that job. During a capture, the core keeps the card. When the capture ends, the core adds the card command after the commands that the other rules give. A newer card replaces a kept card. `CaptureFailed` during `Listening` and `SystemSleep` delete the kept card: the text stays in the clipboard and in the last result. |
| Paste | The paste adapter runs the target check. It posts no more than one paste command for each generation. |
| Step failure | The log records the step name and the reason. It does not record the text. |
| Privacy | `Transcript` and `FinalText` implement a redacting `Debug` that shows only the number of characters. Logs never contain audio or text. |

These requirements need unit tests. Trait signatures alone do not enforce them.

## Streaming rules

These rules apply from Phase 2. Test them with fake ports before implementing the network adapter.

- Each generation owns its audio queue and transcription task. Starting another capture must not replace an earlier job's task.
- `StartCapture` starts the microphone and opens the provider session asynchronously. Initial audio stays in a bounded queue during setup.
- Audio chunks retain capture order. The adapter must also bound its internal buffers.
- `StopCaptureAndProcess` signals the existing task to finish; it never opens a second transcription session.
- The task drains accepted audio, flushes conversion, and calls `finish` once. Release and duration limit events cannot finish it twice.
- `TextReady` requires a complete transcript and an existing job. Partial transcripts never create jobs.
- Escape, other input, and microphone loss stop the active capture and its stream. System sleep also cancels all jobs.
- A full queue, failed connection, or timeout ends the affected session with an error. It never triggers replay or incomplete insertion.
- Cancellation while connecting, sending, or finishing must release resources. Late results from cancelled generations have no effect.
- A failure before release returns capture to `Idle`. A failure after release removes its job and unblocks later jobs.
- Capture duration settings do not size queues. Test bounded audio memory at each supported duration.

Phase 2 tests must cover release and failure races, provider messages during capture, slow connections, full queues, and cancellation.
The existing state machine is the starting point. Update its events only when these tests show a need.
