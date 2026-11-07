# Code structure

Status: Revision 26 (2026-11-07). The owner approved the workspace on 2026-10-31. Phase 1 and Phase 2 are complete.
`bloop-core` contains the streaming types and ports. `bloop-audio` contains the sample queue, the microphone capture, and the 16 kHz converter.
`bloop-app` contains the worker.
The driver starts the microphone capture and the worker. The worker streams the audio to Voxtral, and the driver pastes the complete raw transcript.
At start, `bloop-macos` reads the API key from Keychain and checks the microphone permission. `bloop-app` gives the key to the adapter.
When the system is about to sleep, `bloop-macos` sends `SystemSleep` to the core.
`bloop-providers` contains the Voxtral transcription adapter in the default `mistral` feature.
Phase 3 adds optional correction, one toggle to the existing status menu, and the dictionary.
Phase 4 iteration 1 adds the settings file. Iteration 2 adds the settings hub: `bloop-app` reads the files again at each dictation key press.
Iteration 3 adds the menu model: `bloop-app` builds the menu content, and `bloop-macos` renders it each time the menu opens.
Phase 6 adds learning from fixes.
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
│   └── bloop-app/             Executable: settings, construction, driver
├── prompts/
│   ├── natural-light.md       Built-in correction prompt (Phase 3)
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
| Built-in prompts in the binary, plus prompt files of the user | The user changes the correction without a build. A revised built-in prompt reaches the application at the next build. The evaluation tool prints the hash of the measured prompt. | Prompts at compile time only. Default prompt files on disk that a later build does not update. |

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
│   └── platform.rs    EventSink, AudioSink, AudioInput, Overlay, TextOutput, Scheduler, SecretStore;
│                      the menu model and MenuSource (Phase 4 iteration 3)
└── error.rs           AppError, ProviderError, AudioQueueError, SecretError, Permission; Phase 3 adds StepError
```

### bloop-providers

```text
src/
├── lib.rs             TranscriberConfig, ChatConfig with its model accessors, build_transcriber, build_chat_model (Phase 3);
│                      Deserialize (feature-gated), Clone, PartialEq, and template() (Phase 4)
├── http.rs            Shared correction client, time limits, HTTP error conversion to ProviderError (Phase 3)
└── mistral/
    ├── mod.rs         Mistral settings, their template tables, and Keychain account ("mistral"); bloop-app supplies the API key;
    │                  the chat settings have the models list (Phase 4)
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
├── menu.rs            Status item and menu: renders the menu model at each open, with the menu delegate,
│                      menuAction: with item tags, and submenus (Phase 4 iteration 3)
├── workspace.rs       Opens a file or a folder through NSWorkspace, without unsafe (Phase 4 iteration 3)
├── keychain.rs        impl SecretStore
├── scheduler.rs       Main queue timers: impl Scheduler
├── sleep.rs           Sleep observer: sends SystemSleep to the core
└── permissions.rs     Permission checks and System Settings links: open_settings for the menu
```

### bloop-app

```text
src/
├── main.rs            Starts Tokio and the settings hub, builds objects, starts the AppKit run loop
├── settings/          Settings file (Phase 4 iteration 1)
│   ├── mod.rs         Paths; prepare: the template, the empty dictionary, and the prompts folder when missing; parse;
│   │                  the write of the correction state
│   ├── schema.rs      Settings, its tables, the newtypes HoldMs, Language, and PromptName, parse
│   ├── template.rs    The commented settings file with the default values
│   └── edit.rs        The toml_edit change of one key
├── files.rs           read_bounded, write_if_missing, replace (Phase 4 iteration 1)
├── hub.rs             Settings hub: file texts, parsed settings, snapshot, chat model reuse, HubSnapshots (Phase 4 iteration 2);
│                      the menu view and the refused key state (iteration 3)
├── menu.rs            Menu builder: AppMenu, menu model, status lines, and menu actions (Phase 4 iteration 3); lists (iteration 4)
├── dictionary.rs      Dictionary file: parse (Phase 3), size limit (Phase 4 iteration 2), commented empty template (iteration 3),
│                      save and ignore a pair (Phase 6)
├── prompts.rs         Built-in correction prompts, from prompts/natural-light.md (Phase 3);
│                      resolves a name to a user file or a built-in prompt (Phase 4 iteration 1)
├── driver.rs          Driver: events to the core, commands to the ports;
│                      the Snapshots trait and a settings snapshot for each generation (Phase 4 iteration 2)
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

`settings/` parses `config.toml` with the schema below. The settings hub reads the file at each dictation key press and each time the menu opens.
Thus a change in the file applies from the next dictation.
`settings/` writes `[refine] enabled` at each click of "Correct text", through `Hub::set`.

Each key is optional. A missing key has the value below, which is its default value.
Each table rejects unknown keys. A `schema` value other than 1 is an error.

```toml
schema = 1

[dictation]
hold_ms = 220                 # 100..=1000.
language = "fr"               # Two lowercase ASCII letters.

[transcriber]
provider = "mistral"
model = "voxtral-mini-transcribe-realtime-3-0"
target_streaming_delay_ms = 480

[chat]
provider = "mistral"
model = "mistral-medium-latest"
models = ["mistral-medium-latest", "mistral-small-latest"]   # The "Model" submenu (iteration 4).
reasoning_effort = "none"

[refine]
enabled = false
prompt = "natural-light"      # A built-in prompt or a file of the prompts folder, without ".md".
```

- `[dictation] key` comes with the optional key iteration (Phase 4 iteration 5). Until then, it is an unknown key.
- The correction time limit stays the constant `CORRECTION_LIMIT` (8 s) in `main.rs` (owner decision A).
- `settings/` validates the values at parse time with `try_from` newtypes: `HoldMs`, `Language`, and `PromptName`.
  Each newtype uses `type Error = &'static str`. Thus a `Settings` value is always valid.
- `PromptName` accepts 1 to 64 characters from `A-Z`, `a-z`, `0-9`, `_`, and `-`.
- An error gives the line of the key, or of its table. The `toml` message contains the value.
  Thus only the line goes into the error type and the log.
- `[refine] prompt` names a file of the prompts folder or a built-in prompt, without the `.md` extension.
  A file replaces the built-in prompt with the same name.
- If `config.toml` does not exist at start, `settings::prepare` writes the template and creates the prompts folder. `Hub::start` calls it.
  If `dictionary.toml` does not exist, `settings::prepare` also writes `dictionary::TEMPLATE`: comments and no entries.
  `template.rs` builds the template from its own tables and the provider texts. Thus it names no provider.
- For an invalid file, each capture fails with `InvalidSettings`, the hold threshold has its default value, and the correction is off.
- For each new snapshot, the log records the nonsecret values: the hold threshold, the language, the provider settings, the prompt name,
  and the correction state.
- The dictionary has its own file, `dictionary.toml`, next to `config.toml`.
- `files.rs` gives the file rules as `pub(crate)` functions: `read_bounded`, `write_if_missing`, and `replace`.
  Phase 6 writes the dictionary with the same functions.
- The size limits are constants: `settings::LIMIT` (64 KiB), `prompts::LIMIT` (32 KiB), and `dictionary::LIMIT` (256 KiB).
- The [architecture](ARCHITECTURE.md#settings-and-status-menu) gives the file rules, the errors, and the menu.

The settings hub (Phase 4 iteration 2):

```rust
pub(crate) struct Snapshot {
    pub(crate) hold: Duration,
    pub(crate) speech: Result<Arc<Speech>, AppError>,
}

pub(crate) trait Snapshots {
    fn snapshot(&mut self) -> Rc<Snapshot>;
}
```

- `bloop-app` owns the settings. `Hub` in `hub.rs` keeps the file texts, the parsed settings, the chat model, and the current `Snapshot`.
- `Hub` runs on the main thread only. All its methods take `&self`, and its `RefCell` stays inside it.
  The driver adapter, the menu source `AppMenu`, and the event handler of `main.rs` share it as `Rc<Hub>`.
- `Hub::start` writes the template when it is missing, reads the files and the keys, and builds the first snapshot.
  It never fails: each error goes into the hub state. `main.rs` gives it `CORRECTION_LIMIT`.
- `Hub::refresh` reads `config.toml` and `dictionary.toml` with `read_bounded`, and the selected prompt when the correction is on.
  If each text and the prompt equal the last ones, the snapshot stays. Else the hub parses the changed files and builds a new snapshot.
- `Hub::refresh` returns the current snapshot as `Option<Rc<Snapshot>>`. The value is `None` only for a failed borrow.
- `Hub::set` writes one key for a `Change`. Then it refreshes. Thus one path builds each new snapshot. A refused write logs a warning.
  Iteration 2 has `Change::Correction`. Iteration 4 adds `Prompt`, `Model`, and `Hold`.
- `Hub::view` refreshes, then returns a `View` for the menu: the settings summary or the `config.toml` error,
  the prompt error with the correction on, the dictionary error, and the refused key state. It contains no file content.
- `Hub::note` gets each event before the driver. An event with `Unauthorized` sets the refused key state when `Hub::start` read the key.
  The state is a `Cell` outside the `RefCell`. It stays until the next start.
- `Driver` gets `Snapshots` as a fifth generic parameter, like the other ports. `HubSnapshots` implements it for the hub. Tests use a fake.
- `HubSnapshots` refreshes the hub and gives the current snapshot. After a failed borrow, it gives its last snapshot.
- At `StartHoldTimer`, the driver takes a snapshot, keeps it with the generation, and starts the timer with `snapshot.hold`.
- At `StartCapture`, the driver gives `&snapshot.speech` of that generation to `Spawner::spawn`.
  A generation without its snapshot fails with `CaptureFailed(Internal)`. This is a bug guard.
- `speech` is `Err(InvalidSettings)` when `config.toml` or `dictionary.toml` is invalid. It is `Err(Provider(Unauthorized))` without a key.
- `Speech::correction` is a `Result<Correction, NoCorrection>`, with `NoCorrection::{Off, ChatUnavailable, InvalidPrompt}`.
  The [contracts](CONTRACTS.md#pipeline) give the effect of each value.
- With an invalid `config.toml`, `hold` has its default value. With an invalid `dictionary.toml`, `hold` comes from the settings.
- The hub builds the transcriber again for each new snapshot. This only stores the settings and the key.
- The hub builds the chat client again only when `ChatConfig` changes (`PartialEq`). `build_chat_model` does no I/O.
  A prompt change builds a new `Correction` with the same chat model.
- Keys: `Hub::start` reads the key of each account that the start settings name. With invalid settings, it uses the default settings.
  A provider that the file names later, without a key read at start, is `Unauthorized` until the next start.
  Bloop does not read Keychain at the key press, because a read can wait for a dialog.
- The keys are in `Hub`, outside its `RefCell`, because they do not change after start. Thus the status menu gets the key error without a borrow.
- Each driver entry is a block on the main queue, and `MainSink::send` is always asynchronous.
  AppKit calls `menuNeedsUpdate:` and `menuAction:` from the run loop, between these blocks.
- A hub borrow contains only file I/O, parsing, object construction, and log calls.
  It contains no AppKit call, no `Scheduler::after`, and no `MainSink::send`. The hub returns each `Rc` after the borrow ends.
  Thus a failed `try_borrow_mut` is a bug. Bloop logs it at `error!` and never panics.
  Then the driver adapter uses its last snapshot, and the menu shows its last model.
- The log records file names, error kinds, lines, and nonsecret values: models, prompt name, and hold. It never records file content.
  The hub logs an invalid file only after a text change. Thus a key press or a menu open without a change does not repeat the log.

The `provider` field selects a variant of a tagged enum in `bloop-providers`:

```rust
#[cfg_attr(
    feature = "mistral",
    derive(serde::Deserialize),
    serde(tag = "provider", rename_all = "snake_case")
)]
pub enum TranscriberConfig {
    #[cfg(feature = "mistral")]
    Mistral(mistral::TranscribeConfig),
}
```

Each vendor declares its own settings.
Each config gives the Keychain account of its provider. `bloop-app` reads the key with `SecretStore` and gives it to the build function.
`bloop-app` uses only these configs and the `Transcriber` and `ChatModel` traits. It names no provider.
`ChatConfig::model` and `ChatConfig::with_model` read and replace the model identifier for each provider. The evaluation tool uses them for `--model`.
If the provider name is unknown, the settings are invalid. Captures fail with the error view. The menu shows the error.

The configs and the settings file (Phase 4 iteration 1):

- The four config types derive `Clone` and `PartialEq`. `Deserialize` is under `cfg_attr(feature = "mistral", ...)`, like the `Default` impls.
  Thus `serde` stays optional.
- Each variant struct has `#[serde(default, deny_unknown_fields)]`. `temperature` has `#[serde(skip)]`.
- A missing `[transcriber]` or `[chat]` table gives the default config. `provider` is necessary when the table exists.
- An unknown `provider` value gives its own line. An unknown key or a wrong type in these two tables gives the line of the table header,
  because `serde` buffers a tagged enum. A design review checked these lines with `toml` 1.2.0 and `serde` 1.0.229.
- `models` is a field of the vendor chat struct: the models of the configured provider.
  A change of the list builds the chat client again. This is rare and costs one TLS handshake.
- `TranscriberConfig::template()` and `ChatConfig::template()` return the commented tables of the default provider, as `&'static str`.
  `bloop-app` builds the template from its own tables and these texts. Thus vendor names stay in `bloop-providers`.
- A `bloop-app` test proves that the settings template parses to the default settings. Thus each template text gives `Default::default()`.
  The tests of the provider tables are also in `bloop-app`, because `bloop-providers` has no `toml` dependency.

## Extension procedures

### Add a provider

1. Add a module in `bloop-providers/src/` with its own Cargo feature.
2. For STT, implement `Transcriber` and `TranscriptionSession`. For correction, implement `ChatModel` (Phase 3).
3. Add a variant to `TranscriberConfig` or `ChatConfig` (Phase 3). Add one match arm in the build function.
4. Derive `Clone` and `PartialEq` on the vendor config, and `Deserialize` under its feature (Phase 4).
   Add `#[serde(default, deny_unknown_fields)]`. Write its commented table for `template()`.
   A chat config also needs the `models` field.
5. Store the API key in Keychain. Use the vendor name as the account.
6. Set `provider` in `config.toml` (Phase 4).

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
| `bloop-providers` | Protocol messages, ordered audio, final response, and cancellation with a fake server. Chat replies, truncation, and HTTP errors with a fake HTTP server (Phase 3). The settings tests of `bloop-app` cover the parse of the provider configs and the template texts, because `bloop-providers` has no `toml` dependency. Real API tests run manually. |
| `bloop-audio` | Incremental conversion across chunk boundaries, final flush, queue limits, and level calculation with generated signals |
| `bloop-app` | Driver and worker with fake ports: slow connection, full queue, release, failure, and cancellation during capture, a send, or `finish`. Phase 3 adds correction timeout, cancellation, reverse completion order, and toggle isolation. A cancelled task sends no event. Settings tests cover the parse, the `toml_edit` change of the correction state, and file writes in a temporary folder. Phase 4 adds the tests in the list below. Tests of the evaluation tool: arguments, test set parse and split, punctuation counts, prompt hash, median, and report format. One sample runs with a fake chat model. |
| `bloop-macos` | Unit tests of the logic without macOS types: key state, target list, clipboard transaction, bar heights, and orb view changes. Tests of the clipboard save, restore, and text write on a private `NSPasteboard`. Phase 4 adds pure tests of `tag_of` and `action_of`: tag 0 is reserved, and an overflow fails. `NSMenu` needs a `MainThreadMarker`, which the test harness does not give. Thus manual checks cover the menu renderer. Manual acceptance checks from the [roadmap](ROADMAP.md) for the other macOS calls. |

Phase 4 tests in `bloop-app`:

- Settings: each default value; each range limit with its line (the line of the value, of the key, and of the table);
  an unknown key in each table; wrong types; invalid prompt names (`../x`, empty, 65 characters); `[dictation] key` is an error;
  the template parses to the default values; each `Change` keeps the comments and the other keys byte for byte.
  The provider tables: a missing table gives the default config; a table without `provider`; an unknown provider; an unknown key.
- Files, in temporary folders with the `scratch_dir` pattern: `write_if_missing` writes one time and keeps an existing file byte for byte;
  `read_bounded` at the limit and one byte over it; prompt resolution (file, built-in, file over a built-in, missing, empty, too large).
- Hub, in temporary folders with a fake `SecretStore`: an unchanged text builds no new snapshot (`Rc::ptr_eq`);
  a changed settings file, dictionary, or prompt file builds one; invalid, unreadable, or too large files give `InvalidSettings`;
  a prompt change keeps the chat model, and a `[chat]` change replaces it (`Arc::ptr_eq`, with the real `build_chat_model` and a dummy key);
  with the correction off, the hub does not read the prompt; `set` writes the key with its comments and does not change an invalid file;
  no key at start gives `Unauthorized`; a failed borrow gives the last snapshot.
- Driver: the driver takes the snapshot at `StartHoldTimer`, and its hold time starts the timer; a later change does not affect the generation;
  a missing snapshot gives `CaptureFailed(Internal)` before the microphone starts.
- Worker: `InvalidPrompt` with the correction on gives `Err(InvalidSettings)`; `Off` and `ChatUnavailable` give the raw text.
- Menu: pure tests of the model builder (status lines and their order, lists, the added current value, hidden items with invalid settings,
  action identifiers); the event filter for a refused key; a click on "Correct text" writes the key; the dictionary template parses.

Phase 4 iterations 1 and 2 changed the settings, dictionary, worker, and driver tests. Each commit reports the changed tests.
Iteration 3 moved the `bloop-macos/src/menu.rs` tests of `status` and `Toggle` to the model builder.
The `Toggle` test became a test of two clicks on "Correct text".

Phase 4 iteration 3 manual checks of the menu renderer: one `menuNeedsUpdate:` for each open, the checkmarks, ⌘Q, the tag guard,
"Open config.toml…" with and without an application for `.toml` files, and "Open prompts folder…". Miri is not covered for `bloop-macos`.

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
| 4: Settings and status menu | Iteration 1: `bloop-providers`: `lib.rs`, `mistral/mod.rs`. `bloop-app`: `settings/` (it replaces `settings.rs`), `files.rs`, `prompts.rs`, `worker.rs`, `main.rs`. Iteration 2: `bloop-app`: `hub.rs`, `driver.rs`, `worker.rs`, `main.rs`. Iteration 3: `bloop-core`: the menu model in `ports/platform.rs`. `bloop-macos`: `menu.rs`, `workspace.rs`, `permissions.rs`. `bloop-app`: `menu.rs`, `hub.rs`, `settings/mod.rs`, `dictionary.rs`, `main.rs`. Iteration 4: `bloop-app`: `menu.rs`, `hub.rs`. Iteration 5, optional: `bloop-macos`: `keys.rs`. No new dependency and no new Cargo feature. |
| 6: Learn from fixes | `bloop-core`: `fixes.rs`, the suggestion orb view. `bloop-macos`: `focus.rs`, the suggestion card in `orb/`. `bloop-app`: save and ignore in `dictionary.rs`. |
| 5, 7, and 8 | The [roadmap](ROADMAP.md) gives the features. Their files get detail before each phase starts. |

`bloop-app` keeps one constant in `main.rs`: the correction time limit `CORRECTION_LIMIT`, 8 s (Phase 3).
This is an initial value, not measured. It stays a constant in Phase 4 (owner decision A, 2026-11-07).
The hold threshold and the language are settings: `[dictation] hold_ms`, 220 ms by default, and `[dictation] language`, `fr` by default.
The word list comes from the dictionary file.

The default correction prompt is `natural-light`. `prompts.rs` puts the file in the binary with `include_str!`.
`prompts.rs` keeps a table of the built-in prompts. It resolves a name to a user file or to a built-in prompt.

The default provider settings come from `TranscriberConfig::default()` and `ChatConfig::default()` in `bloop-providers`.
The transcription uses `voxtral-mini-transcribe-realtime-3-0` with `target_streaming_delay_ms` 480.
The correction uses `mistral-medium-latest` with `reasoning_effort` `none` and temperature 0.
Thus `main.rs` names no provider.
`config.toml` gives these settings. A missing key or table keeps the default value.
The state of the correction toggle is `[refine] enabled` in `config.toml`. The settings hub reads it at each refresh.
In the settings snapshot, `NoCorrection::Off` gives the off state (Phase 4 iteration 2).

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
