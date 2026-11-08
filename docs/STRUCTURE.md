# Code structure

Bloop is a Rust 2024 workspace with five crates. The manifests define versions, features, and dependencies.
The application supports macOS on Apple silicon. All workspace packages have `publish = false`.

## Crate boundaries

```text
                    bloop-app
                  /     |     \
        bloop-macos bloop-audio bloop-providers
                  \     |     /
                    bloop-core
```

- `bloop-core` has no I/O, runtime, or platform API. Its only external dependency is `secrecy`.
- `bloop-macos`, `bloop-audio`, and `bloop-providers` do not depend on each other.
- `bloop-app` constructs adapters from settings and connects them to the driver.
- HTTP, WebSocket, and vendor-specific code stay in `bloop-providers`.
- Only `bloop-macos` contains `unsafe` code. Its approved surfaces are in [AGENTS.md](../AGENTS.md).
- The AI ports return `BoxFuture`, so adapters can be selected through trait objects.

## Source map

| Crate | Modules and responsibilities |
| --- | --- |
| `bloop-core` | `domain`: audio and text types; `session`: events, commands, and job order; `ports`: AI and platform interfaces; `error`: typed failures |
| `bloop-core` | `pipeline`: ordered text steps and fallback; `steps/`: correction, typography, and trim; `dictionary`: entries and written forms |
| `bloop-audio` | `capture`: microphone stream; `device`: input selection; `buffer`: bounded sample queue; `convert`: incremental conversion to 16 kHz |
| `bloop-providers` | `lib`: provider settings and constructors; `http`: shared HTTP client; `mistral/`: configuration, streaming transcription, and chat |
| `bloop-macos` | `app`: AppKit lifecycle; `events` and `scheduler`: main queue; `keys`: event tap; `sleep`: sleep notification |
| `bloop-macos` | `paste` and `paste/clipboard`: target checks and clipboard transactions; `orb/`: indicator and cards; `menu`: AppKit menu renderer |
| `bloop-macos` | `keychain`: credentials; `permissions`: permission checks and settings links; `workspace`: file and folder opening |
| `bloop-app` | `main`: construction; `driver`: command execution and generation resources; `worker`: streaming task and text pipeline |
| `bloop-app` | `settings/`: schema, template, and edits; `files`: bounded reads and file writes; `hub`: refresh, settings snapshots, and adapter reuse |
| `bloop-app` | `menu`: menu model and actions; `prompts`: built-in and user prompts; `dictionary`: file parsing; `logging`: file logs |
| `bloop-app` | `bin/eval/`: correction evaluation, arguments, sample sets, and reports |

`bloop-app` is the default binary. `eval` is a separate binary that sends test samples to the real correction API.
Both binaries use `steps::correction` and the same pipeline implementation.
A module with a large test suite keeps its tests in a sibling `tests.rs` file, declared with `#[cfg(test)] mod tests;`.

## Supporting files

| Path | Purpose |
| --- | --- |
| `prompts/` | Built-in prompts and French evaluation sets |
| `bundle/Info.plist` | Application identity, icon file, accessory mode, and microphone permission text |
| `bundle/Bloop.icns` | Application icon, made by `scripts/icon.sh` |
| `scripts/bundle.sh` | Release build, application bundle, and code signature |
| `scripts/icon.sh` | Application icon from `assets/branding/app-icon.svg` |
| `assets/branding/` | Logo sources: README wordmarks, application icon, and menu bar icon |
| `Cargo.lock` | Resolved dependency versions |
| `rustfmt.toml`, `clippy.toml`, `Cargo.toml` | Formatting and lint policy |

The default provider feature is `mistral`. Provider dependencies are optional behind that feature.
The workspace has no CI configuration and no pinned toolchain file.

## Maintenance references

- [Architecture](ARCHITECTURE.md): execution, data flow, and limits.
- [Contracts](CONTRACTS.md): invariants and verification surfaces.
- [Setup](SETUP.md): installation, settings, prompts, and evaluation.
- [Writing rules](WRITING.md): documentation language and terminology.

Use the source declarations for exact Rust signatures. Do not duplicate them in design documents.
