# Bloop agent contract

Bloop is a personal macOS dictation app in Rust. `README.md`, `CONTRIBUTING.md`, and the documents in `docs/` describe the current product and code.

## Load order

1. This file.
2. `README.md`, `CONTRIBUTING.md`, and `docs/ARCHITECTURE.md`.
   Read `docs/ARCHITECTURE.md` before any Rust change, and `docs/SETUP.md` when the task touches the settings, the files, or the usage.
3. `.claude/skills/rust-strict/SKILL.md` (the version pinned by the git submodule).
   Read it before you create, change, review, debug, or verify Rust or Cargo files.
4. Rust policy files when they exist: `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`,
   `clippy.toml`, `.cargo/config.toml`, and `deny.toml`.

Explicit owner instructions take precedence over repository guidance and skill defaults.
For an unresolved conflict between repository rules, stop the affected work and report the conflict.
Do not choose the interpretation that permits more work.

## Package facts

The workspace manifests are the source of truth. When they change, update this section in the same commit.

- Layout: virtual workspace with `resolver = "3"`. Members in `crates/`: `bloop-core`, `bloop-providers`,
  `bloop-audio`, `bloop-macos`, `bloop-app`.
  `bloop-app` has one binary, `bloop-app`.
- Version: `0.1.0` for every package, from `[workspace.package]`. `scripts/bundle.sh` copies it to `CFBundleShortVersionString`.
- Edition: 2024. All packages have `publish = false`. `Cargo.lock` is under version control.
- License: `Apache-2.0`, with the repository URL, from `[workspace.package]`. Every package inherits both.
  The license text is `LICENSE`.
- Declared MSRV (`rust-version`): 1.99. This declaration does not prove compatibility without an MSRV check.
- Toolchain policy: stable, without `rust-toolchain.toml`. Report the actual toolchain when running checks.
  Do not introduce nightly without a documented need.
- Platform: macOS on arm64 only. Do not infer support for other targets.
- Features: `bloop-providers` has one vendor feature, `mistral`, enabled by default. Its dependencies are optional behind the feature.
  Verify with the default features unless the task changes a feature.
- Lint floor: the rust-strict starter policy (`references/project-setup.md`, section 3) in `[workspace.lints]`,
  plus `raw_borrows_via_references`. Every member inherits it with `[lints] workspace = true`. Do not weaken it.
- Formatting: `rustfmt.toml` with `style_edition = "2024"`. `clippy.toml` allows `unwrap`, `expect`, `panic`, and `dbg` in tests only.
- Gates before each commit:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
- A task limit on execution permits a report, not a commit. The gates stay mandatory before each commit.
  Report skipped checks and never describe them as passed.
- CI: none. Do not claim CI coverage.
- Drift profile: rust-strict defaults.

## Domain contract

### Rules

- The five-crate workspace and its dependency rules are in `docs/ARCHITECTURE.md`.
- The ports of `bloop-core` are `Transcriber`, `TranscriptionSession`, `ChatModel`, `TextStep`, and the platform ports.
  The AI ports return `BoxFuture`, so that the settings select the provider at run time.
- The dictation key is Right Command. The capture starts after the hold threshold and ends at key release.
- Streaming is the only transcription path. Speech providers implement one streaming contract.
  There is no file transcription, no upload fallback, and no automatic audio replay.
- Capture has no maximum duration.
- Bloop has no settings window. The settings are `config.toml` and the status menu.
- Each generation keeps its settings snapshot. Insertion follows capture order.
  Clipboard writes check the change count, as `docs/ARCHITECTURE.md` defines.
- A clipboard read is not proof of insertion. Recovery messages reflect this limit.

### Crate boundaries

- `bloop-core` depends on no other Bloop crate. It has no I/O, async runtime, or platform API.
  Its only external dependency is `secrecy`.
- `bloop-macos`, `bloop-audio`, and `bloop-providers` do not depend on each other.
- Only `bloop-app` selects and connects adapters from settings. Provider constructors stay in `bloop-providers`.
- HTTP, WebSocket, and vendor-specific production code stay in `bloop-providers`.

### Standing deviation: unsafe in bloop-macos

- Scope: the crate `bloop-macos` only.
- Approved surfaces:
  - AppKit and Foundation through `objc2`: application setup, `NSPanel`, status item and menu,
    `NSPasteboard` and its data provider, `NSWorkspace` and the sleep notification observer
    on its `NSNotificationCenter`, and `NSRunningApplication`.
  - Core Animation layers and animations.
  - Core Graphics event taps and posted events: `CGEventTapCreate`, `CGEventTapEnable`, `CGEventPost`,
    `CGEventSourceCounterForEventType`, and the run loop source of the tap in `kCFRunLoopCommonModes`.
  - `IsSecureEventInputEnabled`, Accessibility and Input Monitoring permission checks, Keychain access,
    and the main dispatch queue.
  - The AVFoundation microphone authorization status and request.
  - The menu delegate in `crates/bloop-macos/src/menu.rs`: `unsafe impl NSMenuDelegate` (`menuNeedsUpdate:`),
    one menu action selector `menuAction:` with item tags, and submenus.
- Each module that contains `unsafe` carries `#![expect(unsafe_code, reason = "...")]`.
- All other crates use `#![forbid(unsafe_code)]` at each library or binary root.
- `unsafe` outside these surfaces needs new approval.
- Miri cannot run FFI calls to macOS frameworks. For `bloop-macos`, compensate with tests of the safe wrappers,
  review of the drop and error paths, and the manual checks in `CONTRIBUTING.md`. Report Miri as not covered.
- A callback from macOS (event tap, pasteboard data provider, timer) must not panic.
  A panic in an `extern "C"` callback stops Bloop.
- Get `MainThreadMarker` in `main` and pass it to components that need it.
  A platform callback may check the current thread and return if it is not the main thread.
  Do not call `MainThreadMarker::new().expect(...)` in a module.

### Secrets and privacy

- The API key of each provider is in Keychain: service `bloop`, account = vendor name.
- Never put an API key in source, settings, logs, errors, argv, or `Debug` output. Use `secrecy::SecretString`.
- Types that contain audio, transcripts, prompts, dictionary terms, or request text redact that content in `Debug`.
  `TranscriptionOptions` shows the vocabulary count, not its entries.
- Logs contain operational metadata and controlled error categories. Never log dictated text, audio, credentials, or arbitrary provider fields.
  Use local error categories and HTTP status codes. Do not log error bodies, unknown event types, or raw finish reasons.
- The application has no `log` bridge. Review dependency logging before adding one; handshake traces can expose credentials.

### Critical surfaces

For behavior changes, add or update the relevant regression tests and perform the applicable checks below.
Documentation-only changes do not require new tests.

| Surface | Required evidence |
| --- | --- |
| `crates/bloop-core/src/session.rs` | Event sequences, command order, stale generations, and job cleanup |
| `crates/bloop-core/src/pipeline.rs` and `crates/bloop-core/src/steps/` | Fake providers: empty transcript, text steps, and fallback after a failed correction |
| Streaming ports, audio queue, converter, driver, and worker | Ordered audio, conversion and flush, finish, cancellation, full queue, failure, and stale results |
| `crates/bloop-app/src/files.rs` and `crates/bloop-app/src/settings/` | Bounded reads, valid defaults, comment preservation, overlapping writers, and cleanup after a failed write |
| `crates/bloop-app/src/hub.rs` | Temporary folders and a fake `SecretStore`: refresh, invalid files, snapshot isolation, chat model reuse, and menu writes |
| Provider logging and text-bearing `Debug` implementations | Sentinel values must not appear in logs or debug output |
| `crates/bloop-macos/src/paste.rs` and `crates/bloop-macos/src/paste/clipboard.rs` | Transaction tests and manual checks: target checks, one paste per generation, delayed restore, cancellation, and changed ownership |
| Application shutdown: `main.rs`, `events.rs`, `app.rs`, and menu quit | Cleanup after the run loop, disabled late callbacks, and manual quit during a pending clipboard transaction |
| `crates/bloop-macos/src/keys.rs` | Manual checks: hold threshold, combinations, and Escape blocking |
| `crates/bloop-macos/src/menu.rs` | Action/tag tests and manual checks: refresh, checkmarks, submenus, ⌘Q, and file links |

### Documentation

- Repository documentation uses English, short sentences, and the present tense, as `CONTRIBUTING.md` describes.
- Describe the current implementation. Do not add a roadmap, a history, measurements, or speculative features.
- Update the affected documents with the code: `docs/ARCHITECTURE.md` for the design, the invariants, and the limits;
  `docs/SETUP.md` for the settings, the files, and the troubleshooting; `README.md` for the usage;
  and `CONTRIBUTING.md` for the build, the checks, the tests, and the tools.
- Keep exact Rust signatures in source. Distinguish configured limits from verification gaps.

## Working rules

- Make the smallest change that satisfies the request.
- Add a dependency with `cargo add` to `[workspace.dependencies]` first, and check it with `cargo info`.
- The lint floor enforces this rule in non-test code: no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`,
  or `dbg!`.
- Commit messages: the subject is one imperative sentence with a capital letter, no type prefix
  (no `docs:` or `feat:`), and no final period. The body says what changed and why, wrapped at 72 characters.
