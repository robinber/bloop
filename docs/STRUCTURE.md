# Code structure

Status: Revision 15 (2026-11-05). The owner approved the workspace on 2026-10-31. Phase 1 is complete. Phase 2 is in progress.
`bloop-core` contains the streaming types and ports. `bloop-audio` contains the sample queue, the microphone capture, and the 16 kHz converter.
`bloop-app` contains the worker.
The driver starts the microphone capture and the worker. The worker streams the audio to Voxtral, and the driver pastes the complete raw transcript.
At start, `bloop-macos` reads the API key from Keychain and checks the microphone permission. `bloop-app` gives the key to the adapter.
When the system is about to sleep, `bloop-macos` sends `SystemSleep` to the core.
`bloop-providers` contains the Voxtral transcription adapter in the default `mistral` feature.

## Goals

- Each part has one responsibility and a small interface.
- A new provider or a new text feature needs changes in one crate only.
- The compiler enforces the boundaries between the parts.
- Tests can examine the core logic without macOS, the network, or a microphone.

## Workspace

Bloop is one Cargo workspace with five crates. All crates use the Rust 2024 edition.

```text
bloop/
├── Cargo.toml                 Workspace, shared dependency versions, shared lints
├── crates/
│   ├── bloop-core/            Domain types, state machine, pipeline, text steps (Phase 3), ports
│   ├── bloop-providers/       AI providers: one module for each vendor
│   ├── bloop-audio/           Microphone capture and incremental PCM conversion
│   ├── bloop-macos/           AppKit, event tap, orb, paste, menu, Keychain
│   └── bloop-app/             Executable: settings (Phase 2), construction, driver
├── prompts/
│   └── natural-clean-v1.md    Correction prompt under version control (Phase 3)
├── bundle/
│   └── Info.plist             Bundle metadata; LSUIElement for accessory mode
├── scripts/
│   └── bundle.sh              Builds and signs Bloop.app
└── docs/
```

## Dependency rules

```text
                 bloop-app
               /     |     \
     bloop-macos bloop-audio bloop-providers
               \     |     /
                 bloop-core
```

1. All dependencies point to `bloop-core`. `bloop-core` depends on no other Bloop crate.
2. `bloop-macos`, `bloop-audio`, and `bloop-providers` do not depend on each other.
3. Only `bloop-app` knows all crates. It is the only place that builds objects from the settings.
4. Only `bloop-macos` contains `unsafe` code and `objc2`.
5. Only `bloop-providers` contains HTTP and WebSocket code and vendor names.
6. `bloop-core` has no I/O, no async runtime, and no platform API. It uses only one small pure crate: `secrecy`.

Cargo enforces rules 1 to 5, because a crate cannot use a crate that is not in its dependencies.
All crates inherit the workspace lints. The workspace sets `unsafe_code = "deny"`.
In `bloop-macos`, each module that needs `unsafe` carries a scoped `#![expect(unsafe_code, reason = "...")]`.
The other crates add `#![forbid(unsafe_code)]` at their root.

## Design decisions

| Decision | Reason | Rejected alternative |
| --- | --- | --- |
| One workspace with five crates | Cargo enforces the boundaries. A change compiles only the crates that it affects. | One crate with modules: the boundaries depend on discipline only. |
| Ports in `bloop-core`, adapters in other crates | Tests can examine the core without macOS, the network, or a microphone. An adapter can change without a change to the core. | Direct calls from the state machine to AppKit and HTTP. |
| Pure state machine: events in, commands out | Tests are deterministic. The core has no threads, no clock, and no callbacks. | Session state in callbacks across many modules. |
| Capabilities separate from text steps | A provider implements capabilities. A text feature uses capabilities. Thus a new provider does not change the features, and a new feature does not change the providers. | One trait for each combination of feature and vendor. |
| `BoxFuture` in the AI ports | The traits are dyn-compatible. Thus the settings select the provider at run time. | `impl Future` in traits: not dyn-compatible. The `async-trait` macro: same result, with an additional macro. |
| One streaming STT contract | Audio reaches the provider during capture. All speech adapters use the same lifecycle. | File transcription before streaming, or a permanent upload fallback. |
| Tagged enums for provider settings | Each vendor declares its own typed settings. An unknown provider name stops the start. | An untyped map of keys and values. |
| One provider crate with one Cargo feature for each vendor | Little repeated configuration. A vendor that is not selected does not compile. | One crate for each vendor. Use it later if one vendor module becomes large. |
| Main-thread ports without a `Send` bound | AppKit objects stay on the main thread. `EventSink` and `AudioSink` cross thread boundaries. | `Send` bounds that need wrappers around AppKit objects. |
| `unsafe` code only in `bloop-macos` | The review of unsafe code covers one crate only. | `unsafe` code in each crate that calls a system API. |

These items are outside the structure: plugins loaded at run time, and support for Windows or Linux beyond the ports.

## Crates

### bloop-core

```text
src/
├── lib.rs
├── domain.rs          Generation, AudioChunk, CaptureSummary, LevelMeter, TargetInfo, Transcript, FinalText
├── session.rs         Core, Capture, Job, Event, Command, OrbView, InsertOutcome, FallbackReason
├── pipeline.rs        Complete transcript to result event; Phase 3 adds the text steps and raw fallback
├── steps/             Text steps (Phase 3)
│   ├── mod.rs
│   ├── refine.rs      LlmRefine: correction through a ChatModel and a prompt
│   └── trim.rs        Trim: removes line breaks at the start and at the end
├── ports/
│   ├── ai.rs          Transcriber, TranscriptionSession, BoxFuture; Phase 3 adds ChatModel and TextStep
│   └── platform.rs    EventSink, AudioSink, AudioInput, Overlay, TextOutput, Scheduler, SecretStore
└── error.rs           AppError, ProviderError, AudioQueueError, SecretError, Permission; Phase 3 adds StepError
```

### bloop-providers

```text
src/
├── lib.rs             TranscriberConfig and build_transcriber (Phase 2), ChatConfig and build_chat_model (Phase 3)
├── http.rs            Reused correction client, time limits, HTTP error conversion to ProviderError (Phase 3)
└── mistral/
    ├── mod.rs         Mistral settings and Keychain account ("mistral"); bloop-app supplies the API key
    ├── transcribe.rs  Voxtral WebSocket: impl Transcriber and TranscriptionSession
    └── chat.rs        Mistral Small: impl ChatModel (Phase 3)
```

Each vendor module has its own Cargo feature. The `mistral` feature is the default.

### bloop-audio

```text
src/
├── lib.rs
├── capture.rs         cpal stream, mono mix, LevelMeter update: impl AudioInput
├── buffer.rs          Preallocated bounded sample queue with end and overflow marks: impl AudioSink
└── convert.rs         Converter: incremental resampling to 16 kHz mono; flush at capture end
```

`convert.rs` uses the FFT resampler of `rubato`. This resampler filters the audio before it lowers the rate. Thus it prevents aliasing.
On 2026-11-05, the owner selected 16 kHz for all microphones, because Voxtral returned no text for AirPods audio at 24 kHz.

### bloop-macos

```text
src/
├── lib.rs
├── app.rs             NSApplication setup, accessory mode, run loop start
├── events.rs          MainSink: impl EventSink through the main queue
├── keys.rs            Event tap for the dictation key, other input, and Escape
├── orb/
│   ├── mod.rs         OrbPanel: impl Overlay
│   ├── panel.rs       NSPanel configuration and position
│   └── layers.rs      Pill, application icon, bars, animations
├── paste.rs           Target check, pasteboard, Command-V: impl TextOutput
│   └── clipboard.rs   Clipboard save, restore, and text write
├── menu.rs            Status item and menu
├── keychain.rs        impl SecretStore
├── scheduler.rs       Main queue timers: impl Scheduler
├── sleep.rs           Sleep observer: sends SystemSleep to the core
└── permissions.rs     Permission checks and System Settings links
```

### bloop-app

```text
src/
├── main.rs            Reads the API key, starts Tokio, builds objects, starts the AppKit run loop
├── config.rs          TOML settings with default values (Phase 2)
├── driver.rs          Driver: events to the core, commands to the ports
├── worker.rs          Transcription task of one generation: queue, conversion, provider session, result event
└── logging.rs         tracing output to ~/Library/Logs/Bloop/bloop.log
```

## Runtime flow

```text
event tap -> EventSink -> Driver -> Core::handle -> commands
                            |
                  StartCapture(generation)
                            |
          microphone -> bounded queue -> converter -> provider stream
                            |
               StopCaptureAndProcess(generation)
                            |
          stop capture -> drain queue -> flush converter -> finish stream
                            |
          complete Transcript -> Pipeline::run -> EventSink -> Driver
```

- The driver is on the main thread. It owns `Core` and the platform ports.
- For `StartCapture`, the driver connects the audio adapter to a bounded queue and starts the transcription task.
- Microphone capture starts while the connection opens. Initial samples stay in the queue until the stream is ready.
- The worker converts and sends chunks in order. Network waits never block the audio callback or the main thread.
- For `StopCaptureAndProcess`, the driver sets the end mark of the queue. Then it stops the producer and gets a `CaptureSummary`.
- The existing task drains the queue, flushes the converter, and finishes the stream once.
- The complete transcript enters the text pipeline. Partial transcripts never produce `TextReady`.
- The worker runs as a work task and a report task. The report task sends the result event.
  The report task also converts a panic of the work task into `CaptureFailed` with `AppError::Internal`.
- The worker finishes the stream only after the end mark. Thus its `TextReady` reaches the core after the core creates the job.
- The driver keeps task handles by generation. `CancelCapture` stops the active capture and its stream; `CancelWork` stops the specified job and forgets its paste target.
- `CancelCapture` carries no generation. Thus the driver keeps the generation of the active capture from `StartCapture`.
- After each event, the driver tells the event tap if a capture is active. The tap uses this value for Escape.
- Bloop sets no maximum capture duration. The driver schedules no timer to end capture.

## Settings and provider selection

Phase 2 adds these settings with `config.rs`. Phase 3 adds the `[chat]` and `[refine]` tables.

```toml
[dictation]
key = "right_command"
hold_ms = 220
language = "fr"
vocabulary = ["Rust", "Tokio", "objc2"]

[transcriber]
provider = "mistral"
model = "voxtral-mini-transcribe-realtime-3-0"
target_streaming_delay_ms = 480

[chat]
provider = "mistral"
model = "mistral-small-2603"
reasoning_effort = "none"

[refine]
enabled = true
prompt = "natural-clean-v1"
```

The `provider` field selects a variant of a tagged enum in `bloop-providers`:

```rust
#[derive(serde::Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum TranscriberConfig {
    Mistral(mistral::TranscribeConfig),
}
```

Each vendor declares its own settings.
If the provider name is unknown, Bloop stops at start with a clear error.

## Extension procedures

### Add a provider

1. Add a module in `bloop-providers/src/` with its own Cargo feature.
2. For STT, implement `Transcriber` and `TranscriptionSession`. For correction, implement `ChatModel` (Phase 3).
3. Add a variant to `TranscriberConfig` (Phase 2) or `ChatConfig` (Phase 3). Add one match arm in the build function.
4. Store the API key in Keychain. Use the vendor name as the account.
5. Set `provider` in `config.toml` (Phase 2).

The other crates do not change.
Every STT adapter sends audio during capture. It must not collect a complete recording before sending it.
Future DGX-hosted and Microsoft adapters use the same streaming contract. Their transport stays inside the provider crate.

### Add a text feature

Examples: French typography, word replacements, translation, one style for each application.
This procedure applies from Phase 3.

1. Add a `TextStep` in `bloop-core/src/steps/`. A step that needs a model receives an `Arc<dyn ChatModel>`.
2. Add unit tests for the step.
3. Add the step to the step list in `bloop-app`.

`StepContext` contains the target application. Thus a style for each application needs no new port.

### Add a platform function

Examples: start and stop sounds, text adaptation at the cursor.

1. Add a method or a port in `bloop-core/src/ports/platform.rs`.
2. Implement it in `bloop-macos`.
3. If the state machine must decide when to use it, add a `Command`.

### Streaming lifecycle

Phase 2 implements the [streaming rules](CONTRACTS.md#streaming-rules) with the first audio adapter.
There is no separate file transcription path or later migration.

1. Test the lifecycle with fake audio and provider ports before the adapters.
2. Give each generation its own bounded queue and transcription task.
3. Test failures during capture and after release. Both paths must release resources and unblock later jobs.
4. Test cancellation while connecting, sending, and finishing. Late results must not produce insertion.
5. Keep the existing generation and insertion rules. Extend events only when these tests show a need.

## Tests

| Crate | Tests |
| --- | --- |
| `bloop-core` | State machine: event sequences, expected commands, and stale results. Pipeline. Steps with fake providers (Phase 3). |
| `bloop-providers` | Protocol messages, ordered audio, final response, and cancellation with a fake server. Real API tests run manually. |
| `bloop-audio` | Incremental conversion across chunk boundaries, final flush, queue limits, and level calculation with generated signals |
| `bloop-app` | Driver and worker with fake ports: slow connection, full queue, release, failure, and cancellation during the capture, a send, or `finish`. A cancelled task sends no event. |
| `bloop-macos` | Unit tests of the logic without macOS types: key state, target list, clipboard transaction, bar heights, and orb view changes. Tests of the clipboard save, restore, and text write on a private `NSPasteboard`. Manual acceptance checks from the [roadmap](ROADMAP.md) for the other macOS calls. |

## Shared rules

- `[workspace.dependencies]` keeps one version for each dependency.
- The workspace lints are the rust-strict starter policy, with `undocumented_unsafe_blocks` and
  `multiple_unsafe_ops_per_block` denied. `AGENTS.md` records the lint floor.
- `rustfmt.toml` sets `style_edition = "2024"`. `Cargo.lock` is under version control.
- The error enums in `bloop-core` are plain data. Events and port results carry them. `bloop-app` writes them to the log with `Debug`.
  If a boundary needs `Display`, the crate that owns the error type adds it.
  Only `bloop-app` uses `anyhow`, for start errors.
- All crates use `tracing` for logs.
- Types that contain text or audio implement a redacting `Debug` that never shows the content.
- `LevelMeter` uses `Relaxed` ordering. The level is one independent value: no other data is published through it.

## Implementation plan

Each [roadmap](ROADMAP.md) phase adds code from the key press to the paste.
Thus the application operates at the end of each phase.

| Phase | Crates and files |
| --- | --- |
| 1: Key, orb, and paste | Root: `Cargo.toml` with the workspace lints, `rustfmt.toml`, `clippy.toml`, `Cargo.lock`. `bloop-core`: `domain.rs`, `session.rs`, `ports/platform.rs`, `error.rs`, with unit tests. `bloop-macos`: `app.rs`, `events.rs`, `keys.rs`, `orb/`, `paste.rs`, `menu.rs`, `scheduler.rs`, `permissions.rs`. `bloop-app`: `main.rs`, `driver.rs`, `logging.rs`. `scripts/bundle.sh`. |
| 2: Microphone and streaming | `bloop-core`: audio and streaming contracts, `pipeline.rs` without steps. `bloop-audio`: `capture.rs`, `buffer.rs`, `convert.rs`. `bloop-providers`: `mistral/transcribe.rs`. `bloop-app`: `worker.rs`, `config.rs`, stream ownership and lifecycle tests. `bloop-macos`: `keychain.rs`, the microphone check in `permissions.rs`, `sleep.rs`. |
| 3: Correction | `bloop-core`: `ChatModel` and `TextStep` in `ports/ai.rs`, `steps/refine.rs`, `steps/trim.rs`. `bloop-providers`: `http.rs`, `mistral/chat.rs`. `prompts/natural-clean-v1.md`. |

Until `config.rs` exists, `bloop-app` keeps these temporary constants in `main.rs`:

- The hold threshold: `HOLD`, 220 ms.
- The transcription settings: model `voxtral-mini-transcribe-realtime-3-0`, `target_streaming_delay_ms` 480,
  language `fr`, and an empty word list.

Phase 2 replaces these constants with `config.rs`.

## Work method

Agents follow `AGENTS.md` and the rust-strict skill (`.claude/skills/rust-strict`, v2.0.0).

1. Write the types and the unit tests in `bloop-core` before the adapter.
2. Add a fake implementation for tests at the same time as each new port.
3. Write the adapter. Then connect it in the driver.
4. Do the manual acceptance checks of the phase on the reference Mac.
5. Before each commit, run the gates in `AGENTS.md`: `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.

- Add each new dependency to `[workspace.dependencies]` first.
- If a decision changes, update this document and the [contracts](CONTRACTS.md) in the same commit.
- Record measured values separately from targets.
