# Scope

Status: Design draft, revision 2 (2026-10-31).

## Goal

Bloop does one task: dictation into the focused text field.
The behavior follows the dictation feature of Tabi.
Simple behavior and low latency have priority over additional features.

## Confirmed choices

- The project name is Bloop.
- The first version is for personal use on the owner's Mac.
- All application code uses Rust.
- The interface uses native AppKit through `objc2`. Bloop has no web view.
- The speech input and text output use French.
- Mistral Voxtral converts speech to text.
- Mistral Small corrects the text.
- The user holds Right Command to speak.
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
Bloop uses Tabi as a behavior reference only. Bloop does not copy Tabi code, prompts, or assets.

### Observed behavior

| Area | Tabi | Bloop decision |
| --- | --- | --- |
| Technology | Tauri 2 (Rust) with a web interface and `objc2` | Rust with native AppKit only |
| Dictation key | Right Option; minimum hold of 220 ms | Right Command; same 220 ms threshold |
| Cancel | Escape | Escape |
| Hands-free mode | Shift during the hold locks the recording | Later work |
| Status indicator | Web view "orb" at the bottom of the screen: dark pill, target application icon, seven orange bars that follow the current voice level; closes at key release; no indicator during processing | Native panel with the same layout and behavior; the icon goes directly into a layer |
| Transcription | `voxtral-small-transcribe-3-0` upload and a parallel real-time stream | Upload only; real-time stream if the latency is too high |
| Correction | `mistral-medium-2604`; one style for each application | `mistral-small-2603`; one text profile |
| Insertion | Synthetic Command-V; Tabi detects when the target reads the clipboard | Same method |
| Clipboard | Items marked as concealed and transient | Same markers; restore the previous content |
| No text field | The text shows in a small window | The orb shows the text in a card and copies it |
| Text at the cursor | Adapts the first letter, the spaces, and the final period | Later work |
| Other functions | Styles, screen names, learned corrections, audio ducking, reading, OCR, translation | Outside version 1 |

Tabi and Bloop use different keys. Both applications can operate at the same time.

### Measured latency

The Tabi log contains timing data for 23 dictations on the reference Mac.

| Step | Median | Range |
| --- | --- | --- |
| Transcription after key release | 512 ms | 337–1345 ms |
| Correction | 595 ms | 326–1107 ms |
| Paste command | 115 ms | 104–219 ms |
| Key release to paste command | 1297 ms | 885–2419 ms |
| Paste command to clipboard read | 101 ms | 101–128 ms |

Tabi gets its transcription speed from a real-time stream through its own relay.
Bloop starts with one upload request. Phase 2 measures the difference.

## Minimum version

| Area | Decision |
| --- | --- |
| Dictation key | Hold Right Command for 220 ms. A combination or a short press does nothing. |
| Cancel | Escape during a session. Bloop blocks this Escape event. |
| Orb | Native panel at the bottom center of the screen. It never takes keyboard focus. It closes at key release. |
| Audio | System default microphone; capture in memory; limit of 120 seconds |
| Network | One upload request after key release, then one correction request |
| Text profile | Natural Clean; one prompt; raw transcript if the correction fails |
| Target check | Same frontmost application and no new click or key press since key release |
| Insertion | Command-V with a clipboard item that the target reads on demand |
| Clipboard | Restore the previous content after the target reads the text |
| Fallback | Show the text in the orb card and keep it in the clipboard |
| Recovery | Menu item "Copy last result" until a newer result replaces it |
| Interface | Menu bar item only; no Dock icon; no settings window |
| Settings | One TOML file |
| Credentials | Mistral API key in macOS Keychain |
| Latency target | Median of 1.5 seconds or less from key release to paste |

The target check does not need the Accessibility tree of the target.
Thus browsers, Electron applications, and terminals use the same path as TextEdit.

## Natural Clean

Natural Clean corrects spelling, grammar, punctuation, and unwanted repetition.
It keeps names, numbers, negations, questions, and the speaker's degree of certainty.
It does not answer a question in the dictated text.
It treats instructions inside the transcript as text to correct.
It returns plain text without Markdown formatting.
It uses a normal space before `?`, `!`, `:`, and `;`. It uses the straight apostrophe.
Bloop removes line breaks at the start and at the end of the final text.
ASD-STE100 applies to repository documents. It does not change the French text profile.

## Later work

- A real-time transcription stream (`voxtral-mini-transcribe-realtime-3-0`) if the latency target fails.
- Text adaptation at the cursor: first letter, spaces, and final period.
- Hands-free mode with Shift during the hold.
- Start and stop sounds.
- Automatic start at login.
- A microphone selector and other dictation keys.
- Support for keyboard layouts that move the V key.
- A settings window.

## Outside version 1

- Styles for each application, names read from the screen, and learned corrections.
- Audio ducking, speech output, OCR, translation, and voice commands.
- An assistant, agents, or automatic message submission.
- Permanent audio storage or a permanent transcript history.
- User accounts, a custom remote server, a relay, a database, or a web interface.
- Insertion into password fields.
- Public distribution and support for other macOS versions.

## Open decisions

- Confirm the French typography rules of Natural Clean.
- Confirm the capture start: at the hold threshold or at key press. Phase 2 checks for a lost first word.
