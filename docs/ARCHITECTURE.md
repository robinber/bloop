# Architecture

Bloop is a personal macOS dictation app. It uses Rust, native AppKit, and direct connections to Mistral.
The interface consists of a menu bar item and a recording indicator. There is no web view or application server.

The [source map](STRUCTURE.md) describes the crate boundaries. The [contracts](CONTRACTS.md) define the invariants.

## Dictation flow

```text
Right Command down -> settings snapshot -> hold timer
Hold threshold     -> microphone -> bounded queue -> converter -> provider stream
Right Command up   -> target snapshot -> stop microphone -> drain and finish stream
Complete transcript -> optional correction -> ordered insertion -> clipboard recovery
```

The hold threshold defaults to 220 ms. Recording starts after this threshold, so the user must wait for the indicator before speaking.
Right Command combinations remain available. Escape or other input cancels the active capture.
System sleep cancels the capture and all jobs. Recording has no maximum duration.

The core has one capture state: `Idle`, `Pending`, or `Listening`.
After release, a job moves through `Processing`, `Ready`, and `Inserting`.
Up to three jobs can exist. A new capture can start while earlier jobs run, and insertion follows capture order.
Each generation identifies one dictation. Results from cancelled generations have no effect.

## Threads and ownership

| Context | Work |
| --- | --- |
| Main thread | AppKit, event tap, core, driver, settings, menu, indicator, and clipboard |
| Audio callback | Mono mixing, sample queue writes, and atomic level and failure updates |
| Tokio worker | Incremental conversion, network I/O, and correction |

The audio callback uses preallocated storage. It does not allocate, lock, log, or wait for network I/O.
One Tokio worker thread runs the asynchronous tasks. The worker waits 20 ms between queue-drain and send cycles.
During provider operations, it checks queue failures at the same interval. Network sends can extend a cycle.
The driver owns work handles by generation. Cancellation aborts the work and releases its queue and connection.
A supervising task converts a work-task panic to `AppError::Internal`.
Events return through the main dispatch queue.

## Audio and transcription

Bloop uses the default input. If it is a Bluetooth device, Bloop selects the built-in microphone when one exists.
The microphone runs at its native rate. The worker converts mono samples to signed 16-bit little-endian PCM at 16 kHz.
The converter uses filtered resampling and removes its own delay.

The sample queue holds 960,000 samples: 20 seconds at 48 kHz or 10 seconds at 96 kHz.
Its capacity does not grow with recording duration. An overflow fails the dictation instead of dropping samples silently.
System audio overruns are counted and logged at capture end. They do not stop capture.
The worker sends complete 100 ms chunks during capture. At release, it sends the remaining samples without waiting for a full chunk.

One WebSocket connection serves each dictation. The adapter applies the language, dictionary, and delay before sending audio.
Partial transcripts stay inside the adapter. Only a complete transcript reaches the pipeline.
Bloop does not reconnect, replay audio, or upload a recording after a streaming failure.
It stores no audio file or complete local recording.

## Correction

Correction is off by default. When enabled, it applies three steps:

1. `LlmRefine` sends the complete transcript with the selected prompt to the chat model.
2. `Typography` normalizes French non-breaking spaces around punctuation and converts `’` to `'`.
3. `Trim` removes leading and trailing line breaks. It preserves spaces.

A failed or skipped step preserves its input for the next step.
Thus a failed model request retains the transcript, while typography and trim still apply.
An incomplete or empty model reply is a failure. Bloop cannot detect every change in meaning from a model.

The built-in prompts are `natural-light` and `code`. A user file can override either prompt or add a named prompt.
The HTTP client and its connection pool are reused while the chat settings stay unchanged.
The [setup guide](SETUP.md#settings-file) lists the configured model defaults. A model alias can resolve to a different provider version over time.

## Time and size limits

These are implementation limits, not measured latency guarantees.

| Operation | Limit |
| --- | --- |
| Transcription connection | 5 s |
| Each audio send | 5 s |
| Transcription finish, after the queue drains | 15 s |
| Correction request, including its response body | 8 s |
| Successful HTTP response body | 1 MiB |
| WebSocket message | 1 MiB |
| Clipboard snapshot | 64 items and 64 MiB |

There is no total deadline for the audio backlog after release. Each send has its own deadline before `finish` starts.
A slow connection can therefore delay completion and later insertions beyond 15 seconds.
No timeout limits the recording duration.

### Measured latency

Bloop measured these values from its own log on the reference Mac, from 2026-11-05 to 2026-11-08.
The sample is 109 dictations that reached the paste command: 98 with the correction on and 11 with it off.

| Step | Correction on | Correction off |
| --- | --- | --- |
| Key release to complete transcript | median 902 ms | median 836 ms |
| Correction | median 639 ms | not applicable |
| Key release to paste command | median 1572 ms, from 840 to 11738 ms | median 988 ms, from 572 to 3105 ms |
| Paste command to target read | median 6 ms | median 7 ms |
| Target read to `InsertDone` | median 314 ms | median 315 ms |

The daily medians with the correction on were 1536, 1758, 1457, and 1267 ms.
The design target was a median of 1.5 s from key release to paste command with the correction on. The period missed it by 72 ms.
The provider `finish` step alone has a median of 864 ms over 199 captures, pasted or not. Transcription completion is the largest step.

## Insertion and recovery

At release, Bloop records the frontmost process and the keyboard and mouse input counters.
Before pasting, it saves the clipboard, checks that the target and counters are unchanged, and checks secure input.
It posts at most one Command-V event pair for a generation.

The temporary clipboard item supplies its text on demand. A clipboard read produces `Landed`.
This result confirms a read, not a verified change in the focused field.
The previous clipboard is restored 300 ms after the read, if Bloop still owns the temporary item.
If no read occurs within one second, Bloop reports a fallback and keeps text available for manual recovery.

The card shows an excerpt. **Copy last result** copies the latest result retained for insertion.
The card does not guarantee that the clipboard still contains that text.
A newer result can replace the result retained by the menu.

The target check does not inspect the Accessibility tree. A clipboard manager can trigger a read before the target does.
Clipboard restoration can be incomplete if a type cannot be read or the snapshot exceeds its limits.
Reading promised clipboard data can block the main thread while another application supplies it.
The paste uses key code 9 for V. Keyboard layouts with a different V position are not supported.
Terminals without bracketed paste can execute pasted line breaks.

## Settings and data

The hub reads bounded settings and dictionary files at each dictation key press and menu open.
It reads the selected prompt when correction is enabled. An unchanged set of files keeps the existing settings snapshot.
Each generation retains its snapshot, so edits affect subsequent dictations.
Menu changes preserve TOML comments and replace the settings file through a temporary file in the same folder.

API keys are read from Keychain at startup. A key change requires a restart.
Audio and dictionary terms go to the speech provider. Correction also sends the transcript and prompt to the chat provider.
Logs contain operational metadata, the identity of the frontmost application at key release, and controlled error categories, without audio, transcript text, or credentials.
The `eval` tool deliberately writes its test samples and results to the terminal.

See [Setup](SETUP.md) for file paths, settings validation, permissions, and troubleshooting.
