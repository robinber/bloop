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

Create `~/Library/Application Support/Bloop/dictionary.toml` to give the speech provider names and technical terms.
Create the parent directory first if needed:

```sh
mkdir -p "$HOME/Library/Application Support/Bloop"
```

Add one entry for each written form:

```toml
[[entry]]
written = "objc2"

[[entry]]
written = "Voxtral"
```

Each entry needs a non-empty `written` value.
The optional `heard` field stores the form that Bloop writes now. It does not affect transcription or correction yet.

Bloop reads the dictionary at start. After editing the file, quit and open Bloop again.
Without the file, the dictionary is empty.

## Settings file

Bloop keeps the state of **Correct text** in `~/Library/Application Support/Bloop/config.toml`, in the key `[refine] enabled`.
Bloop writes the file when you click the item, and keeps your comments.
For now, the file accepts only `schema = 1` and `[refine] enabled`.
After you edit the file, quit and open Bloop again. An invalid file prevents dictation. The log gives the line of the error.

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
- **Red recording indicator after a dictionary edit:** check the TOML syntax and each `written` value.
  An invalid dictionary prevents dictation until you fix the file and restart Bloop.
- **Text was not inserted:** select **Copy last result** in the menu, then paste manually.

Logs are in `~/Library/Logs/Bloop/bloop.log`.
Dictionary errors include the line number when it is available.
