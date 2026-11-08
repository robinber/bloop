# Configuration and troubleshooting

This guide describes the files, the Keychain item, the permissions, and the log of Bloop, and what to do when a dictation fails.
The [README](../README.md#installation) covers the installation.

## Files

Bloop keeps its files in `~/Library/Application Support/Bloop/`:

| Path | Content | Size limit |
| --- | --- | --- |
| `config.toml` | The settings | 64 KiB |
| `dictionary.toml` | The dictionary | 256 KiB |
| `prompts/` | Your correction prompts, one `.md` file each | 32 KiB for each file |

At the first start, Bloop writes a commented `config.toml` with the default values, a commented `dictionary.toml` without entries, and the empty `prompts` folder.
It never changes an existing file at start. To get a commented file again, delete it, then quit and open Bloop again.

Bloop reads `config.toml` and `dictionary.toml` again at each dictation key press and each time the status menu opens.
When the correction is on, it also reads the prompt of the dictation: the prompt of the application in front at the key press, or the selected prompt. A change applies from the next dictation, without a restart.
A dictation that already started keeps the settings of its key press.

A file over its size limit, a file that is not UTF-8 text, or a file with an error prevents dictation until you fix it.
The menu shows the file and, when the parser gives it, the line. The menu and the log never show the content of a file.

## Settings file

Each key of `config.toml` is optional. A missing key has its default value. Each table rejects an unknown key.

| Key | Default | Values |
| --- | --- | --- |
| `schema` | `1` | Only `1`. |
| `[dictation] hold_ms` | `220` | The hold time of the dictation key before the recording starts, from 100 to 1000 ms. The **Hold delay** submenu lists 150, 220, 300, and 400 ms. Another value of the file shows at the end of the list. |
| `[dictation] language` | `"fr"` | The language of the dictation: two lowercase ASCII letters. Bloop sends it to the speech provider. |
| `[transcriber] provider` | `"mistral"` | The speech provider. The key is required when the table exists. `mistral` is the only provider in this build. |
| `[transcriber] model` | `"voxtral-mini-transcribe-realtime-3-0"` | The realtime transcription model. |
| `[transcriber] target_streaming_delay_ms` | `480` | The balance between delay and accuracy, in milliseconds. Bloop sends the value to the provider without a local range check. |
| `[chat] provider` | `"mistral"` | The correction provider. The key is required when the table exists. |
| `[chat] model` | `"mistral-medium-latest"` | The correction model. The **Model** submenu selects it. |
| `[chat] models` | `["mistral-medium-latest", "mistral-small-latest"]` | The models that the **Model** submenu lists. |
| `[chat] reasoning_effort` | `"none"` | The `reasoning_effort` value of the chat request. The provider checks the value. |
| `[refine] enabled` | `false` | The state of **Correct text**. |
| `[refine] prompt` | `"natural-light"` | The default correction prompt: a built-in name, or a file of the prompts folder without `.md`. 1 to 64 characters from `A-Z`, `a-z`, `0-9`, `_`, and `-`. The **Prompt** submenu selects it. |
| `[refine.apps]` | The built-in lists | The applications of each prompt: one key for each prompt name, with a list of bundle identifiers. A bundle identifier has 1 to 255 characters from `A-Z`, `a-z`, `0-9`, `.`, `-`, and `_`. See [Prompt by application](#prompt-by-application). |

The sampling temperature of the correction is 0. The file cannot change it.
Model identifiers and provider options go to the API as they are. A valid file does not guarantee that the provider accepts them.

### Changes from the menu

**Correct text** and the **Prompt**, **Model**, and **Hold delay** submenus write their key to `config.toml`. Bloop keeps the comments and the other keys.
A missing table goes at the end of the file. A model change writes the `provider` key of `[chat]` when the table does not have it.
**Prompt by app** writes the lists of `[refine.apps]` that change. A list without a key gets its key with the whole list. A new `[refine.apps]` table goes directly after `[refine]`.
A changed list keeps the comment above its key, but loses the comments inside a list on several lines.

Bloop parses the current file before a write and refuses to change an invalid file. It also refuses a change that makes the file larger than 64 KiB.
The write goes to a temporary file in the same folder, which then replaces `config.toml`.
Each time you open the menu, the checkmarks show the values of the file, including a change that you made by hand.

### Errors

An invalid file shows one line in the menu and hides **Correct text** and the submenus:

- `config.toml: invalid setting, line N` for a syntax error, a wrong type, a value out of its limits, an unknown key, or a `schema` other than `1`.
  The same application in two lists of `[refine.apps]` also gives this error, with the line of the `[refine.apps]` header.
- `config.toml: file too large` and `config.toml: file not readable`.

In `[transcriber]` and `[chat]`, an unknown key or a wrong type gives the line of the table header. Select the line to open the file.

## Prompts

The correction sends a prompt to the chat model as its system message, and the transcript as its user message. Bloop has two built-in prompts:

| Prompt | Use |
| --- | --- |
| `natural-light` | The default prompt, for everyday text. It adds punctuation and capital letters, removes hesitations and stutters, and applies spoken corrections. It keeps the spoken words, the negations, the numbers, and the register. |
| `code` | Technical dictation, for example to a coding agent or a terminal. It applies the same corrections, writes technical terms in their usual form, and writes a spoken symbol as the symbol. |

With `code`, "point", "slash", "tiret", "underscore" or "tiret du bas", "arobase", "dièse", and "deux points" become `.`, `/`, `-`, `_`, `@`, `#`, and `:` in a technical context.
For example, "package point json" becomes `package.json`, "tiret tiret workspace" becomes `--workspace`, and "arobase types slash node" becomes `@types/node`.
When the whole dictation is a command line, `code` writes it without a capital letter and without a final period.

The built-in prompt files are `prompts/natural-light.md` and `prompts/code.md` in this repository. Select the default prompt in the **Prompt** submenu.

### Your own prompt

1. Write the prompt in a `.md` file of `~/Library/Application Support/Bloop/prompts/`, for example `mail.md`. To start from a built-in prompt, copy `prompts/natural-light.md` or `prompts/code.md` from this repository. Tell the model that the transcript comes between a `<transcript>` line and a `</transcript>` line, because the request puts it there.
2. Select **mail** in the **Prompt** submenu, or set `prompt = "mail"` in `[refine]`.

A file with the name of a built-in prompt replaces it. The **Prompt** submenu lists the built-in names and the `.md` files whose name is a valid prompt name, sorted and without duplicates.
A change in a prompt file applies from the next dictation.

A prompt file must contain UTF-8 text that is not only whitespace, with at most 32 KiB.
When the selected prompt is missing or not valid and **Correct text** is on, each dictation that uses it fails, and the menu shows `Prompt "name": not found` or `Prompt "name": not valid`. Select the line to open the prompts folder.
With **Correct text** off, Bloop does not read a prompt, and dictation operates.

### Prompt by application

When **Correct text** is on, the application in front at the dictation key press selects the prompt. When the list of a prompt contains the bundle identifier of the application, the dictation uses that prompt.
Another application uses the default prompt of the **Prompt** submenu. A browser uses the default prompt too: Bloop does not select a prompt by website.

Two prompts have a built-in list:

| Prompt | Applications |
| --- | --- |
| `code` | Terminal `com.apple.Terminal`, Ghostty `com.mitchellh.ghostty`, iTerm2 `com.googlecode.iterm2`, Warp `dev.warp.Warp-Stable`, Visual Studio Code `com.microsoft.VSCode`, Cursor `com.todesktop.230313mzl4w4u92`, Zed `dev.zed.Zed`, Xcode `com.apple.dt.Xcode`, Claude `com.anthropic.claudefordesktop` |
| `natural-light` | Messages `com.apple.MobileSMS`, WhatsApp `net.whatsapp.WhatsApp`, Telegram `ru.keepcoder.Telegram`, Telegram Desktop `org.telegram.desktop`, Signal `org.whispersystems.signal-desktop` |

To change the lists, add a `[refine.apps]` table to `config.toml`:

```toml
[refine.apps]
code = ["com.apple.Terminal", "com.mitchellh.ghostty"]
mail = ["com.apple.mail"]
```

- A key replaces the built-in list of its prompt, even with an empty list. A prompt without a key keeps its built-in list.
- A key can name your own prompt, such as `mail` for `prompts/mail.md`.
- An application in a key leaves the built-in lists. When you remove it from the key, it returns to its built-in list, if it has one.
- The same application in two keys is an error. An application repeated in one list counts one time.
- The comparison of bundle identifiers is case-sensitive. Copy the identifier from the log, as described below.

The **Prompt by app** submenu shows and changes the lists. Its first line gives the prompt of the application in front, for example `Ghostty uses code` or `Notes uses the default prompt`. Then it has one submenu for each prompt, with its applications:

- Select an application to remove it from the list.
- Select **Add** followed by the name of the application in front to add it to the list. Bloop removes it from the other lists.

The submenus show the name of the installed application, else the name of the built-in list, else the bundle identifier. A key of `[refine.apps]` that names no prompt also has a submenu, without **Add**, so that you can remove its applications.

To find the bundle identifier of an application, dictate one time in it, then search the log for `the target is recorded`. The line gives `bundle_id`.

When the prompt of a list is missing or not valid and **Correct text** is on, each dictation in its applications fails, and the menu shows `Prompt "name": not found` or `Prompt "name": not valid`. The other applications operate.

## Dictionary

`dictionary.toml` gives the speech provider the written forms of names and technical terms. Bloop sends them with each dictation, as the vocabulary of the transcription session.
Select **Open dictionary…** to edit the file.

Add one entry for each written form:

```toml
[[entry]]
written = "Tokio"

[[entry]]
heard = "obj c 2"
written = "objc2"
```

Each entry needs a `written` value that is not only whitespace. The optional `heard` field is metadata: it does not affect the transcription or the correction.
A written form that occurs twice goes to the provider one time. Without the file, the dictionary is empty. A change applies from the next dictation.

An invalid file prevents dictation. The menu shows `dictionary.toml: invalid entry, line N` for a syntax error, a wrong type, a missing `written` key, an unknown key, or an empty written form, and `dictionary.toml: file too large` or `dictionary.toml: file not readable`.
Select the line to open the file.

## API key

Bloop reads the API key from the login keychain as a generic password with the service `bloop` and the account `mistral`. The [README](../README.md#store-the-api-key) gives the `security` command.
Transcription and correction use the same key.

Bloop reads the key one time at start, because a Keychain read can wait for a dialog. After you add or change the key, quit and open Bloop again.

- Without a key, the menu shows **Add the API key to Keychain**, and each dictation fails.
- When Keychain has the key but refuses the read, the menu shows **Allow Bloop to read the API key in Keychain**. This happens when Bloop is not in the access list of the key. Use the `-T` option of the `security` command, or select **Always Allow** in the dialog of macOS.
- When the provider rejects the key, the menu shows **The provider refused the API key** until the next start.

Only the transcription reports a refused key in the menu. A failed correction request, including one with a refused key, keeps the raw transcript.

## Permissions

| Permission | Use | System Settings pane |
| --- | --- | --- |
| Microphone | The recording | **Privacy & Security > Microphone** |
| Input Monitoring | The event tap that sees the dictation key and Escape in every application | **Privacy & Security > Input Monitoring** |
| Accessibility | The active event tap, which blocks Escape during a dictation, and the paste command | **Privacy & Security > Accessibility** |

At start, Bloop asks macOS for each missing permission. macOS shows its own request one time for each permission. After that, enable Bloop in System Settings.
The menu lists each missing permission as **Allow Microphone access…**, **Allow Accessibility access…**, or **Allow Input Monitoring…**, and a click opens its pane.
macOS can keep the old result until Bloop starts again. After you change a permission, quit and open Bloop again.
When macOS refuses the event tap, the menu shows **Open Bloop again to use the dictation key**.

Bloop records with the default input device. When the default input is a Bluetooth device, Bloop records with the built-in microphone of the Mac instead, when the Mac has one.

## Log

Bloop writes its log to `~/Library/Logs/Bloop/bloop.log`.
At start, a file larger than 5 MiB moves to `bloop.log.1` and replaces the previous backup. The active file can grow beyond 5 MiB until the next start.

The log contains the events and commands of each dictation, durations, sample and character counts, the nonsecret settings of each snapshot, the bundle identifier of the application in front at key press and the prompt that it selects when **Correct text** is on, the bundle identifier of an application that you add or remove in the menu, the name and bundle identifier of the application in front at key release, controlled error categories and HTTP status codes, and the location of a panic.
It never contains audio, transcript text, prompt text, dictionary entries, file content, or the API key.

## Troubleshooting

- **The menu lists a permission.** Click the line to open System Settings, enable Bloop, then quit and open Bloop again.
- **The menu asks for the API key.** Store the key with the `security` command of the README, then quit and open Bloop again.
- **The provider refused the API key.** Store a valid key, then quit and open Bloop again.
- **Red bars after a file edit.** The menu shows the file error and its line. Click the line to open the file or the prompts folder. Dictation operates again after you fix the file.
- **Red bars without a file error.** The dictation failed: a network error, a provider error, a full audio queue, or a lost microphone. The log gives the category.
- **The text was not inserted.** Select **Copy last result**, then paste by hand. Bloop does not paste when another application came to the front, when you typed or clicked after the key release, when a password field has secure input, or when the application did not read the clipboard within one second.
- **Nothing happens when I hold Right Command.** Check the menu for a missing permission or a refused event tap. Wait for the indicator before you speak: the recording starts after the hold delay. Right Command with another key is a shortcut, not a dictation.
- **The paste went to another field.** Bloop checks the application in front, not the focused field. Click in the target field before you hold the key.
- **A Bluetooth headset is connected.** Bloop records with the built-in microphone, not the headset.
- **The dictation used the prompt of another application.** The prompt comes from the application in front at the key press. A swipe to another Space, or an application that comes to the front during the dictation, does not change it. Bring the target application to the front before you hold the key.
