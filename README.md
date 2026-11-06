<img src="assets/branding/logo.svg" alt="Bloop logo" width="96" height="96">

# Bloop

Bloop is a macOS menu bar app for French dictation, built with Rust and native AppKit.

Hold **Right Command**, speak, then release the key to insert your words into the focused text field.

## Features

- Streaming transcription with Mistral Voxtral.
- Optional text correction with a Mistral chat model.
- A custom dictionary for names and technical terms.
- A small recording indicator that follows your voice level.
- Clipboard restoration after a successful paste, with manual text recovery from the menu.

Bloop is in active development for personal use. It has been tested on one Apple silicon Mac running macOS 27.

## Installation

You need an Apple silicon Mac, Rust 1.99 or later, Xcode Command Line Tools, and a Mistral API key.
Bloop requires an internet connection.

Clone the repository:

```sh
git clone https://github.com/robinber/bloop.git
cd bloop
```

Follow the [setup guide](docs/SETUP.md) to create a signing identity and store your API key in Keychain.
The guide covers the build and macOS permissions.

After setup, build and open Bloop with:

```sh
./scripts/bundle.sh
open target/release/Bloop.app
```

## Usage

1. Click a text field.
2. Hold **Right Command**. Wait for the recording indicator, then speak.
3. Release the key. Bloop completes the transcript and pastes the text.

Press **Escape** during recording to cancel. Bloop sets no maximum recording duration.

In the menu bar, select **Correct text** to enable correction for subsequent dictations.
Correction is off each time Bloop starts. If correction fails or exceeds eight seconds, Bloop uses the raw transcript.

Select **Copy last result** to recover the most recent result for manual paste.
See the [dictionary setup](docs/SETUP.md#dictionary) to add names and technical terms.

## Data

Audio and dictionary terms go directly to Mistral for transcription.
When correction is enabled, the transcript also goes to the Mistral chat model.
API keys stay in macOS Keychain.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- [Architecture](docs/ARCHITECTURE.md)
- [Code structure](docs/STRUCTURE.md)
- [Roadmap](docs/ROADMAP.md)
