<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/branding/wordmark-dark.svg">
    <img src="assets/branding/wordmark.svg" alt="Bloop" width="320">
  </picture>
</h1>

Bloop is a macOS menu bar app for French dictation, written in Rust with native AppKit.
Hold **Right Command**, speak, and release the key. Bloop streams your voice to Mistral while you speak, completes the transcript, corrects it with a chat model when you ask for it, and pastes the text into the field that had the focus.
It has no window and keeps no recording.

## Features

- Streaming transcription with the Mistral Voxtral realtime model. One WebSocket connection serves each dictation.
- Optional correction by a Mistral chat model, switched on and off from the menu bar. Two built-in prompts, `natural-light` for everyday text and `code` for technical dictation, and your own prompt files.
- A dictionary of names and technical terms that the speech model receives with each dictation.
- A small indicator at the bottom of the screen. It shows the icon of the target application and follows your voice level.
- A paste into the application that had the focus at key release, then the restoration of your previous clipboard.
- A recovery card and a **Copy last result** menu item when a paste is not possible.
- Consecutive dictations. A new dictation can start while earlier ones are still in progress, and Bloop pastes them in order.
- No maximum recording duration.

The default language is French, and the built-in prompts are written for French. The language setting accepts another two-letter code.

## Requirements

- A Mac with Apple silicon. Bloop supports macOS on arm64 only.
- Rust 1.99 or later. The workspace declares `rust-version = "1.99"`.
- Xcode Command Line Tools, for the linker and `codesign`.
- A Mistral API key. Transcription and correction use the same key.
- An internet connection. Bloop sends the audio and the text to the Mistral API.

## Installation

Run the commands from the repository root.

### Clone the repository

```sh
git clone https://github.com/robinber/bloop.git
cd bloop
```

### Create a signing identity

Bloop signs its application bundle with a stable code signing identity, so that macOS keeps the permissions across builds.
Create the identity one time in Keychain Access:

1. Select **Keychain Access > Certificate Assistant > Create a Certificate**.
2. Set the name to `Bloop Dev`.
3. Set **Identity Type** to **Self Signed Root**.
4. Set **Certificate Type** to **Code Signing**.
5. Select **Let me override defaults** and set the validity period to 3650 days.
6. Complete the certificate creation.

To use another identity, set `BLOOP_SIGN_IDENTITY` when you run the build script.

### Build

```sh
./scripts/bundle.sh
```

The script builds the release binary, writes `target/release/Bloop.app` with its `Info.plist` and icon, and signs the bundle.
At the first signature, macOS asks for access to the signing key. Select **Always Allow**.

### Store the API key

```sh
security add-generic-password -s bloop -a mistral -T "$PWD/target/release/Bloop.app" -U -w
```

The command asks for the key, so the key does not enter the shell history.
The Keychain service is `bloop` and the account is `mistral`. The `-T` option lets the signed app read the key without an access dialog.
Bloop reads the key at start. After you change the key, quit and open Bloop again.

### First launch

```sh
open target/release/Bloop.app
```

Bloop appears in the menu bar, without a Dock icon. Allow these permissions when macOS asks for them, or in **System Settings > Privacy & Security**:

| Permission | Why Bloop needs it |
| --- | --- |
| **Microphone** | To record your voice while you hold the dictation key. |
| **Input Monitoring** | To see the dictation key and the Escape key in every application. |
| **Accessibility** | To run the active event tap, which blocks Escape during a dictation, and to post the paste command. |

After you change a permission, quit and open Bloop again. The menu lists each missing permission, with a link to its System Settings pane, and a missing API key.
When the setup is complete, the menu shows **Ready: hold Right Command to dictate**.

At the first start, Bloop creates `~/Library/Application Support/Bloop/` with a commented `config.toml`, an empty `dictionary.toml`, and a `prompts` folder.

## Usage

1. Click in a text field.
2. Hold **Right Command**. After the hold delay, 220 ms by default, the indicator opens at the bottom of the screen. Then speak.
3. Release the key. Bloop completes the transcript, corrects it when **Correct text** is on, and pastes it.

Bloop records as long as you hold the key. There is no maximum duration.

- A short press, shorter than the hold delay, does nothing, except show the card of a failed paste that waits for the key release.
- **Right Command** with another key or a click is a normal shortcut. It cancels the dictation silently, so your shortcuts keep working.
- **Escape** during a dictation cancels it. Bloop blocks that Escape press, so the application does not receive it. Escape does not cancel the dictations that you already released.
- Up to three released dictations can be in progress at the same time. While three are in progress, a new hold shows red bars and records nothing. Red bars also report a failed dictation, except while you hold **Right Command** for a new one.
- System sleep cancels the dictation and all the pending ones.

### The status menu

| Item | Effect |
| --- | --- |
| **Correct text** | Turns the correction on or off for the next dictations. The checkmark shows the value of the settings file. |
| **Prompt** | Has one submenu for each prompt: the built-in `natural-light` and `code`, and the `.md` files of your prompts folder. A checkmark marks the default prompt. In the submenu of a prompt, **Default prompt** selects it, a click on an application removes it from the list of the prompt, and **Add application** adds an open application, or another one with **Choose…**. |
| **Model** | Selects the correction model from the list in the settings file. |
| **Hold delay** | Selects the hold time of the dictation key: 150, 220, 300, or 400 ms. |
| **Copy last result** | Copies the text of the latest dictation that Bloop tried to paste. |
| **Open config.toml…**, **Open prompts folder…**, **Open dictionary…** | Open the settings file, the prompts folder, and the dictionary file. |
| **Quit Bloop** (⌘Q) | Quits. Bloop first restores a clipboard that a paste still holds. |

Each choice writes its key, or its lists, to `config.toml` and keeps your comments. A choice that keeps the text of the file, such as the current model, writes nothing. A change applies from the next dictation.
With **Correct text** on, the application in front selects the prompt. By default, a terminal, a code editor, Claude, or Codex uses `code`, a messaging application uses `natural-light`, and another application uses the default prompt. The [configuration guide](docs/SETUP.md#prompt-by-application) lists the applications and how to change them.
The top of the menu reports what needs your attention: a missing permission, a missing or refused API key, or an invalid file with its line.

### When a paste is not possible

Bloop pastes only when the same application is still in front, no key press or click happened since you released the key, and no password field has secure input.
Otherwise, or when the application does not read the clipboard within one second, the indicator shows a card with the start of the text for eight seconds. If another dictation fails during that time, its red bars replace the card.
Bloop leaves the text in the clipboard as a normal item. When the application did not read the clipboard and another application changed it in the meantime, Bloop keeps that new content instead.
Select **Copy last result** to copy the text again, then paste it by hand.

After a successful paste, Bloop restores your previous clipboard 300 ms after the application reads the text.

## Configuration

Bloop has no settings window. Its files are in `~/Library/Application Support/Bloop/`:

| File | Content |
| --- | --- |
| `config.toml` | The hold delay, the language, the transcription and correction models, and the correction state and prompt. |
| `dictionary.toml` | The names and technical terms that the speech model must spell correctly. |
| `prompts/` | Your correction prompts, one `.md` file each. |

Bloop reads these files again at each dictation key press and each time the menu opens. A change applies from the next dictation, without a restart.
The [configuration guide](docs/SETUP.md) lists every key, its default value, and its limits.

## Privacy

- **What leaves your Mac.** During a dictation, Bloop sends the audio, as 16 kHz PCM, the language, and the written forms of your dictionary to the Mistral realtime transcription API. When **Correct text** is on, it also sends the complete transcript and the prompt of the dictation to the Mistral chat completions API. Bloop connects to no other service.
- **What Bloop stores.** The settings file, the dictionary, your prompt files, and a log. Bloop writes no audio file and no history of your dictations. The last result stays in memory for **Copy last result** until a newer result replaces it or Bloop quits.
- **The API key.** It lives in macOS Keychain. Bloop never writes it to the settings, the log, an error message, or a command line.
- **The log.** `~/Library/Logs/Bloop/bloop.log` records events, durations, character counts, nonsecret settings such as the model identifiers, the bundle identifier of the application in front at key press when **Correct text** is on, the name and bundle identifier of the application in front at key release, and error categories. It never contains audio, transcript text, prompt text, dictionary entries, or the key.

## Troubleshooting

See the [troubleshooting section](docs/SETUP.md#troubleshooting) of the configuration guide: missing permissions, a refused API key, an invalid settings file, and a text that was not inserted.

## Development

Bloop is a Rust 2024 workspace with five crates. [CONTRIBUTING.md](CONTRIBUTING.md) describes the layout, the build, the checks, and the tests. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) describes the design.

## License

Bloop is distributed under the [Apache License, Version 2.0](LICENSE).
