# Scope

Status: Design draft, revision 11 (2026-11-06). Phase 1 and Phase 2 are complete.

## Goal

Bloop does one task: dictation into the focused text field.
The behavior follows the dictation feature of Tabi.
After Phase 2, Bloop adds features around dictation. The status menu turns each feature on and off.
A feature that is off does not change the dictation path. Low latency stays a priority.

## Confirmed choices

- The project name is Bloop.
- Bloop is for personal use on the owner's Mac.
- All application code uses Rust.
- The interface uses native AppKit through `objc2`. Bloop has no web view.
- The speech input and text output use French.
- Bloop is provider-agnostic. The settings select the speech provider and the chat provider.
- Mistral is the default provider: Voxtral converts speech to text, and a Mistral chat model corrects the text.
- An optional text step corrects the text. The correction is off by default.
- The user can edit the correction prompts and add other prompts.
- Streaming is the only STT path. Audio goes to the provider while capture continues.
- Phase 1 proves the key, orb, and insertion with fixed text. Phase 2 adds streaming. Phase 3 adds the optional correction.
- Phase 3 adds a correction toggle and validates daily use. Bloop keeps the toggle state in `config.toml`.
- Phase 4 adds persistent settings and more menu choices when needed.
- A dictionary gives the written form of names and terms (Phase 3).
  After the user edits a pasted text, Bloop proposes new entries (Phase 6).
- The user holds Right Command to speak.
- Bloop sets no maximum capture duration. The owner can reconsider this after daily use.
- A small orb shows the session state. The orb is part of the MVP.
- The application does not send messages or execute commands.
- The repository uses English and ASD-STE100 writing rules.

The reference Mac uses macOS 27.0.1 and the arm64 architecture.
These values came from a local system check on 2026-10-31.
They do not define support for other systems.

## Tabi reference

Tabi 0.1.88 is installed on the reference Mac.
The review on 2026-10-31 covered the interface, the application bundle, the settings file, and the log.
The review did not read the dictation history.
A second review on 2026-11-05 added the website, the settings and state files, and 45 dictations in the log.
Bloop uses Tabi as a behavior reference only. Bloop does not copy Tabi code, prompts, or assets.

### Observed behavior

| Area | Tabi | Bloop decision |
| --- | --- | --- |
| Technology | Tauri 2 (Rust) with a web interface and `objc2` | Rust with native AppKit only |
| Dictation key | fn by default, or a second key: right Option or right Command. Minimum hold of 220 ms. On the reference Mac, the second key is right Option. | Right Command; same 220 ms threshold. Phase 5 starts the capture at key press. |
| Cancel | Escape; nothing is typed; the user can undo the cancel for 5 seconds | Escape; undo of a cancel in Phase 8 |
| Hands-free mode | Shift or Space during the hold, or a double press, locks the recording. One press stops it. | Not planned |
| Status indicator | Web view "orb" at the bottom of the screen: dark pill, target application icon, seven orange bars that follow the current voice level; closes at key release; no indicator during processing | Native panel with the same layout and behavior; the icon goes directly into a layer |
| Transcription | Two paths: a WebSocket relay during capture, and an upload of the complete recording after key release. In 45 of 49 dictations, the words came from the upload. | One streaming connection to the selected speech provider during capture; no upload |
| Correction | `mistral-medium-2604`, an alias of Mistral Medium 3.5 in the models API; five styles, each with a list of applications; long dictations get a correction in parts during the capture | Optional; `mistral-medium-latest` or the model that the menu selects; one prompt that the menu selects; no style for each application |
| Dictionary | Pairs "tabi writes, should be", applied when the sentence fits. After a paste, Tabi watches the field and proposes the corrections of the user. | One dictionary file (Phase 3). Learning from fixes in Phase 6, with a question before each new entry. |
| Insertion | Synthetic Command-V; Tabi detects when the target reads the clipboard | Same method |
| Clipboard | Items marked as concealed and transient | Same markers; restore the previous content |
| No text field | The text shows in a small window | The orb shows the text in a card and copies it |
| Text at the cursor | Adapts the first letter, the spaces, and the final period. The log confirms the three operations. | Phase 8 |
| Other functions | Names read from the front window, learned corrections, dictionary, audio ducking on the built-in output, sounds, noise suppression, microphone selector, history, reading, OCR, translation, and questions to an assistant | Some features in Phase 3 to Phase 8; see the [roadmap](ROADMAP.md) |

Tabi and Bloop use different keys. Both applications can operate at the same time.

### Measured latency

On 2026-10-31, the Tabi log contained timing data for 23 dictations on the reference Mac.

| Step | Median | Range |
| --- | --- | --- |
| Transcription after key release | 512 ms | 337–1345 ms |
| Correction | 595 ms | 326–1107 ms |
| Paste command | 115 ms | 104–219 ms |
| Key release to paste command | 1297 ms | 885–2419 ms |
| Paste command to clipboard read | 101 ms | 101–128 ms |

The second review on 2026-11-05 read 45 dictations in the Tabi log:

| Step | Median | Range |
| --- | --- | --- |
| Transcription after key release | 506 ms | 271–1727 ms |
| Correction | 574 ms | 326–1334 ms |
| Paste command | 116 ms | 104–219 ms |
| Key release to paste command | 1281 ms | 781–2967 ms |
| Paste command to clipboard read | 101 ms | 101–128 ms |

In 45 of 49 dictations, the words came from the upload after key release, not from the WebSocket relay.
Thus the Tabi transcription time is the time of an upload, not of a stream.
Phase 2 measured the transcription completion time after key release. Phase 3 measures the complete path with correction.

## Current decisions

| Area | Decision |
| --- | --- |
| Dictation key | Hold Right Command for 220 ms. The capture starts at this threshold. A combination or a short press does nothing. Phase 5 starts the capture at key press. |
| Cancel | Escape during a session. Bloop blocks this Escape event. |
| Orb | Native panel at the bottom center of the screen. It never takes keyboard focus. It closes when capture ends. |
| Audio | System default microphone, or the built-in microphone when the default input is a Bluetooth device; bounded queues in memory; no maximum capture duration |
| Network | One transcription stream during capture; one correction request after the complete transcript, when the correction is on |
| Correction | Off by default; one prompt file that the menu selects; raw transcript if the correction fails |
| Dictionary | One file. The written forms go to the transcription. The pairs have no effect for now (parked on 2026-11-06). Bloop asks before each new entry (Phase 6). |
| Target check | Same frontmost application and no new click or key press since capture ended |
| Insertion | Command-V with a clipboard item that the target reads on demand |
| Clipboard | Restore the previous content after the target reads the text |
| Fallback | Show the text in the orb card and keep it in the clipboard |
| Recovery | Menu item "Copy last result" until a newer result replaces it |
| Interface | Menu bar item only; no Dock icon; no settings window; correction toggle in Phase 3, more choices in Phase 4 |
| Settings | One TOML file that the user and Bloop edit (Phase 4); prompt files that the user can edit |
| Credentials | The API key of each provider in macOS Keychain; the account is the provider name |
| Latency target | Median of 1.5 seconds or less from key release to paste command, with the correction on |

The target check does not need the Accessibility tree of the target.
Thus browsers, Electron applications, and terminals use the same path as TextEdit.

## Correction prompts

The correction sends one prompt and the complete transcript to the selected chat model. The prompts do not depend on the provider.
Phase 3 starts with `natural-light`. `natural-full` remains a candidate after the first daily-use trial:

- `natural-light` corrects punctuation and capital letters, with few commas and no colons. It removes hesitations and stutters, and applies spoken corrections.
- `natural-full` also corrects spelling, grammar, and unwanted repetition.

Both prompt designs keep names, numbers, negations, questions, and the speaker's degree of certainty.
They do not answer a question in the dictated text.
They treat instructions inside the transcript as text to correct.
They return plain text without Markdown formatting.
For a spoken correction, they keep the last version: "à 2 heures, non, 3 heures" becomes "à 3 heures".

Phase 4 lets the user edit and add prompt files. A menu choice is added when alternative prompts are useful.
A prompt can ask for line breaks and lists. The default prompts do not ask for them.

A text step in the code applies the French typography after the correction:

- It replaces a non-breaking space before `?`, `!`, `:`, `;`, and `»`, and after `«`, with a normal space.
- It replaces the typographic apostrophe with the straight apostrophe.
- It does not change line breaks.

A model can mix normal and non-breaking spaces, and the user cannot see the difference. Thus the code applies this rule, not the prompt.
The normal space is a deliberate difference from the OQLF and Lexique rules. Terminals and some editors do not handle non-breaking spaces well.
The owner selected this rule on 2026-11-05.
Bloop removes line breaks at the start and at the end of the final text.
ASD-STE100 applies to repository documents. It does not change the French text.

## Later work

The [roadmap](ROADMAP.md#priorities) gives the priorities.
These items have no phase yet.

- Text formatting: paragraphs, and lists with `- `. The model applies it from the structure of the speech or from spoken commands.
  A toggle in the "Prompt" submenu turns it on. The owner starts this work when needed.
- Automatic start at login.
- Other dictation keys.
- Support for keyboard layouts that move the V key.
- A settings window.

## Not planned

- File transcription, upload fallback, and simultaneous transcription through multiple providers.
- Styles for each application. The owner decided this on 2026-11-05.
- Hands-free mode and the live text in the orb. The owner decided this on 2026-11-05.
- Speech output, OCR, translation, and voice commands that control other applications.
- An assistant, agents, or automatic message submission.
- Permanent audio storage or a permanent transcript history.
- User accounts, a custom remote server, a relay, a database, or a web interface.
- Insertion into password fields.
- Public distribution and support for other macOS versions.

## Open decisions

- Design the capture start at key press after the microphone checks of Phase 5.
- Select the short-text skip, length ratio rules, and correction timeout from the Phase 3 evaluation.
