# Setup

Run the commands below from the repository root.

## Signing identity

Bloop uses a stable code signing identity to keep macOS permissions across builds.

Create the identity once in Keychain Access:

1. Select **Keychain Access > Certificate Assistant > Create a Certificate**.
2. Set the name to `Bloop Dev`.
3. Set **Identity Type** to **Self Signed Root**.
4. Set **Certificate Type** to **Code Signing**.
5. Select **Let me override defaults** and set the validity period to 3650 days.
6. Complete the certificate creation.

## Build

```sh
./scripts/bundle.sh
```

The script builds and signs `target/release/Bloop.app`.
At the first signature, macOS asks for access to the signing key. Select **Always Allow**.
To use a different identity, set `BLOOP_SIGN_IDENTITY` when you run the script.

## API key

After the first build, store your Mistral API key in Keychain:

```sh
security add-generic-password -s bloop -a mistral -T "$PWD/target/release/Bloop.app" -U -w
```

The command asks for the key, so the key does not enter the shell history.
The `-T` option lets the signed app read the key without an access dialog.
The Keychain service is `bloop`, and the account is `mistral`.
Transcription and correction use the same key.

## First start

```sh
open target/release/Bloop.app
```

Allow these permissions in **System Settings > Privacy & Security** when macOS requests them:

- **Microphone** to record your voice.
- **Input Monitoring** to detect the dictation key.
- **Accessibility** to paste the text.

After changing permissions, quit and open Bloop again.
The menu shows missing permissions or a missing API key. When setup is complete, it shows **Ready: hold Right Command to dictate**.

Bloop uses the default microphone.
If the default input is a Bluetooth device, Bloop uses the built-in microphone when one is available.

## Dictionary

The dictionary file `~/Library/Application Support/Bloop/dictionary.toml` gives the speech provider names and technical terms.
At the first start, Bloop writes this file with comments and no entries. Bloop never changes an existing file at start.
Select **Open dictionary…** in the menu to edit it.

Add one entry for each written form:

```toml
[[entry]]
written = "objc2"

[[entry]]
written = "Voxtral"
```

Each entry needs a non-empty `written` value. The file size is at most 256 KiB.
The optional `heard` field stores the form that Bloop writes now. It does not affect transcription or correction yet.

Bloop reads the dictionary again at each dictation. A change applies from the next dictation, without restart.
Without the file, the dictionary is empty. An invalid file prevents dictation. The menu shows the error and its line.

## Settings file

The settings file is `~/Library/Application Support/Bloop/config.toml`.
At the first start, Bloop writes this file with comments and the default values. Bloop never changes an existing file at start.
To get the commented file again, delete the file, then quit and open Bloop again.
Select **Open config.toml…** in the menu to edit the file.

Each key is optional. A missing key has its default value:

| Key | Default value | Values |
| --- | --- | --- |
| `schema` | `1` | Only `1` |
| `[dictation] hold_ms` | `220` | The hold time of the dictation key before the recording starts, from 100 to 1000 ms |
| `[dictation] language` | `"fr"` | Two lowercase ASCII letters |
| `[transcriber] provider` | `"mistral"` | The speech provider. It is necessary when the table exists. |
| `[transcriber] model` | `"voxtral-mini-transcribe-realtime-3-0"` | The transcription model |
| `[transcriber] target_streaming_delay_ms` | `480` | The balance between delay and accuracy, in milliseconds |
| `[chat] provider` | `"mistral"` | The correction provider. It is necessary when the table exists. |
| `[chat] model` | `"mistral-medium-latest"` | The correction model |
| `[chat] models` | `["mistral-medium-latest", "mistral-small-latest"]` | A list of correction models. It has no effect for now. |
| `[chat] reasoning_effort` | `"none"` | The reasoning effort of the correction model |
| `[refine] enabled` | `false` | The state of **Correct text** |
| `[refine] prompt` | `"natural-light"` | A prompt name: 1 to 64 characters from `A-Z`, `a-z`, `0-9`, `_`, and `-` |

Each table rejects an unknown key. The file size is at most 64 KiB.
When you click **Correct text**, Bloop writes `[refine] enabled` and keeps your comments.

A change in the file applies from the next dictation, without restart. A new API key still needs a restart.
Each time you open the menu, the checkmark of **Correct text** shows the value in the file.
An invalid file prevents dictation. The menu shows the line of the key, or of its table, and hides **Correct text**.
In `[transcriber]` and `[chat]`, an unknown key or a wrong type gives the line of the table.

### Prompts

The correction sends a prompt to the chat model. The built-in prompt is `natural-light`.
Bloop creates the folder `~/Library/Application Support/Bloop/prompts/` at start. It contains only your prompts.
Select **Open prompts folder…** in the menu to open it.

To use your own prompt:

1. Write the prompt in a file of the prompts folder, for example `dev.md`.
   To start from the built-in prompt, copy `prompts/natural-light.md` of this repository.
2. Set `prompt = "dev"` in `[refine]`.

The next dictation uses the prompt. A change in the prompt file also applies from the next dictation.

A file with the name of a built-in prompt replaces it. For example, `natural-light.md` replaces the built-in prompt.
A prompt file must contain UTF-8 text that is not empty. Its size is at most 32 KiB.
If the selected prompt is missing or not valid, each dictation with **Correct text** fails. Without **Correct text**, dictation operates.
The menu shows the prompt name and the reason.

## Evaluate the correction

The `eval` tool runs the correction steps of Bloop on the French test set `prompts/test-set-fr.toml`, with the real API.
It writes each sample, its final text, and a summary to the terminal.

```sh
cargo run --bin eval -- --prompt prompts/natural-light.md
```

- Without `--validation`, the tool runs the samples that are not reserved. With `--validation`, it runs only the reserved samples.
- `--model` selects another model of the same provider. `--limit-ms` sets the time limit of each request: 30,000 ms by default.
- `cargo run --bin eval -- --help` shows all options.

The tool reads the API key from Keychain. Each build of `eval` is a new program for Keychain, so macOS asks for access to the key.
Select **Always Allow** to stop the request until the next build, or **Allow** to give access for this run only.
The tool sends each sample to the provider. The samples are invented and contain no private data.

## Troubleshooting

- **No recording:** check the menu for missing permissions, then quit and open Bloop again.
- **Missing API key:** add the key with the command above, then quit and open Bloop again.
- **The provider refused the API key:** store a valid key with the command above, then quit and open Bloop again.
- **Red recording indicator after a file edit:** the menu shows the file and the line of the error.
  Select the line to open the file. An invalid file prevents dictation until you fix it.
- **Text was not inserted:** select **Copy last result** in the menu, then paste manually.

Logs are in `~/Library/Logs/Bloop/bloop.log`.
