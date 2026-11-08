# Contributing

This document is for a developer or an agent who changes Bloop.
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) describes the design, and [AGENTS.md](AGENTS.md) is the maintenance contract that agents follow.

## Workspace

Bloop is a Rust 2024 virtual workspace with five crates in `crates/`. The manifests are the source of truth for versions, features, and dependencies.

| Crate | Role |
| --- | --- |
| `bloop-core` | Domain types, the session state machine, the text pipeline and steps, and the ports. No I/O, no async runtime, no platform API. Its only external dependency is `secrecy`. |
| `bloop-audio` | Microphone capture with `cpal`, the input device choice, the bounded sample queue, and the conversion to 16 kHz. |
| `bloop-providers` | The AI provider adapters. One module and one Cargo feature for each vendor. `mistral` is the default feature. |
| `bloop-macos` | AppKit integration: application, event tap, orb, paste and clipboard, status menu, permissions, Keychain, sleep observer, and workspace: the frontmost and open applications, file and folder opening, and the application panel. The only crate with `unsafe` code. |
| `bloop-app` | The application: settings, hub, driver, worker, menu model, logging, and the `eval` tool. |

`bloop-app` has two binaries: `bloop-app`, the application and `default-run`, and `eval`, the correction evaluation tool.
All crates depend on `bloop-core`. `bloop-macos`, `bloop-audio`, and `bloop-providers` do not depend on each other. Only `bloop-app` builds objects from the settings.

Supporting files:

| Path | Purpose |
| --- | --- |
| `prompts/*.md` | The built-in prompts, included in the binary with `include_str!` |
| `prompts/test-set-*.toml` | The French test sets of the `eval` tool |
| `bundle/Info.plist` | The bundle identity, `LSUIElement`, and the microphone usage text |
| `bundle/Bloop.icns` | The application icon, made by `scripts/icon.sh` |
| `assets/branding/` | The SVG sources: the logo, the README wordmarks, the application icon, and the menu bar icon |
| `scripts/bundle.sh` | Release build, application bundle, and code signature |
| `scripts/icon.sh` | The `.icns` file from `assets/branding/app-icon.svg` |
| `.claude/skills/rust-strict` | The Rust policy that agents follow, as a git submodule |

## Build and run

Clone with the submodule, so that the Rust policy is present:

```sh
git clone --recurse-submodules https://github.com/robinber/bloop.git
```

A plain `cargo build` compiles the workspace. Run the application from its bundle:

```sh
./scripts/bundle.sh
open target/release/Bloop.app
```

The script runs `cargo build --release --locked` for the `bloop-app` binary, copies `bundle/Info.plist` and `bundle/Bloop.icns` into `target/release/Bloop.app`, sets `CFBundleShortVersionString` from the package version, and signs the bundle with the identity `Bloop Dev`, or with `BLOOP_SIGN_IDENTITY` when it is set.
The bundle matters: `Info.plist` makes Bloop an accessory application without a Dock icon and declares the microphone usage, and the stable signature keeps the macOS permissions across builds.
The [README](README.md#installation) describes the signing identity, the API key, and the permissions.

To change the application icon, edit `assets/branding/app-icon.svg` and run `./scripts/icon.sh`. It renders the SVG with `qlmanage`, scales it with `sips`, and writes `bundle/Bloop.icns` with `iconutil`.
The menu bar icon is `assets/branding/status-icon.svg`, included in `bloop-macos` as a template image.

## Checks

Run the three gates before each commit:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The workspace has no CI. The lint floor is in `[workspace.lints]` of `Cargo.toml`: the rust-strict starter policy plus `raw_borrows_via_references`. Every crate inherits it with `[lints] workspace = true`.
Outside tests, it denies `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, and `dbg!`. `clippy.toml` allows them in tests.
`rustfmt.toml` sets `style_edition = "2024"`. The toolchain is stable, without `rust-toolchain.toml`. Verify with the default features.

## Tests

- A module with a large test suite keeps its tests in a sibling `tests.rs`, declared with `#[cfg(test)] mod tests;`. Examples: `session/tests.rs`, `driver/tests.rs`, `worker/tests.rs`, `hub/tests.rs`, `menu/tests.rs`, `paste/tests.rs`, and `mistral/transcribe/tests.rs`. A small suite stays at the end of its module.
- Fakes live in `fakes.rs`. `bloop-core/src/fakes.rs` has the fake chat model and the future driver of the step and pipeline tests. `bloop-app/src/worker/fakes.rs` has the scripted speech provider, chat model, and event sink of the worker and driver tests. `bloop-providers/src/test_log.rs` captures the log fields of a test, to check that no private value enters them.
- Provider tests start a local TCP server as the fake API. No test reaches the network.
- Settings tests write to a scratch folder under the system temporary directory and remove it.
- Clipboard tests use a pasteboard with a unique name, never the general clipboard.
- Never edit, delete, ignore, or weaken a test to get a green run. When a test disagrees with the intended behavior, stop and report it.

### Manual checks

No test covers the real permissions, microphone, network, or AppKit behavior. After a change to one of these surfaces, check on a Mac:

| Surface | Checks |
| --- | --- |
| `bloop-macos/src/keys.rs` | Short hold, Right Command combinations, Escape press, repeats, and release, and sleep during a capture. |
| `bloop-audio` | Microphone permission and device selection, silence, a long capture, and consecutive dictations. |
| `bloop-macos/src/paste.rs` and `paste/clipboard.rs` | Paste in a native application, a browser, and a terminal; a changed target; secure input; an unread clipboard item; text and image restoration; consecutive pastes; cancellation; and quit during a pending transaction. |
| `bloop-macos/src/menu.rs` and `bloop-app/src/menu.rs` | Refresh at open, checkmarks, submenus, ⌘Q, and the file and permission links. In **Prompt**, the checkmark on the submenu of the default prompt and on its **Default prompt**, a click on an application, and **Add application** with the open applications change `config.toml`. |
| **Choose…**: `bloop-macos/src/workspace.rs` | The panel comes to the front on `/Applications`. Choose Codex (`ChatGPT.app`) and check that `config.toml` gets `com.openai.codex`. A cancelled panel, and a chosen file that is not an application, change nothing. After the panel, the application that was in front before has the focus again. **Quit Bloop** while the panel is open closes the panel only. The open applications leave out Bloop and the applications without a regular policy. |
| Correction | Correction on and off, a failed correction, and manual recovery with **Copy last result**. |
| Prompt by application: `bloop-macos/src/workspace.rs` and `bloop-app/src/hub.rs` | With the correction on, a dictation in Ghostty uses `code`, in WhatsApp uses `natural-light`, and in Notes uses the default prompt. Check the `the dictation prompt is chosen` log line. |

Miri cannot run FFI calls to macOS frameworks. For `bloop-macos`, test the safe wrappers, review the drop and error paths, and run the manual checks.

## Evaluation tool

`eval` runs the correction steps of Bloop, the same `steps::correction` list as the application, on the samples of a test set with the real Mistral API.
It writes each sample, its final text, and a summary to the terminal, for manual review. A control character in a text, such as a line break or an escape, shows as its Rust escape, for example `\n`. The samples are invented and contain no private data.

```sh
cargo run --bin eval -- --prompt prompts/natural-light.md
cargo run --bin eval -- --set prompts/test-set-code-fr.toml --prompt prompts/code.md
```

| Option | Effect |
| --- | --- |
| `--set <path>` | The test set. Default `prompts/test-set-fr.toml`. |
| `--prompt <path>` | The prompt file. Default `prompts/natural-light.md`. |
| `--model <id>` | Another chat model of the same provider. Default: the model of the default chat settings. |
| `--validation` | Runs only the samples with `validation = true`, which are reserved for the check after a prompt change. Without it, the tool runs the other samples. |
| `--limit-ms <n>` | The time limit of each request, from 1 to 300000 ms. Default 30000. |
| `--help` | Shows the options. |

A test set is a TOML file with optional `[[dictionary]]` pairs with `written` and `heard`, and `[[sample]]` entries with a unique `id`, a `category`, the `raw` transcript, an optional `note` for the reviewer, and `validation`.
The categories are `question`, `negation`, `number`, `name`, `technical`, `spoken-correction`, `hesitation`, `instruction`, `long`, `punctuation`, `dictionary-fits`, and `dictionary-does-not-fit`.
`prompts/test-set-fr.toml` is for `natural-light`, and `prompts/test-set-code-fr.toml` is for `code`.

The tool reads the API key from Keychain, service `bloop`, account `mistral`. The `eval` binary is not in the access list of the key, so macOS shows an access dialog. Select **Allow** for one run, or **Always Allow**.

## Dependencies

Add a dependency with `cargo add` to `[workspace.dependencies]` first, then reference it in the crate with `workspace = true`. Check the crate with `cargo info` before you use its API.
Keep features additive. Provider dependencies are optional behind their vendor feature. `Cargo.lock` is under version control, and `scripts/bundle.sh` builds with `--locked`.

## Unsafe code

Only `bloop-macos` contains `unsafe`. Each module with `unsafe` carries `#![expect(unsafe_code, reason = "...")]`, and each block has a `// SAFETY:` comment. The other crates have `#![forbid(unsafe_code)]` at their root.
The approved surfaces are listed in [AGENTS.md](AGENTS.md). A callback from macOS must not panic.

## Commits

One commit covers one change, and the three gates pass before it.

- The subject is one imperative sentence with a capital letter, no type prefix such as `docs:` or `feat:`, and no final period.
- The body says what changed and why, wrapped at 72 characters.

## Documentation

Write the documents in English, with short sentences in the present tense. Describe the current behavior only: no plans, no history, and no measurements.
Keep the exact identifiers of the code, the menu, and the files, such as `hold_ms`, **Correct text**, and `config.toml`.
When a change alters the behavior, update `README.md`, `docs/SETUP.md`, or `docs/ARCHITECTURE.md` in the same commit, and the package facts of `AGENTS.md` when a manifest changes.

## License

Bloop is distributed under the Apache License, Version 2.0, in [LICENSE](LICENSE).
Unless you state otherwise, a contribution that you submit for inclusion in Bloop is licensed under the same terms, without additional terms or conditions.
