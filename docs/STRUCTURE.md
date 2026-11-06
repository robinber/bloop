# Code structure

Status: Revision 22 (2026-11-06). The owner approved the workspace on 2026-10-31. Phase 1 and Phase 2 are complete.
`bloop-core` contains the streaming types and ports. `bloop-audio` contains the sample queue, the microphone capture, and the 16 kHz converter.
`bloop-app` contains the worker.
The driver starts the microphone capture and the worker. The worker streams the audio to Voxtral, and the driver pastes the complete raw transcript.
At start, `bloop-macos` reads the API key from Keychain and checks the microphone permission. `bloop-app` gives the key to the adapter.
When the system is about to sleep, `bloop-macos` sends `SystemSleep` to the core.
`bloop-providers` contains the Voxtral transcription adapter in the default `mistral` feature.
Phase 3 adds optional correction, one toggle to the existing status menu, and the dictionary.
Phase 4 adds the settings hub and general menu model. Phase 6 adds learning from fixes.
The [roadmap](ROADMAP.md#priorities) gives the priorities.

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
│   └── bloop-app/             Executable: settings (correction state; the rest in Phase 4), construction, driver
├── prompts/
│   ├── natural-light.md       Default correction prompt (Phase 3)
│   ├── natural-full.md        Candidate prompt after the first daily-use trial
│   └── test-set-fr.toml       French test set of the correction, with reserved validation samples (Phase 3)
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
| Built-in microphone over a Bluetooth default input | A Bluetooth microphone uses the hands-free profile: a low rate, a switch of the headset audio at each capture start, and no text for short dictations. | The default input only. A microphone submenu stays a Phase 7 option. |
| Tagged enums for provider settings | Each vendor declares its own typed settings. An unknown provider name makes the settings invalid. | An untyped map of keys and values. |
| One provider crate with one Cargo feature for each vendor | Little repeated configuration. A vendor that is not selected does not compile. | One crate for each vendor. Use it later if one vendor module becomes large. |
| Main-thread ports without a `Send` bound | AppKit objects stay on the main thread. `EventSink` and `AudioSink` cross thread boundaries. | `Send` bounds that need wrappers around AppKit objects. |
| `unsafe` code only in `bloop-macos` | The review of unsafe code covers one crate only. | `unsafe` code in each crate that calls a system API. |
| Settings in `bloop-app`, with a snapshot for each generation | The core stays a session state machine. A change cannot reach an active capture or a running job. | Settings in the core: the core grows with each feature. |
| One `config.toml` that Bloop edits with `toml_edit` | One source of truth. Hand edits and comments stay. | A read-only file and a state file: two sources and precedence rules. `NSUserDefaults`: no comments and a second source. |
| Menu built from a data model | A feature adds data, not AppKit code. Tests examine the model without macOS. | One AppKit item and one selector for each feature. |
| Prompt files that the user edits, with defaults in the binary | The user changes the correction without a build. The hash in the log identifies the measured prompt. | Prompts at compile time only. |

These items are outside the structure: plugins loaded at run time, and support for Windows or Linux beyond the ports.

## Crates

### bloop-core

```text
src/
├── lib.rs
├── domain.rs          Generation, AudioChunk, CaptureSummary, LevelMeter, TargetInfo, Transcript, FinalText
├── dictionary.rs      Dictionary and its entries: written forms and pairs (Phase 3)
├── fixes.rs           Comparison of the pasted text and the edited text: suggestions (Phase 6)
├── session.rs         Core, Capture, Job, Event, Command, OrbView, InsertOutcome, FallbackReason
├── pipeline.rs        Complete transcript to result event (run) or final text (process): text steps, raw fallback, step report
├── steps/             Text steps (Phase 3)
│   ├── mod.rs         correction(): the step list of the correction, for the app and the evaluation tool
│   ├── refine.rs      LlmRefine: correction through a ChatModel and a prompt
│   ├── typography.rs  Typography: French spaces and apostrophes after the correction
│   └── trim.rs        Trim: removes line breaks at the start and at the end
├── ports/
│   ├── ai.rs          Transcriber, TranscriptionSession, BoxFuture; Phase 3 adds ChatModel, ChatReply, and TextStep
│   └── platform.rs    EventSink, AudioSink, AudioInput, Overlay, TextOutput, Scheduler, SecretStore; Phase 4 adds the menu model
└── error.rs           AppError, ProviderError, AudioQueueError, SecretError, Permission; Phase 3 adds StepError
```

### bloop-providers

```text
src/
├── lib.rs             TranscriberConfig, ChatConfig with its model accessors, build_transcriber, build_chat_model (Phase 3)
├── http.rs            Shared correction client, time limits, HTTP error conversion to ProviderError (Phase 3)
└── mistral/
    ├── mod.rs         Mistral settings and Keychain account ("mistral"); bloop-app supplies the API key
    ├── transcribe.rs  Voxtral WebSocket: impl Transcriber and TranscriptionSession
    └── chat.rs        Mistral chat models: impl ChatModel (Phase 3)
```

Each vendor module has its own Cargo feature. The `mistral` feature is the default.

### bloop-audio

```text
src/
├── lib.rs
├── capture.rs         cpal stream, mono mix, LevelMeter update: impl AudioInput
├── device.rs          Input device choice: the built-in microphone replaces a Bluetooth default input
├── buffer.rs          Preallocated bounded sample queue with end and overflow marks: impl AudioSink
└── convert.rs         Converter: incremental resampling to 16 kHz mono; flush at capture end
```

`convert.rs` uses the FFT resampler of `rubato`. This resampler filters the audio before it lowers the rate. Thus it prevents aliasing.
On 2026-11-05, the owner selected 16 kHz for all microphones, because Voxtral returned no text for AirPods audio at 24 kHz.
`device.rs` reads the transport of each device through the safe helpers of `coreaudio-rs`, the Core Audio crate behind `cpal`.
On 2026-11-05, the owner selected the built-in microphone over a Bluetooth default input, because Voxtral returned no text for short AirPods captures at 16 kHz.

### bloop-macos

```text
src/
├── lib.rs
├── app.rs             NSApplication setup, accessory mode, run loop start
├── events.rs          MainSink: impl EventSink through the main queue
├── keys.rs            Event tap for the dictation key, other input, and Escape
├── focus.rs           Accessibility watch of the focused field after a paste (Phase 6)
├── orb/
│   ├── mod.rs         OrbPanel: impl Overlay
│   ├── panel.rs       NSPanel configuration and position
│   └── layers.rs      Pill, application icon, bars, animations
├── paste.rs           Target check, pasteboard, Command-V: impl TextOutput
│   └── clipboard.rs   Clipboard save, restore, and text write
├── menu.rs            Status item and menu; correction toggle in Phase 3; general menu model in Phase 4
├── keychain.rs        impl SecretStore
├── scheduler.rs       Main queue timers: impl Scheduler
├── sleep.rs           Sleep observer: sends SystemSleep to the core
└── permissions.rs     Permission checks and System Settings links
```

### bloop-app

```text
src/
├── main.rs            Reads the API key, starts Tokio, builds objects, starts the AppKit run loop
├── settings.rs        Settings file: read at start, write of the correction state with toml_edit
│                      Phase 4 makes it settings/: full schema, template, prompt files
├── hub.rs             Settings hub: current snapshot, objects built from it, menu status (Phase 4)
├── menu.rs            Menu model and menu actions (Phase 4)
├── dictionary.rs      Dictionary file: read (Phase 3), save and ignore a pair (Phase 6)
├── prompts.rs         Default correction prompt in the binary, from prompts/natural-light.md (Phase 3)
├── driver.rs          Driver: events to the core, commands to the ports
├── worker.rs          Transcription task of one generation: queue, conversion, provider session, pipeline, result event
├── logging.rs         tracing output to ~/Library/Logs/Bloop/bloop.log
└── bin/eval/          Evaluation tool: one prompt and one model on the test set, with the real API (Phase 3)
    ├── main.rs        Checks the files, reads the API key, builds the model, runs the samples
    ├── args.rs        Command-line arguments
    ├── set.rs         Test set file: language, dictionary pairs, samples, dev and validation split
    ├── run.rs         One sample through the correction steps; records the chat duration and finish reason
    └── report.rs      Sample blocks and summary: punctuation counts, median and maximum duration, prompt hash
```

`bin/eval/` is a second binary, `eval`. `default-run` keeps `bloop-app` as the binary of `cargo run`.
The tool builds the steps with `steps::correction` and runs them with `Pipeline::process`. Thus it runs the text steps of the application.
It checks the arguments, the test set, and the prompt before it reads the API key or opens a connection.
It runs the samples one at a time, in file order, without retries. Each request has the time limit of `--limit-ms`, 30 s by default.
It writes the samples and the results to stdout. It sends no text to the log. The log lines of the chat adapter go to stderr.
The prompt hash is FNV-1a with 64 bits, over the prompt text as sent.
Without `--validation`, the tool runs only the samples that are not reserved. With it, the tool runs only the reserved samples.

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

Phase 4 adds these settings. Until then, `main.rs` keeps the hold threshold, the language, and the correction time limit as temporary constants.
The provider settings come from the default configs in `bloop-providers`.
In Phase 3, `bloop-app` owns the correction toggle and captures its value for each generation at `StartCapture`.
`settings.rs` keeps the toggle state in `config.toml`, in the key `[refine] enabled`. It reads the key at start and writes it at each click.
Until Phase 4, the file accepts only `schema` and `[refine] enabled`. The other keys below make the file invalid. The settings hub comes in Phase 4.

```toml
schema = 1

[dictation]
key = "right_command"
hold_ms = 220
language = "fr"

[transcriber]
provider = "mistral"
model = "voxtral-mini-transcribe-realtime-3-0"
target_streaming_delay_ms = 480

[chat]
provider = "mistral"
model = "mistral-medium-latest"
reasoning_effort = "none"

[refine]
enabled = false
prompt = "natural-light"
```

- `bloop-app` owns the settings. The settings hub keeps the current snapshot: the settings and the objects built from them.
- When the core creates a generation (`StartHoldTimer`), the driver takes the snapshot of the hub.
  The hold, the transcription options, and the text steps of that generation come from this snapshot.
- A menu action changes one key through the hub. The hub writes the file, reads it again, and builds the changed objects.
- The hub builds the chat client one time. It builds the step list of each snapshot from the selected prompt and model.
- `[refine] prompt` names a file in the prompts folder, without the `.md` extension.
- The dictionary has its own file, `dictionary.toml`, next to `config.toml`.
- Each table rejects unknown keys. A `schema` value that is newer than Bloop is an error.
- The [architecture](ARCHITECTURE.md#settings-and-status-menu) gives the file rules and the menu.

The `provider` field selects a variant of a tagged enum in `bloop-providers`:

```rust
#[derive(serde::Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum TranscriberConfig {
    Mistral(mistral::TranscribeConfig),
}
```

Each vendor declares its own settings.
Each config gives the Keychain account of its provider. `bloop-app` reads the key with `SecretStore` and gives it to the build function.
`bloop-app` uses only these configs and the `Transcriber` and `ChatModel` traits. It names no provider.
`ChatConfig::model` and `ChatConfig::with_model` read and replace the model identifier for each provider. The evaluation tool uses them for `--model`.
If the provider name is unknown, the settings are invalid. The menu shows the error, and captures fail with the error view.

## Extension procedures

### Add a provider

1. Add a module in `bloop-providers/src/` with its own Cargo feature.
2. For STT, implement `Transcriber` and `TranscriptionSession`. For correction, implement `ChatModel` (Phase 3).
3. Add a variant to `TranscriberConfig` or `ChatConfig` (Phase 3). Add one match arm in the build function.
4. Store the API key in Keychain. Use the vendor name as the account.
5. Set `provider` in `config.toml` (Phase 4).

The other crates do not change.
Every STT adapter sends audio during capture. It must not collect a complete recording before sending it.
Future DGX-hosted and Microsoft adapters use the same streaming contract. Their transport stays inside the provider crate.

### Add a text feature

Examples: snippets, translation.
This procedure applies from Phase 3.

1. Add a `TextStep` in `bloop-core/src/steps/`. A step that needs a model receives an `Arc<dyn ChatModel>`.
2. Add unit tests for the step.
3. Add the step to the step list that `bloop-app` builds for each settings snapshot.
   A step of the correction goes in `steps::correction`. Thus the application and the evaluation tool stay identical.

`StepContext` contains the language. A step that needs the target application adds the target to `StepContext`.
The driver then gives the target to the running task at key release.

### Add a platform function

Examples: start and stop sounds, text adaptation at the cursor.

1. Add a method or a port in `bloop-core/src/ports/platform.rs`.
2. Implement it in `bloop-macos`.
3. If the state machine must decide when to use it, add a `Command`.

### Add a setting or a menu feature

This procedure applies from Phase 4.

1. Add the field to the settings schema with a default value. Add it to the template.
2. Add the items to the menu model and a change for the menu action.
3. If the feature changes the text, add a text step. If it needs a system API, add a platform function.
4. Add tests for the default value, the menu model, and the change.

The AppKit code of the menu does not change.

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
| `bloop-core` | State machine: event sequences, expected commands, and stale results. Pipeline and step report. Steps with a fake chat model (Phase 3). |
| `bloop-providers` | Protocol messages, ordered audio, final response, and cancellation with a fake server. Chat replies, truncation, and HTTP errors with a fake HTTP server (Phase 3). Real API tests run manually. |
| `bloop-audio` | Incremental conversion across chunk boundaries, final flush, queue limits, and level calculation with generated signals |
| `bloop-app` | Driver and worker with fake ports: slow connection, full queue, release, failure, and cancellation during capture, a send, or `finish`. Phase 3 adds correction timeout, cancellation, reverse completion order, and toggle isolation. A cancelled task sends no event. Settings tests cover the parse, the `toml_edit` change of the correction state, and file writes in a temporary folder. Phase 4 adds the template, the settings snapshot, and the menu model. Tests of the evaluation tool: arguments, test set parse and split, punctuation counts, prompt hash, median, and report format. One sample runs with a fake chat model. |
| `bloop-macos` | Unit tests of the logic without macOS types: key state, target list, clipboard transaction, bar heights, and orb view changes. Tests of the clipboard save, restore, and text write on a private `NSPasteboard`. Rendering of a menu model (Phase 4). Manual acceptance checks from the [roadmap](ROADMAP.md) for the other macOS calls. |

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

Follow the [roadmap priorities](ROADMAP.md#priorities).
The application operates at the end of each selected phase.

| Phase | Crates and files |
| --- | --- |
| 1: Key, orb, and paste | Root: `Cargo.toml` with the workspace lints, `rustfmt.toml`, `clippy.toml`, `Cargo.lock`. `bloop-core`: `domain.rs`, `session.rs`, `ports/platform.rs`, `error.rs`, with unit tests. `bloop-macos`: `app.rs`, `events.rs`, `keys.rs`, `orb/`, `paste.rs`, `menu.rs`, `scheduler.rs`, `permissions.rs`. `bloop-app`: `main.rs`, `driver.rs`, `logging.rs`. `scripts/bundle.sh`. |
| 2: Microphone and streaming | `bloop-core`: audio and streaming contracts, `pipeline.rs` without steps. `bloop-audio`: `capture.rs`, `buffer.rs`, `convert.rs`. `bloop-providers`: `mistral/transcribe.rs`. `bloop-app`: `worker.rs`, stream ownership and lifecycle tests. `bloop-macos`: `keychain.rs`, the microphone check in `permissions.rs`, `sleep.rs`. |
| 3: Optional correction | `bloop-core`: `ChatModel`, `ChatReply`, and `TextStep` in `ports/ai.rs`, `StepError`, `steps/`, the step report in `pipeline.rs`. `bloop-providers`: `http.rs`, `mistral/chat.rs`, the provider configs and build functions in `lib.rs`. `bloop-app`: the pipeline in `worker.rs`, the toggle value for each generation, `bin/eval/`. `bloop-macos`: one toggle in the existing `menu.rs`. `bloop-core` and `bloop-app`: `dictionary.rs`. `prompts/natural-light.md` and the test set. |
| 4: Settings and status menu | `bloop-core`: the menu model in `ports/platform.rs`. `bloop-app`: `settings/` (it replaces `settings.rs`), `hub.rs`, `menu.rs`. `bloop-macos`: `menu.rs` renders the model. |
| 6: Learn from fixes | `bloop-core`: `fixes.rs`, the suggestion orb view. `bloop-macos`: `focus.rs`, the suggestion card in `orb/`. `bloop-app`: save and ignore in `dictionary.rs`. |
| 5, 7, and 8 | The [roadmap](ROADMAP.md) gives the features. Their files get detail before each phase starts. |

Until Phase 4 adds the settings, `bloop-app` keeps these temporary constants in `main.rs`:

- The hold threshold: `HOLD`, 220 ms.
- The language: `LANGUAGE`, `fr`. The word list comes from the dictionary file.
- The correction time limit (Phase 3): `CORRECTION_LIMIT`, 8 s. This is an initial value, not measured.

The correction prompt (Phase 3) is `natural-light`. `prompts.rs` puts the file in the binary with `include_str!`.

From Phase 3, the default provider settings come from `TranscriberConfig::default()` and `ChatConfig::default()` in `bloop-providers`.
The transcription uses `voxtral-mini-transcribe-realtime-3-0` with `target_streaming_delay_ms` 480.
The correction uses `mistral-medium-latest` with `reasoning_effort` `none` and temperature 0.
Thus `main.rs` names no provider.
Phase 3 adds the correction toggle as runtime state. It does not require the settings hub.
`settings.rs` keeps the toggle state in `config.toml` (`[refine] enabled`) before the hub exists.
Phase 4 replaces the constants with the settings hub.

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
