# Rust contracts

Status: revision 18 (2026-11-05). `bloop-core` implements the session state machine, the domain data, the error enums except `StepError`,
the transcription ports, the platform ports, and the Phase 2 pipeline.
The owner approved the audio and streaming signatures on 2026-11-01, and the `SecretStore` signature on 2026-11-02.
The text step and pipeline signatures are Phase 3 proposals, revised on 2026-11-05. The menu model is a Phase 4 proposal.
Bloop sets no maximum capture duration. Capture continues until key release, cancellation, or failure.

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
    CaptureLimitReached(Generation), // Legacy event; the driver does not send it. Phase 3 removes it.
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

#[derive(Debug, Clone, Default)]
pub struct LevelMeter(Arc<AtomicU32>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetInfo {
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
}

pub struct Transcript(String);

#[derive(Clone, PartialEq, Eq)]
pub struct FinalText(String);
```

`AudioChunk` contains mono signed 16-bit little-endian PCM at the sample rate of the transcription session, without a file header.
The worker creates the chunks at 16 kHz. Thus one chunk of 100 ms contains 1,600 samples.
`AudioChunk::length(NonZeroU32)` returns the number of samples in 100 ms at the given rate. Below 10 Hz, it returns 1.
It is the only source of the chunk length.
`AudioChunk::split(&[f32], NonZeroU32)` encodes samples at the given rate as consecutive chunks of this length, in order.
It is the only constructor. Thus a chunk is never empty and contains only whole samples.
Only the last chunk can be shorter than 100 ms.
The 100 ms length follows the Mistral examples, and it decreases the number of WebSocket messages. It is not a measured value.
A sample from -1 to 1 uses the full 16-bit range. Values outside that range saturate, and NaN encodes as 0.
`AudioChunk::as_bytes` exposes the bytes. Only the worker creates these chunks.
`CaptureSummary` contains the duration and peak level. It contains no recording.
`LevelMeter::set` stores a level from 0 to 1. It clamps other values, and NaN stores 0. `LevelMeter::get` reads the level.
`FinalText::new` creates a final text and `FinalText::as_str` reads it. `Transcript` uses the same shape.
`FinalText::from(transcript)` keeps the raw transcript as the final text, without a copy.
Audio and text types use a redacting `Debug` implementation.

## AI ports

```rust
use std::{future::Future, num::NonZeroU32, pin::Pin};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct TranscriptionOptions<'a> {
    pub language: &'a str,
    pub vocabulary: &'a [String],
    pub sample_rate: NonZeroU32,
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
}

pub struct ChatRequest<'a> {
    pub system: &'a str,
    pub user: &'a str,
    pub prediction: Option<&'a str>,
    pub max_tokens: NonZeroU32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    Complete,
    Truncated,
    Other,
}

pub struct ChatReply {
    pub text: String,
    pub finish: Finish,
}

pub trait ChatModel: Send + Sync {
    fn complete<'a>(&'a self, request: ChatRequest<'a>)
        -> BoxFuture<'a, Result<ChatReply, ProviderError>>;
}

pub struct StepContext {
    pub language: String,
}

pub enum StepOutcome {
    Applied(String),
    Skipped(SkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    TooFewWords,
}

pub trait TextStep: Send + Sync {
    fn name(&self) -> &'static str;

    fn apply<'a>(&'a self, text: &'a str, context: &'a StepContext)
        -> BoxFuture<'a, Result<StepOutcome, StepError>>;
}
```

The traits return `BoxFuture`. Thus they are dyn-compatible, and the settings can select a provider at run time.
Each returned future requires an allocation. Audio chunks and these futures are created outside the audio callback.
Vendor options, for example `reasoning_effort`, stay in the provider settings. They are not in the requests.
`prediction` is a hint: the expected reply is near this text. An adapter without this feature ignores it.
`max_tokens` limits the reply. The step that sends the request calculates it from the input length.
`Finish::Complete` means that the provider ended the reply normally (`stop` for Mistral).
`Truncated` means that the reply stopped at the token limit. A reply that is not `Complete` is a step failure.
The chat adapter logs the request duration, the token counts, and the finish reason. It never logs the text.
`ChatReply` and `StepOutcome` contain text. They implement a redacting `Debug`.
`StepContext` contains the language of the dictation. A step that needs the target application adds the target to `StepContext`.
The driver then gives the target to the running task at key release, because the task starts before `RecordTarget`.
A step does not write logs. It returns its outcome, and the pipeline adds it to the step report.

### Transcription lifecycle

- `start` opens a connection and applies the options before the first audio chunk.
- `sample_rate` is the rate of the audio chunks, in Hz. The worker sets 16 kHz. The adapter sends it to the provider without a change.
- The returned session owns its connection and settings. It does not borrow the options or the `Transcriber`.
- One task sends chunks in order. Successful `send_audio` means the chunk was sent, not that transcription is complete.
- The adapter reads the pending provider messages before each send and during `finish`. Its buffers belong to the session.
- `finish` consumes the session, ends audio input, and waits for one complete transcript.
- Cancellation drops the session. The Mistral protocol has no abort message: closing the connection cancels the session.
- Partial provider results stay inside the adapter. They never enter the text pipeline or insertion queue.
- Dropping the session, or its pending `start` or `finish` future, releases local resources and stops owned tasks.
- Dropping `send_audio` stops that send operation. Cancellation also drops the session; Bloop never retries the interrupted chunk.
- The worker applies the time limits: the connection limit to `start`, the inactivity limit to each `send_audio`,
  and the completion limit to `finish`. An expired limit fails the operation with `ProviderError::Timeout`.
  Cancellation must also work during any pending operation.
- Bloop does not reconnect or replay audio automatically. No adapter can replace streaming with a complete-file upload.

The core ignores a result for an unknown generation. The provider does not call the core or perform insertion.

## Platform ports

```rust
use std::num::NonZeroU32;

use secrecy::SecretString;

pub trait EventSink: Send + Sync {
    fn send(&self, event: Event);
}

pub trait AudioSink: Send {
    fn try_push(&mut self, samples: &[f32]) -> Result<(), AudioQueueError>;
}

pub trait AudioInput {
    fn start(&mut self, generation: Generation, sink: Box<dyn AudioSink>)
        -> Result<NonZeroU32, AppError>;
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
    fn cancel(&mut self, generation: Generation);
}

pub trait Scheduler {
    fn after(&self, delay: Duration, event: Event);
}

pub trait SecretStore {
    fn get(&self, account: &str) -> Result<SecretString, SecretError>;
}
```

- Main-thread ports have no `Send` bound. `EventSink` and `AudioSink` transfer data across threads.
- `AudioInput::start` starts capture without waiting for the provider connection.
- Without the microphone permission, macOS can supply silent samples instead of an error.
  Thus, before each capture, the audio adapter checks the permission.
  If the permission is missing, `AudioInput::start` returns `AppError::PermissionMissing(Permission::Microphone)`.
- `AudioInput::start` returns the native sample rate of the device, in Hz. The rate is not zero and stays fixed during the capture.
  The driver gives this rate to the worker.
  The worker converts the samples from this rate to 16 kHz mono PCM. It sets 16 kHz in `TranscriptionOptions`.
- `AudioSink` receives mono samples at the native sample rate.
- The worker creates the converter of `bloop-audio` before `start`. The audio callback does not convert.
  The converter removes the delay of its resampler. Thus the 16 kHz audio stays in time with the capture.
  For `n` samples at the native rate `r`, the converted capture contains `ceil(n × 16000 / r)` samples.
  At 16 kHz, the converter copies the samples without a change.
- A converter error is a bug. The worker logs it one time, without audio, and sends `CaptureFailed` with `AppError::Internal`.
- `try_push` copies borrowed samples into preallocated bounded storage. It does not allocate, block, or perform I/O.
- The audio callback sends no event and writes no log. It updates only atomic values: sample count, peak level, and overflow mark.
- The audio library calls an error callback outside the audio callback, except for a rare host time error on the audio thread.
  The previous rule applies only to the audio callback.
  For a device loss, the error callback sends `CaptureFailed` with `AppError::Microphone`. It writes no log.
- The audio library can report a device overrun on the real-time audio thread.
  Thus, for an overrun, the error callback only adds one to an atomic counter. `AudioInput::stop` writes the count to the log.
- When the queue is full, `try_push` returns `Full` and the queue keeps an overflow mark.
- After each queue read and its sends, the worker waits 20 ms. The callback does not wake it.
  The worker converts the samples incrementally to 16 kHz and calls `send_audio` for each chunk.
- During the capture, the worker sends only complete chunks of 100 ms at 16 kHz. It keeps the remaining samples for the next read.
  At the end of the capture, the worker flushes the converter and sends all remaining samples immediately.
  Thus the chunk length adds no delay after key release.
- During a provider operation, the worker checks the queue state at the same interval.
  A full queue or a lost microphone fails the capture without waiting for the provider.
- The worker reports the overflow mark as `AudioQueueError::Full`. Thus a full queue fails the capture.
- Each queue has an end mark. For `StopCaptureAndProcess`, the driver sets this mark, then calls `AudioInput::stop`.
  The worker reads the samples until the producer goes away, then flushes the converter and calls `finish`.
- If the producer goes away without the end mark, the worker reports `AudioQueueError::Closed`.
  Cancellation stops the worker first. Thus it produces no second error.
- The driver sets the end mark on the main thread.
  The worker detects the drop of the sink with an `Acquire` load that synchronizes with the drop.
  Thus the audio adapter must drop the sink on the main thread before `AudioInput::stop` returns.
  If another thread drops the sink, that thread must first synchronize with the main thread after the end mark.
  Without this order, the worker can report `Closed` for a normal end.
- `TextOutput::insert` returns immediately. The adapter sends `Event::InsertDone` through the `EventSink` later,
  after the end of its clipboard transaction.
- `TextOutput::cancel` forgets the target of a job that ends without `InsertDone`, and sends no `InsertDone` for it.
  The clipboard restore still follows a later read.
- The audio adapter writes the level into a `LevelMeter`. The overlay reads the same `LevelMeter`.
- `AudioInput::stop` stops the producer after the final samples and returns the capture summary.
- The worker drains accepted samples and flushes the converter before `finish`.
- `AudioInput::cancel` stops capture. The driver also cancels the stream and discards queued samples for that generation.
- `SecretString` does not show its value in `Debug` output.
- `SecretStore::get` returns `NotFound` when the store has no key for the account.
  It returns `Unavailable` when the store has the key but the read failed, for example because the user denied access.
  The user fixes each case differently. Thus the menu shows a different line for each.
- Bloop reads each API key one time at start, on the main thread, before the run loop.
  macOS can show an access dialog and block that thread until the user answers. A new key needs a restart of Bloop.

## Menu model

Phase 4 proposal. Review these types before the menu adapter.
These plain data types go in `ports/platform.rs`. `bloop-app` builds the model, and `bloop-macos` renders it.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusIcon {
    Ready,
    Attention,
}

pub struct MenuModel {
    pub icon: StatusIcon,
    pub items: Vec<MenuItem>,
}

pub enum MenuItem {
    Status { title: String, action: Option<ActionId> },
    Action { title: String, action: ActionId, enabled: bool },
    Toggle { title: String, action: ActionId, checked: bool, enabled: bool },
    Choice { title: String, options: Vec<ChoiceOption> },
    Separator,
}

pub struct ChoiceOption {
    pub title: String,
    pub action: ActionId,
    pub selected: bool,
}

pub trait MenuSource {
    fn model(&self) -> MenuModel;
    fn activate(&self, action: ActionId);
}

pub trait StatusItem {
    fn set_icon(&mut self, icon: StatusIcon);
}
```

- `bloop-macos` calls `MenuSource::model` each time the menu opens, and rebuilds the items.
- A `Choice` is a submenu with one selected option.
- An `ActionId` is valid only in the model that contains it. `bloop-app` ignores an unknown identifier.
- `MenuSource` and `StatusItem` run on the main thread. They have no `Send` bound.
- AppKit calls `MenuSource` from Objective-C. Thus its methods must not panic.
- When the status changes, `bloop-app` calls `StatusItem::set_icon`. The menu content changes at the next open.

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
    Rejected(Rejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    Incomplete,
    TooShort,
    TooLong,
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
pub enum SecretError {
    NotFound,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppError {
    PermissionMissing(Permission),
    Microphone,
    AudioQueue(AudioQueueError),
    Provider(ProviderError),
    Internal,
}
```

`StepError` and `Rejection` are Phase 3 additions. The other error types exist.
Adapters map protocol and transport failures to these categories without recording response bodies or credentials.
A WebSocket handshake error contains the response headers and body. The adapter maps it to `ProviderError` before any log.
The adapter can log the code and message of a provider `error` event, because they contain no transcript text and no credential.
`tungstenite` writes the handshake request with the API key to the `log` crate at trace level. Bloop installs no `log` bridge.
A future bridge must turn off the `tungstenite` target.
A missing API key in Keychain is `ProviderError::Unauthorized`. The menu status also shows it at start.
`CaptureFailed` carries a failure during capture. `WorkFailed` carries a failure after capture.
If capture ends before a queued `CaptureFailed` arrives, the core applies it to the matching processing job.
If the transcription task panics, the worker sends `CaptureFailed` with `AppError::Internal`.
The core applies this event to the capture or to the processing job of that generation.
Before reporting a failure, the driver stops audio and network work for that generation, even if capture already ended.
Neither event retries transcription or permits partial insertion.

## Pipeline

```rust
pub struct Pipeline {
    steps: Vec<Arc<dyn TextStep>>,
}

pub struct StepReport {
    pub step: &'static str,
    pub result: StepResult,
}

pub enum StepResult {
    Applied,
    Skipped(SkipReason),
    Failed(StepError),
}

pub struct PipelineOutput {
    pub event: Event,
    pub report: Vec<StepReport>,
}
```

`Pipeline::run(&self, generation: Generation, transcript: Transcript, context: &StepContext) -> PipelineOutput` is an `async fn`.
Phase 2 has no text steps and no `Pipeline` type. The function `pipeline::run(generation, transcript) -> Event` does steps 1 and 3.
In Phase 3, `bloop-app` captures the correction toggle for each generation and selects its step list.
The toggle is off at application start. Changes affect only subsequent generations.
Phase 4 builds the list from the full settings snapshot. The steps share the chat client through `Arc`.

The worker runs this pipeline only after `finish` returns the complete transcript.
The worker logs the step report. From Phase 4, a `ProviderError::Unauthorized` in the report goes to the menu status.
The driver does not check the duration or the level of a capture. For silence, the provider returns an empty transcript,
and step 1 returns `NothingHeard`. The driver writes the `CaptureSummary` to the log only.

1. If the complete transcript is empty or contains only whitespace, `run` returns `NothingHeard` and an empty report.
2. `run` applies the steps in order. If a step skips or fails, the text before that step continues to the next step.
3. `run` returns `TextReady` with the final text.

Without steps, the final text is the raw transcript. This is the path when the correction is off.
Phase 2 returns the complete raw transcript. Phase 3 adds the optional correction. Raw fallback preserves a complete transcript when correction fails; it never accepts a failed STT session.

| Step | Behavior |
| --- | --- |
| `LlmRefine` | Sends the prompt and the text to a `ChatModel`. Rejects incomplete or empty replies. Prediction, short-text skipping, and length ratio checks depend on the evaluation below. |
| `Typography` | Replaces a non-breaking space before `?`, `!`, `:`, `;`, and `»`, and after `«`, with a normal space. Replaces `’` with `'`. Does not change line breaks. |
| `Trim` | Removes line breaks at the start and at the end. |

When the correction is on, the step list is `LlmRefine`, `Typography`, `Trim`. When it is off, the list is empty.

The first evaluation uses one model and `natural-light`, without predicted output.
The skip below 4 words and the 50% to 160% character ratio are candidate heuristics, not meaning checks.
Select or remove them against the test set before daily use. Expected spoken corrections must pass.
For example, "à 2 heures, non, 3 heures" to "à 3 heures" has a 40% ratio.
The correction timeout also needs evaluation: a blocked first job delays subsequent insertions.
Tests must cover timeout to raw fallback, cancellation during correction, and corrections that complete in reverse order.

## Required behavior

| Rule | Requirement |
| --- | --- |
| Generation | `DictationKeyDown` with the capture in `Idle` creates a new generation. The core ignores events for an unknown generation. |
| One capture | `DictationKeyDown` with the capture outside `Idle` has no effect. Running jobs do not block a new capture. |
| Job limit | If 3 jobs exist (processing, ready, or pasting) when `HoldElapsed` arrives, the capture does not start and the core shows the error view. A combination during `Pending` stays silent. |
| Short press | `DictationKeyUp` in `Pending` returns the capture to `Idle` and emits no command. A later `HoldElapsed` for that generation has no effect. |
| Combination | `OtherInputDuringHold` in `Pending` returns the capture to `Idle` and emits no command. In `Listening`, it emits `CancelCapture` and `CloseOrb`. |
| Key release | `DictationKeyUp` in `Listening` emits `RecordTarget`, `StopCaptureAndProcess`, and `CloseOrb`, and adds a job. |
| Order | The core emits `Insert` only for the first job in the queue. |
| Last result | The core keeps the text of the last `Insert` for `CopyLastResult`. |
| Escape | `Escape` has the same effect as `OtherInputDuringHold`. It has no effect on jobs. |
| Sleep | `SystemSleep` cancels the capture and all jobs. If a job already posted its paste command, `CancelWork` cannot remove the text. The core ignores its later `InsertDone`. |
| Nothing heard | `NothingHeard` removes the job and emits `CancelWork`. The core emits no orb command. |
| Error | `CaptureFailed` during `Listening` emits `CancelCapture` and shows the error view. For a processing job, `CaptureFailed` has the same effect as `WorkFailed`. This covers stop errors and failures that arrive after release. `WorkFailed` removes the job, emits `CancelWork`, and shows the error view only when no capture is active. |
| Fallback | `InsertDone` with `Fallback` shows the card with the text of that job. During a capture, the core keeps the card. When the capture ends, the core adds the card command after the commands that the other rules give. A newer card replaces a kept card. `CaptureFailed` during `Listening` and `SystemSleep` delete the kept card: the text stays in the clipboard and in the last result. |
| Paste | The paste adapter runs the target check. It posts no more than one paste command for each generation. It reports `Landed` only after the target reads the text. |
| Step failure | The log records the step name and the reason. It does not record the text. |
| Settings | Phase 3 captures the correction toggle when the core creates a generation. Phase 4 captures the full settings snapshot. Changes never affect an active capture or a running job. |
| Privacy | `Transcript` and `FinalText` implement a redacting `Debug` that shows only the number of characters. Logs never contain audio or text. |

These requirements need unit tests. Trait signatures alone do not enforce them.

## Streaming rules

These rules apply from Phase 2. Test them with fake ports before implementing the network adapter.

- Each generation owns its audio queue and transcription task. Starting another capture must not replace an earlier job's task.
- `StartCapture` starts the microphone and opens the provider session asynchronously. Initial audio stays in a bounded queue during setup.
- Audio chunks retain capture order. The adapter must also bound its internal buffers.
- `StopCaptureAndProcess` signals the existing task to finish; it never opens a second transcription session.
- `CancelCapture` carries no generation. The driver keeps the generation of the active capture from `StartCapture`.
- `CancelWork` stops the task of its generation and calls `TextOutput::cancel`.
- The task drains accepted audio, flushes conversion, and calls `finish` once. A repeated key release cannot finish it twice.
- `TextReady` requires a complete transcript and an existing job. Partial transcripts never create jobs.
- Escape, other input, and microphone loss stop the active capture and its stream. System sleep also cancels all jobs.
- A full queue, failed connection, or timeout ends the affected session with an error. It never triggers replay or incomplete insertion.
- Cancellation while connecting, sending, or finishing must release resources. Late results from cancelled generations have no effect.
- A failure before release returns capture to `Idle`. A failure after release removes its job and unblocks later jobs.
- Audio queue capacity does not grow with capture duration. Check bounded audio memory during long dictations.

Phase 2 tests must cover release and failure races, provider messages during capture, slow connections, full queues, and cancellation.
The existing state machine is the starting point. Update its events only when these tests show a need.
