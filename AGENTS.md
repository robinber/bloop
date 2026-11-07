# Bloop agent contract

Bloop is a personal macOS dictation app in Rust. The documents in `docs/` define the product and the design.

## Load order

1. This file.
2. `README.md`, then the documents in `docs/` that the task needs.
   Read `docs/STRUCTURE.md` and `docs/CONTRACTS.md` before any Rust change.
3. `.claude/skills/rust-strict/SKILL.md` (rust-strict v2.0.0, git submodule).
   Read it before you create, change, review, debug, or verify Rust or Cargo files.
4. Rust policy files when they exist: `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`,
   `clippy.toml`, `.cargo/config.toml`, and `deny.toml`.

When these documents disagree, stop and report the conflict.
Do not choose the interpretation that permits more work.

## Package facts

The workspace manifests are the source of truth. When they change, update this section in the same commit.

- Layout: virtual workspace with `resolver = "3"`. Members in `crates/`: `bloop-core`, `bloop-providers`,
  `bloop-audio`, `bloop-macos`, `bloop-app`.
  `bloop-app` has two binaries: `bloop-app` (`default-run`) and the correction evaluation tool `eval`.
- Edition: 2024.
- MSRV (`rust-version`): 1.99, the stable release on the reference Mac (rustc 1.99.1).
- Toolchain: stable. No `rust-toolchain.toml` and no nightly unless a documented need appears.
- Platform: macOS on arm64 only. No other target is supported or tested.
- Features: `bloop-providers` has one feature for each vendor. `mistral` is the default.
  Verify with the default features unless the task changes a feature.
- Lint floor: the rust-strict starter policy (`references/project-setup.md`, section 3) in `[workspace.lints]`,
  plus `raw_borrows_via_references`. Every member inherits it with `[lints] workspace = true`. Do not weaken it.
- Formatting: `rustfmt.toml` with `style_edition = "2024"`.
- Gates before each commit:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
- CI: none. Do not claim CI coverage.
- Drift profile: rust-strict defaults.

## Domain contract

### Owner decisions

These are explicit owner decisions, not hypothetical reuse:

- The five-crate workspace and its dependency rules in `docs/STRUCTURE.md`.
- The ports in `bloop-core` (`Transcriber`, `ChatModel`, `TextStep`, and the platform ports).
  They let the owner add providers and text features.
- `BoxFuture` in the AI ports, so that the settings select the provider at run time.
- Streaming is the only STT path. Finish Phase 1 with fixed text, then implement streaming in Phase 2.
- Providers implement one streaming contract. Do not add file transcription or an upload fallback.
- Capture has no maximum duration. Add a duration limit only if the owner requests it.

### Crate boundaries

- All dependencies point to `bloop-core`. `bloop-core` has no I/O, no async runtime, and no platform API.
- `bloop-macos`, `bloop-audio`, and `bloop-providers` do not depend on each other.
- Only `bloop-app` builds objects from the settings.
- HTTP and WebSocket code and vendor names stay in `bloop-providers`.

### Standing deviation: unsafe in bloop-macos

- Scope: the crate `bloop-macos` only.
- Approved surfaces:
  - AppKit and Foundation through `objc2`: application setup, `NSPanel`, status item and menu,
    `NSPasteboard` and its data provider, `NSWorkspace` and the sleep notification observer
    on its `NSNotificationCenter`, and `NSRunningApplication`.
  - Core Animation layers and animations.
  - Core Graphics event taps and posted events: `CGEventTapCreate`, `CGEventTapEnable`, `CGEventPost`, and
    `CGEventSourceCounterForEventType`.
  - `IsSecureEventInputEnabled`, Accessibility and Input Monitoring permission checks, Keychain access,
    and the main dispatch queue.
  - The AVFoundation microphone authorization status and request (approved on 2026-11-02).
  - The menu delegate in `bloop-macos/src/menu.rs`: `unsafe impl NSMenuDelegate` (`menuNeedsUpdate:`), one menu action selector
    `menuAction:` with item tags, and submenus (approved on 2026-11-07, for Phase 4 iteration 3).
- Each module that contains `unsafe` carries `#![expect(unsafe_code, reason = "...")]`.
- All other crates add `#![forbid(unsafe_code)]` at their root.
- `unsafe` outside these surfaces needs new approval.
- Miri cannot run FFI calls to macOS frameworks. For `bloop-macos`, compensate with tests of the safe wrappers,
  review of the drop and error paths, and the manual checks in `docs/ROADMAP.md`. Report Miri as not covered.
- A callback from macOS (event tap, pasteboard data provider, timer) must not panic.
  A panic in an `extern "C"` callback stops Bloop.
- Get `MainThreadMarker` one time in `main` and pass it to the modules that need it.
  Do not call `MainThreadMarker::new().expect(...)` in a module.

### Secrets and privacy

- The API key of each provider is in Keychain: service `bloop`, account = vendor name.
- Never put an API key in source, settings, logs, errors, argv, or `Debug` output. Use `secrecy::SecretString`.
- `Transcript`, `FinalText`, and audio types implement only a redacting `Debug` that never shows their content.
  Logs never contain audio or text.
- Tabi is a behavior reference only. Do not copy Tabi code, prompts, or assets.
  Do not read its dictation history (`~/Library/Application Support/app.heytabi/history.json`).

### Critical surfaces

Add or update tests when you change these surfaces:

| Surface | Required evidence |
| --- | --- |
| `bloop-core/src/session.rs` | Unit tests: event sequences and expected commands, as `docs/CONTRACTS.md` requires |
| `bloop-core/src/pipeline.rs` and `bloop-core/src/steps/` | Unit tests with fake providers: empty transcript, text steps, raw fallback |
| Streaming ports and driver | Fake provider tests: ordered audio, finish, cancellation, full queue, failure, stale results |
| `bloop-app/src/hub.rs` | Unit tests with temporary folders and a fake `SecretStore`: unchanged text, changed text, invalid files, chat model reuse, the write of a change |
| `bloop-macos/src/paste.rs` and `bloop-macos/src/paste/clipboard.rs` | Manual checks: target check, one paste for each generation, clipboard restore |
| `bloop-macos/src/keys.rs` | Manual checks: hold threshold, combinations, Escape blocking |
| `bloop-macos/src/menu.rs` | Manual checks: one `menuNeedsUpdate:` for each open, checkmarks, ⌘Q, the tag guard, the Open items |

### Documentation

- The documents in `docs/` use English and ASD-STE100 (`docs/WRITING.md`).
- When a design decision changes, update `docs/STRUCTURE.md` and `docs/CONTRACTS.md` in the same commit.
- Record measured values separately from targets.

## Working rules

- Make the smallest change that satisfies the request.
- Add a dependency with `cargo add` to `[workspace.dependencies]` first, and check it with `cargo info`.
- The lint floor enforces this rule in non-test code: no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`,
  or `dbg!`.
- Commit messages: the subject is one imperative sentence with a capital letter, no type prefix
  (no `docs:` or `feat:`), and no final period. The body says what changed and why, wrapped at 72 characters.
