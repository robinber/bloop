# Rust contracts

Status: revision 3. `bloop-core` implements the session state machine, the domain values, and the errors.
The other signatures are proposals without implementation.

All types and traits in this document are in `bloop-core`.
The [code structure](STRUCTURE.md) gives the file for each type.

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

pub struct RawAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub peak: f32,
}

pub struct AudioClip {
    pub wav: Arc<[u8]>,
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

`RawAudio` is the capture output in the native sample rate.
`AudioClip` is the 16 kHz mono WAV data that a `Transcriber` receives.
`FinalText::new` creates a final text and `FinalText::as_str` reads it. `Transcript` uses the same shape.

## AI ports

```rust
use std::{future::Future, pin::Pin};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct TranscriptionRequest<'a> {
    pub audio: &'a AudioClip,
    pub language: &'a str,
    pub vocabulary: &'a [String],
}

pub trait Transcriber: Send + Sync {
    fn transcribe<'a>(&'a self, request: TranscriptionRequest<'a>)
        -> BoxFuture<'a, Result<Transcript, ProviderError>>;
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
The cost is one allocation for each request. This cost is very small compared to a network request.
Vendor options, for example `reasoning_effort`, stay in the provider settings. They are not in the requests.

## Platform ports

```rust
use secrecy::SecretString;

pub trait EventSink: Send + Sync {
    fn send(&self, event: Event);
}

pub trait AudioInput {
    fn start(&mut self, generation: Generation) -> Result<(), AppError>;
    fn stop(&mut self, generation: Generation) -> Result<RawAudio, AppError>;
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

- Platform ports operate on the main thread. They have no `Send` bound.
- `TextOutput::insert` returns immediately. The adapter sends `Event::InsertDone` through the `EventSink` later.
- The audio adapter writes the level into a `LevelMeter`. The overlay reads the same `LevelMeter`.
- `SecretString` does not show its value in `Debug` output.

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
pub enum AppError {
    PermissionMissing(Permission),
    Microphone,
    Provider(ProviderError),
}
```

`ProviderError::is_retryable` returns true for `Network`, `Timeout`, `Http(429)`, and `Http(500..=599)`.

## Pipeline

```rust
pub struct PipelineSettings {
    pub language: String,
    pub vocabulary: Vec<String>,
    pub min_duration: Duration,
    pub min_peak: f32,
}

pub struct Pipeline {
    transcriber: Arc<dyn Transcriber>,
    steps: Vec<Box<dyn TextStep>>,
    settings: PipelineSettings,
}
```

`Pipeline::run(&self, generation: Generation, clip: AudioClip, context: StepContext) -> Event` is an `async fn`.

1. If the clip is too short or too quiet, `run` returns `NothingHeard`.
2. `run` calls the transcriber. After a retryable error, it calls the transcriber again one time.
3. If the transcription fails, `run` returns `WorkFailed`. If the transcript is empty, it returns `NothingHeard`.
4. `run` applies the steps in order. If a step fails, the text before that step continues to the next step.
5. `run` returns `TextReady` with the final text.

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
| Order | The core emits `Insert` only for the first job in the queue. |
| Last result | The core keeps the text of the last `Insert` for `CopyLastResult`. |
| Escape | `Escape` has the same effect as `OtherInputDuringHold`. It has no effect on jobs. |
| Sleep | `SystemSleep` cancels the capture and all jobs. If a job already posted its paste command, `CancelWork` cannot remove the text. The core ignores its later `InsertDone`. |
| Nothing heard | `NothingHeard` removes the job. The core emits no orb command. |
| Error | `CaptureFailed` during `Listening` emits `CancelCapture` and shows the error view. For a job that is processing, `CaptureFailed` has the same effect as `WorkFailed`: the driver sends it when `AudioInput::stop` fails. `WorkFailed` removes the job, and shows the error view only when no capture is active. |
| Fallback | `InsertDone` with `Fallback` shows the card with the text of that job. During a capture, the core keeps the card and shows it when the capture ends. A newer card replaces a kept card. `CaptureFailed` during `Listening` and `SystemSleep` delete the kept card: the text stays in the clipboard and in the last result. |
| Paste | The paste adapter runs the target check. It posts no more than one paste command for each generation. |
| Step failure | The log records the step name and the reason. It does not record the text. |
| Privacy | `Transcript` and `FinalText` implement a redacting `Debug` that shows only the number of characters. Logs never contain audio or text. |

These requirements need unit tests. Trait signatures alone do not enforce them.
