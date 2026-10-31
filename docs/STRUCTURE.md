# Code structure

Status: Approved by the owner on 2026-10-31, revision 1. No code exists.

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
│   ├── bloop-core/            Domain types, state machine, pipeline, text steps, ports
│   ├── bloop-providers/       AI providers: one module for each vendor
│   ├── bloop-audio/           Microphone capture and WAV conversion
│   ├── bloop-macos/           AppKit, event tap, orb, paste, menu, Keychain
│   └── bloop-app/             Executable: settings, construction, driver
├── prompts/
│   └── natural-clean-v1.md    Correction prompt under version control
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
5. Only `bloop-providers` contains HTTP code and vendor names.
6. `bloop-core` has no I/O, no async runtime, and no platform API. It uses only small pure crates (`thiserror`, `secrecy`).

Cargo enforces rules 1 to 5, because a crate cannot use a crate that is not in its dependencies.
The workspace lints set `unsafe_code = "forbid"`. All crates use these lints, except `bloop-macos`.

## Design decisions

| Decision | Reason | Rejected alternative |
| --- | --- | --- |
| One workspace with five crates | Cargo enforces the boundaries. A change compiles only the crates that it affects. | One crate with modules: the boundaries depend on discipline only. |
| Ports in `bloop-core`, adapters in other crates | Tests can examine the core without macOS, the network, or a microphone. An adapter can change without a change to the core. | Direct calls from the state machine to AppKit and HTTP. |
| Pure state machine: events in, commands out | Tests are deterministic. The core has no threads, no clock, and no callbacks. | Session state in callbacks across many modules. |
| Capabilities separate from text steps | A provider implements capabilities. A text feature uses capabilities. Thus a new provider does not change the features, and a new feature does not change the providers. | One trait for each combination of feature and vendor. |
| `BoxFuture` in the AI ports | The traits are dyn-compatible. Thus the settings select the provider at run time. | `impl Future` in traits: not dyn-compatible. The `async-trait` macro: same result, with an additional macro. |
| Tagged enums for provider settings | Each vendor declares its own typed settings. An unknown provider name stops the start. | An untyped map of keys and values. |
| One provider crate with one Cargo feature for each vendor | Little repeated configuration. A vendor that is not selected does not compile. | One crate for each vendor. Use it later if one vendor module becomes large. |
| Platform ports without a `Send` bound | AppKit objects stay on the main thread, as AppKit requires. | `Send` bounds that need wrappers around AppKit objects. |
| `unsafe` code only in `bloop-macos` | The review of unsafe code covers one crate only. | `unsafe` code in each crate that calls a system API. |

These items are outside the structure: plugins loaded at run time, and support for Windows or Linux beyond the ports.

## Crates

### bloop-core

```text
src/
├── lib.rs
├── domain.rs          Generation, RawAudio, AudioClip, LevelMeter, TargetInfo, Transcript, FinalText
├── session.rs         Core, Capture, Job, Event, Command, OrbView
├── pipeline.rs        Pipeline: transcription, text steps, retry and fallback rules
├── steps/
│   ├── mod.rs
│   ├── refine.rs      LlmRefine: correction through a ChatModel and a prompt
│   └── trim.rs        Trim: removes line breaks at the start and at the end
├── ports/
│   ├── ai.rs          Transcriber, ChatModel, TextStep, BoxFuture
│   └── platform.rs    EventSink, AudioInput, Overlay, TextOutput, Scheduler, SecretStore
└── error.rs           AppError, ProviderError, StepError
```

### bloop-providers

```text
src/
├── lib.rs             TranscriberConfig, ChatConfig, build_transcriber, build_chat_model
├── http.rs            Shared HTTP client, time limits, HTTP error conversion to ProviderError
└── mistral/
    ├── mod.rs         Mistral settings; API key from SecretStore (account "mistral")
    ├── transcribe.rs  Voxtral: impl Transcriber
    └── chat.rs        Mistral Small: impl ChatModel
```

Each vendor module has its own Cargo feature. The `mistral` feature is the default.

### bloop-audio

```text
src/
├── lib.rs
├── capture.rs         cpal stream, bounded buffer, LevelMeter update: impl AudioInput
└── convert.rs         to_wav_16k: mono mix, resample, WAV encode
```

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
├── menu.rs            Status item and menu
├── keychain.rs        impl SecretStore
├── scheduler.rs       Run loop timers: impl Scheduler
└── permissions.rs     Permission checks and System Settings links
```

### bloop-app

```text
src/
├── main.rs            Reads settings, starts Tokio, builds objects, starts the AppKit run loop
├── config.rs          TOML settings with default values
├── driver.rs          Driver: events to the core, commands to the ports
└── logging.rs         tracing output to ~/Library/Logs/Bloop/bloop.log
bundle/
└── Info.plist
```

## Runtime flow

```text
event tap ──> EventSink ──> Driver ──> Core::handle ──> commands
                              ^                            │
                              │                            v
                              │        AudioInput, Overlay, TextOutput, Scheduler
                              │                            │
          result event ───────┘                            │ StopCaptureAndProcess
                ^                                          v
                └──────── Tokio task: to_wav_16k, then Pipeline::run
```

- The driver is on the main thread. It owns `Core` and the platform ports.
- For `StopCaptureAndProcess`, the driver gets the `RawAudio` and starts a Tokio task.
- The task converts the audio, runs the pipeline, and sends the result event through the `EventSink`.
- The driver keeps the task handle of each job. `CancelWork` stops the related task.
- After each event, the driver tells the event tap if a capture is active. The tap uses this value for Escape.

## Settings and provider selection

```toml
[dictation]
key = "right_command"
hold_ms = 220
language = "fr"
vocabulary = ["Rust", "Tokio", "objc2"]

[transcriber]
provider = "mistral"
model = "voxtral-small-transcribe-3-0"

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
2. Implement `Transcriber`, `ChatModel`, or both.
3. Add a variant to `TranscriberConfig` or `ChatConfig`. Add one match arm in the build function.
4. Store the API key in Keychain. Use the vendor name as the account.
5. Set `provider` in `config.toml`.

The other crates do not change.

### Add a text feature

Examples: French typography, word replacements, translation, one style for each application.

1. Add a `TextStep` in `bloop-core/src/steps/`. A step that needs a model receives an `Arc<dyn ChatModel>`.
2. Add unit tests for the step.
3. Add the step to the step list in `bloop-app`.

`StepContext` contains the target application. Thus a style for each application needs no new port.

### Add a platform function

Examples: start and stop sounds, text adaptation at the cursor.

1. Add a method or a port in `bloop-core/src/ports/platform.rs`.
2. Implement it in `bloop-macos`.
3. If the state machine must decide when to use it, add a `Command`.

### Real-time transcription

This is later work. It needs a port for a stream of audio parts and a streaming transcriber.
The state machine and the text steps do not change.

## Tests

| Crate | Tests |
| --- | --- |
| `bloop-core` | State machine: event sequences and expected commands. Pipeline and steps with fake providers. |
| `bloop-providers` | Request construction and response parsing with recorded JSON. Real API tests use `#[ignore]` and run manually. |
| `bloop-audio` | WAV conversion and level calculation with generated signals |
| `bloop-app` | Driver with fake ports |
| `bloop-macos` | Manual acceptance checks from the [roadmap](ROADMAP.md) |

## Shared rules

- `[workspace.dependencies]` keeps one version for each dependency.
- Library crates use `thiserror`. Only `bloop-app` uses `anyhow`, for start errors.
- All crates use `tracing` for logs.
- Types that contain text or audio do not implement `Debug`.

## Implementation plan

Each [roadmap](ROADMAP.md) phase adds code from the key press to the paste.
Thus the application operates at the end of each phase.

| Phase | Crates and files |
| --- | --- |
| 1: Key, orb, and paste | `bloop-core`: `domain.rs`, `session.rs`, `ports/platform.rs`, `error.rs`, with unit tests. `bloop-macos`: `app.rs`, `events.rs`, `keys.rs`, `orb/`, `paste.rs`, `menu.rs`, `scheduler.rs`, `permissions.rs`. `bloop-app`: `main.rs`, `config.rs`, `driver.rs`, `logging.rs`. `scripts/bundle.sh`. |
| 2: Microphone and transcription | `bloop-audio`: `capture.rs`, `convert.rs`. `bloop-core`: `Transcriber` in `ports/ai.rs`, and `pipeline.rs` without steps. `bloop-providers`: `http.rs`, `mistral/transcribe.rs`. `bloop-macos`: `keychain.rs`. |
| 3: Correction | `bloop-core`: `ChatModel` and `TextStep` in `ports/ai.rs`, `steps/refine.rs`, `steps/trim.rs`. `bloop-providers`: `mistral/chat.rs`. `prompts/natural-clean-v1.md`. |

In Phase 1, `bloop-app` uses two temporary parts:

- An `AudioInput` that records nothing.
- A driver rule that answers `StopCaptureAndProcess` with a `TextReady` event that contains fixed test text.

Phase 2 removes these two parts.

## Work method

1. Write the types and the unit tests in `bloop-core` before the adapter.
2. Add a fake implementation for tests at the same time as each new port.
3. Write the adapter. Then connect it in the driver.
4. Do the manual acceptance checks of the phase on the reference Mac.
5. Before each commit, run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`.

- Add each new dependency to `[workspace.dependencies]` first.
- If a decision changes, update this document and the [contracts](CONTRACTS.md) in the same commit.
- Record measured values separately from targets.
