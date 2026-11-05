# Architecture

Status: Proposed design, revision 11 (2026-11-05). Phase 1 and Phase 2 are complete. The [roadmap](ROADMAP.md) gives the priorities.

## Principles

- Bloop does one task: dictation into the focused field.
- The behavior follows Tabi. The design stays small.
- All code uses Rust. The interface uses AppKit through `objc2`. Bloop has no web view.
- Bloop never loses the text. If a paste is not possible, Bloop shows the text and copies it.
- Bloop is provider-agnostic. Ports in `bloop-core` hide the providers. Mistral is the default provider and the only adapter today.
- A feature that is off does not change the dictation path. Phase 3 adds a correction toggle; Phase 4 adds persistent settings.

## Process

Phase 1 proves the key, orb, and insertion with fixed text.
Phase 2 adds streaming transcription. Phase 3 adds optional correction, one menu toggle, and a daily-use trial.
Phase 4 adds persistent settings and more menu choices when needed.
Streaming is the only STT path.

```text
Right Command held for 220 ms
    -> the orb opens, capture starts, and the provider connection opens
    -> audio chunks go to the speech provider while the user speaks
Right Command released
    -> the capture stops and the orb closes
    -> queued audio drains, the converter flushes, and the stream ends
    -> the speech provider returns the complete transcript
    -> when the correction is on: correction, typography, trim
    -> clipboard save, target check, then paste or fallback card
    -> the orb closes
```

## Code structure

Bloop is one Cargo workspace with five crates. The [code structure](STRUCTURE.md) gives the details.

| Crate | Responsibility |
| --- | --- |
| `bloop-core` | Domain types, session state machine, pipeline, text steps, and ports |
| `bloop-providers` | AI providers; one module for each vendor |
| `bloop-audio` | Microphone capture and incremental PCM conversion |
| `bloop-macos` | AppKit, event tap, orb, paste, menu, and Keychain |
| `bloop-app` | Executable: settings, construction, and driver |

The [Rust contracts](CONTRACTS.md) define the types and the ports.
The core receives events and returns commands. The driver executes the commands through the ports.
Unit tests drive the core with event sequences, without macOS.

## Threads

| Thread | Work |
| --- | --- |
| Main | AppKit run loop, event tap, core, orb, menu, clipboard, and paste command |
| Audio | `cpal` callback: copies samples to a bounded buffer and updates the level |
| Worker | Incremental audio conversion, transcription stream, and correction request; asynchronous I/O through Tokio |

- The audio callback does not allocate memory, lock, access files, or wait for the network.
- The audio callback sends no event and writes no log. It updates only atomic values: sample count, peak level, and overflow mark.
- The worker tasks run on a Tokio runtime with one worker thread, `bloop-worker`.
- After each audio queue read and its sends, the worker waits 20 ms. The callback does not wake it.
- The audio level is an atomic value. The orb reads it on the main thread.
- The worker sends each result to the main queue with its generation.
- The core ignores a result for an unknown generation, for example a cancelled job.
- The event tap callback returns quickly. It only sends events to the core.

## Session states

A session has two parts: the capture, then a job.
The capture uses the microphone and the orb. The job runs without a visible indicator.
Transcription starts during `Listening`. A job finishes transcription, applies text steps, and attempts insertion.
The driver creates no insertion result from a partial transcript.

```text
Capture: Idle -> Pending -> Listening -> Idle
Job:     Processing -> Ready -> Inserting -> end
```

| Capture state | Starts when | Ends when |
| --- | --- | --- |
| `Pending` | The user presses Right Command alone. | 220 ms pass: `Listening`. Key release or other input: `Idle`, with no signal. |
| `Listening` | The hold threshold passes. | Key release: `Idle` and a new job. Other input or Escape: `Idle`, with no job. |

| Job status | Starts when | Ends when |
| --- | --- | --- |
| `Processing` | The capture stops. | Text: `Ready`. Nothing heard: end, with no paste. Error: end, with the error view if no capture is active. |
| `Ready` | The final text is available. | All earlier jobs are complete: `Inserting`. |
| `Inserting` | The paste module receives the text. | Paste or fallback card: end. |

- Bloop permits one capture at a time.
- A new capture can start while jobs run. Thus the user can dictate again before the previous text lands.
- Jobs paste their text in session order.
- No more than 3 jobs exist at the same time: processing, ready, or pasting. If 3 jobs exist when the hold threshold
  passes, the capture does not start and the orb shows the error view.
- While a capture is active, the orb shows no error view for an earlier job.
- A fallback card for an earlier job waits until the capture ends. Then the orb shows it.
  A capture error or system sleep deletes the card. The text stays in the clipboard and in "Copy last result".
- Escape cancels the capture. It does not cancel a job.
- Microphone loss cancels the capture. System sleep cancels the capture and all jobs.
- Cancellation discards queued audio and stops the associated network task. Capture cancellation also closes the orb.
- A stream failure during capture stops that capture. After capture, it ends the associated job.
- Bloop does not reconnect, replay audio, or use file transcription after a stream failure.

## Dictation key

- The event tap runs on the main run loop. It is an active tap, thus it needs Accessibility.
- Right Command has key code 54. The tap reads it from `flagsChanged` events.
- A key press or a mouse click during the hold is a combination. The session stops with no signal.
  Thus Right Command shortcuts and Command-click continue to operate.
- During `Pending` and `Listening`, the tap blocks Escape. It also blocks the key repeats and the key up of that
  Escape press, because they can arrive after the capture ends. At other times, Escape goes to the application as usual.
- macOS disables a tap after a slow callback. The callback enables the tap again when it receives that event.
  Bloop installs the tap after the slow setup at start, because the run loop does not run before this setup ends.
  The tap can miss the key release while it is disabled. Thus the interruption ends the hold as a combination.
- Bloop marks the events that it posts. The tap ignores these events.
- Secure input stops key events to the tap. In this condition, the dictation key has no effect. This is a known limit.
- Measured on the reference Mac (macOS 27, 2026-11-01): macOS refuses the tap when Accessibility is missing,
  also with Input Monitoring. The capture starts 228–231 ms after the key press. The target is 220 ms.
- The capture starts at the hold threshold, not at key press. Thus Bloop can lose a word that the user says at key press.
  On 2026-11-05, the owner accepted this limit. The [roadmap](ROADMAP.md) gives the measured microphone start.
  Later on 2026-11-05, the owner reopened this decision. Phase 5 starts the capture at key press.

## Orb

- The orb is an `NSPanel` with the borderless and nonactivating styles.
  It never becomes the key window or the main window.
- The panel ignores mouse events and has a clear background. From Phase 6, it accepts mouse events only while it shows the suggestion card.
- The panel uses the status window level. It shows on all Spaces and above full-screen applications.
- The panel opens at the bottom center of the screen that contains the mouse pointer, inside the visible frame.
- Core Animation layers draw the orb. No timer runs while the orb is hidden.

### Layout

The orb is a small dark pill.
On the left, it shows the icon of the application that receives the text.
On the right, seven vertical bars move with the voice level.

| Layer | Content |
| --- | --- |
| Pill | `CALayer` with a dark, almost opaque fill and a corner radius of half its height |
| Application icon | `CALayer` that shows the `NSImage` from `NSRunningApplication.icon` |
| Bars | Seven `CALayer` bars with round ends, used as the mask of one `CAGradientLayer` (deep purple at the bottom, burgundy at the top) |

The initial dimensions come from a Tabi screen capture on the reference Mac. Adjust them by eye in Phase 1.

| Element | Initial size in points |
| --- | --- |
| Pill | 150 × 56 |
| Application icon | 20 × 20 |
| Bar | Width 4; 11.5 between bar centers; height from 6 to 24 |
| Card | 340 × 104, corner radius 16; text 14 pt on up to 3 lines; instruction 12 pt |

The panel has the size of the card. The pill sits at its bottom center.
The panel ignores mouse events. Thus its transparent area does not block clicks.

- The icon shows the frontmost application when the orb opens.
- If the user activates another application during the session, the icon changes.
- For a browser, the orb shows the browser icon. Website icons are later work.
- The layer uses the application icon directly. Bloop does not convert or cache icons.

### Bar animation

- During `Listening`, a 30 Hz timer reads the audio level on the main thread.
- Bloop converts the level to decibels.
- The bars show a fixed range of decibels below a ceiling. The ceiling follows the recent level.
  Thus the bars move the same with a loud microphone and with a quiet microphone.
- The ceiling rises immediately to a louder level. It falls slowly at a fixed rate, between two limits.
- Below a fixed noise floor, the bars show no level. Thus room noise does not move the bars.
- Bloop smooths the result: fast rise, slow fall.
- All bars follow the current level at the same time.
- Each bar has a fixed weight and a small random variation. Thus the bars have different heights.
- Each height change uses a short implicit animation of about 80 ms.
- A bar never goes below its minimum height. Silence shows seven short bars.

### Orb views

The orb is visible during `Listening`, and when the user must see a result or an error.
During processing, the orb is not visible. Tabi has the same behavior.

| Orb view | Behavior |
| --- | --- |
| Opening | The pill grows from 80% to 100% and fades in, in about 150 ms |
| Listening | The bars follow the voice level |
| Closing | When the capture ends, the bars go down, then the pill shrinks and fades out, in about 200 ms |
| Error | The pill opens with red bars for about 1 second, then closes |
| Fallback card | The pill becomes a card with the final text and "Copied, press ⌘V", in about 200 ms. The card shows the first 240 characters. It closes after 8 seconds or at the next capture. |
| Suggestion card | The pill becomes a card with the suggested dictionary entry and the "Save" and "Ignore" buttons (Phase 6). It closes after 8 seconds. |

Bloop does not use Tabi code or assets.

## Audio

- The capture uses the system default input device in its native format.
- Bloop sets no maximum capture duration. Capture continues until key release, cancellation, or failure.
- Before capture, Bloop allocates bounded queues. Their capacity does not grow with the capture duration.
- Capture starts while the provider connection opens. A queue keeps the initial samples until the connection is ready.
- The callback mixes the channels to mono and calculates the level of each buffer.
- During capture, the worker converts samples incrementally to mono signed 16-bit little-endian PCM at 16 kHz, for all microphones.
- The worker resamples with the FFT resampler of `rubato`. It filters the audio before it lowers the rate. Thus it prevents aliasing.
  The audio callback does not convert.
- The worker sends each chunk in order. A full queue fails the session; Bloop never silently drops samples.
- During capture, the worker sends only complete chunks of 100 ms. It keeps the remaining samples for the next queue read.
  At capture end, the worker sends the remaining samples immediately. Thus the chunk length adds no delay after key release.
- At capture end, the worker drains accepted samples and flushes the converter before it ends the stream.
- Bloop sends each capture to the provider, also a short or silent capture. It applies no duration or level threshold.
  For silence, Voxtral returns an empty transcript. Then the pipeline sends `NothingHeard`, and Bloop pastes nothing.
  A level threshold can remove a quiet dictation: the peak of quiet speech can be near the peak of silence.
- Bloop releases each audio chunk after sending it. Cancellation discards remaining chunks.
- Bloop keeps no complete recording and writes no audio to disk.

Long dictation checks measure memory use and completion time. They do not set a maximum capture duration.

## Mistral connections

This section describes the Mistral adapters, the default providers. Another provider implements the same ports with its own protocol.

A WebSocket connection serves each transcription session. A reused HTTP client (`reqwest`) serves correction requests.
The API key comes from Keychain (service `bloop`, account `mistral`).
The user stores the key with the `security add-generic-password` command.

| Request | Model | Settings | Time limit |
| --- | --- | --- | --- |
| Transcription | `voxtral-mini-transcribe-realtime-3-0` | PCM audio; language `fr`; `context_bias` word list; `target_streaming_delay_ms`; all set before the first chunk | Connection 5 s, inactivity 5 s, completion 15 s: initial values, not measured |
| Correction | `mistral-small-2603` initially; other choices later | `reasoning_effort="none"`; `temperature` 0; `max_tokens` from the input length; `natural-light` initially | Select in Phase 3; 8 s was the initial proposal |

Phase 3 selects the correction timeout from the delay acceptable in daily use, including long dictations.
An earlier job that waits for correction also delays later insertions. On timeout, use the raw transcript.
The worker applies the transcription limits. Each chat adapter applies the correction limit that `bloop-app` gives at construction.

The correction HTTP client:

- One `reqwest` client serves all correction requests. Bloop builds it at start.
- It uses `native-tls`, HTTP/2, and keep-alive. Since `reqwest` 0.13, the default TLS is `rustls`. Thus Bloop turns off the default features.
- Connection warm-up with `GET /v1/models` is an optional experiment after the first trial.
  It is not part of the initial path and does not guarantee that a connection stays open until correction.
- Measured on 2026-11-05 without an API key: `api.mistral.ai` negotiated HTTP/2 and TLS 1.3.
  The TCP connection took 10–16 ms, and the TLS handshake 26–30 ms.

The correction request:

- The system message contains the prompt. The user message contains the transcript between delimiters.
- The initial request omits `prediction`. An optional comparison then uses the transcript as predicted output.
  Mistral documents a response-time benefit when the output is near the prediction. Bloop must measure its own gain.
- `max_tokens` comes from the input length: ceil(characters / 2.5 × 1.6) + 32, between 64 and 2,048.
  This is an initial value. Phase 3 checks it against the token counts of the replies.
- Prompt caching is optional after the first trial. The prompt hash can serve as `prompt_cache_key`; it does not guarantee a cache hit.
- A finish reason other than `stop` is an incomplete reply. Bloop then uses the raw transcript.
The connection limit covers setup. The inactivity limit detects a stalled transport, not speech silence.
The completion limit applies after capture ends.
The worker counts it from the `finish` call, after the backlog drained, because provider messages stay inside the adapter.
Thus a backlog after a slow connection can still drain. No short request timeout covers the full capture duration.
The queue capacity is 960,000 samples: 20 s at 48 kHz, and 10 s at 96 kHz. At these rates, the connection limit is shorter than this capacity.
Thus a slow connection fails with a timeout before the queue is full.
During capture, each audio send has the inactivity limit.

The realtime model is a Public Preview since 2026-10-20. Bloop uses its fixed identifier, not the alias.
The Mistral lifecycle policy permits changes to a Public Preview model, also with a fixed identifier.
`target_streaming_delay_ms` (80 to 1200, or 2400) sets the balance between delay and accuracy.
The provider never corrects the text that it sent. Thus this value also changes the final text.
On 2026-11-05, the owner kept 480 ms.

The transcription session follows the provider protocol, checked in Mistral documentation on 2026-11-03:

1. The adapter opens the WebSocket with the model in the query string and the API key in the `Authorization` header.
2. The provider sends `session.created`. The adapter sends one `session.update` with the audio format, language, word list, and delay.
3. The provider answers `session.updated`. The settings are fixed from the first audio chunk.
4. The adapter sends each chunk in an `input_audio.append` message, as base64 PCM.
5. At capture end, the adapter sends `input_audio.end`. The `text` of `transcription.done` is the complete transcript.
6. The provider has no abort message. To cancel, the adapter closes the connection.

The official SDKs also have an `input_audio.flush` message. The documentation does not describe it.
`context_bias` accepts words, phrases, or descriptions. Its limit is the context window of 65,536 tokens, which the audio also uses.
Above this limit, the provider returns error 400.

The adapter does not use the text of the `transcription.text.delta` messages. It logs only their number and length, and the type of each event that it does not use.
The default sample rate of the API is 16 kHz. The official example uses 24 kHz. The documentation does not say if the API accepts 48 kHz.
The realtime microphone example of the official Python SDK accepts only 8000, 16000, 22050, 44100, and 48000 Hz. Its default is 16 kHz.
Source: `mistralai/client-python`, `examples/mistral/audio/async_realtime_transcription_microphone.py`, checked on 2026-11-05.

Measured on the reference Mac on 2026-11-04 and 2026-11-05: Voxtral accepted `sample_rate` 24000 and echoed it in `session.updated`.
For AirPods audio at 24 kHz, it sent no delta and returned an empty transcript without an error.
For the built-in microphone at 48 kHz, it transcribed the speech. The [roadmap](ROADMAP.md) records the values.

On 2026-11-05, the owner decided to convert each capture to 16 kHz mono before the provider, for all microphones.
The documentation does not give the maximum session length, the inactivity limit of the provider, or the result of a silent session.
For a silent session, Voxtral sent no delta and returned an empty transcript without an error.
The maximum session length and the inactivity limit stay unknown.

On connection loss, a full queue, or a timeout, Bloop stops the affected session and reports the failure.
It does not insert a partial transcript or retry the stream automatically. The user can dictate again.
There is no upload fallback.

Raw fallback rules apply only after a complete transcript is available:

- The skip below 4 words is a candidate rule. Phase 3 checks short spoken corrections before enabling it.
- If the correction fails, times out, returns an incomplete reply, or returns empty text, Bloop uses the raw transcript.
- The 50% to 160% character ratio is a candidate rule, not a required limit or a meaning check.
  The example "à 2 heures, non, 3 heures" to "à 3 heures" has a 40% ratio.
  Phase 3 selects or removes these heuristics before daily use. They must not reject expected corrections in the test set.
- The worker logs the reason for each fallback from the step report.

Latency targets, as medians for 10 seconds of speech:

| Step | Target | Tabi median |
| --- | --- | --- |
| Transcription after key release | 650 ms | 512 ms |
| Correction | 650 ms | 595 ms |
| Paste command | 150 ms | 115 ms |
| Key release to paste command | 1500 ms | 1297 ms |

Phase 2 measured transcription completion after key release. Phase 3 measures the complete path with correction.
Record the raw and corrected medians, the longest observed delays, and fallback frequency separately.

The paste command is the time when Bloop posts Command-V. The target read is the time when the target reads the clipboard item.
The `InsertDone` event comes about 300 ms after the target read, after the clipboard restore. Thus `InsertDone` does not measure the paste.
Phase 3 logs the paste command and the target read with the generation.
These values are targets, not Bloop measurements. Streaming still needs completion, correction, and paste time after release.
The realtime model and parameters were checked in Mistral documentation on 2026-11-03.
The correction model and parameters were checked on 2026-10-31 and on 2026-11-05.
Live transcription requests ran on 2026-11-04 and 2026-11-05. The [roadmap](ROADMAP.md) records their measured values.

References: [Mistral realtime transcription](https://docs.mistral.ai/studio/audio/speech-to-text/transcribe-in-realtime),
[Mistral Small 4](https://docs.mistral.ai/models/mistral-small-4-0-26-03),
and [reasoning settings](https://docs.mistral.ai/studio/conversations/reasoning).

## Stream ownership

The driver owns one transcription task and its audio queue for each generation.
The provider implements the streaming contract in [Rust contracts](CONTRACTS.md#ai-ports).

- `StartCapture` starts audio capture and opens the provider session asynchronously.
- `StopCaptureAndProcess` stops capture and tells the existing task to finish. It does not start another transcription.
- The worker sends `TextReady` only after the complete transcript passes through the text steps.
  The job exists at that time: the core creates it before the driver sets the end mark.
- `CancelCapture` stops capture and its stream. It carries no generation: the driver keeps the generation of the active capture.
- `CancelWork` stops the task for the specified generation and forgets its paste target.
- A failure during capture produces `CaptureFailed`. A failure after capture produces `WorkFailed`.
- Bloop records the target at key release. Insertion still follows session order.
- Phase 2 tests release, failure, and cancellation races with a fake provider before the real adapter.

## Paste

At key release, the paste module records the target:

- the process identifier of the frontmost application;
- the counters of key-down and mouse-down events (`CGEventSourceCounterForEventType`).

The target check:

1. The frontmost application has the same process identifier.
2. The event counters did not change.
3. Secure input is not active (`IsSecureEventInputEnabled`).

The paste:

1. Bloop saves all items of the current clipboard, with the data of all their types.
   If macOS cannot list the items, Bloop does not restore the clipboard, and the text stays in it.
   If some types cannot be read, Bloop restores the other types. The log records the number of missing types.
2. Bloop does the target check. The save can wait for another application, thus the check comes after the save.
   If the check fails, Bloop puts the text in the clipboard and shows the fallback card. Bloop posts no paste command.
3. Bloop writes one clipboard item. Its string type uses an on-demand data provider.
   The item also has the `org.nspasteboard.TransientType` and `org.nspasteboard.ConcealedType` markers.
4. Bloop posts Command-V with key code 9 and the Command flag only.
5. The data provider supplies the text when the target reads it. A successful read makes the result `Landed`.
6. 300 ms after the read, Bloop restores the saved items if the clipboard did not change after step 3.
   The restored items have the `org.nspasteboard.TransientType` marker. Thus a clipboard manager does not record them again.
   Then Bloop reports the result. Thus the next paste saves the restored clipboard, not the text of this paste.
7. If no read occurs within 1 second, Bloop replaces the item with a normal text item and shows the fallback card.
   Bloop replaces the item only if the clipboard did not change after step 3.

Bloop posts no more than one paste command for each session.
When a job ends without a paste result, Bloop forgets its target and reports no result for it.
A posted paste command can still land. The restore then follows the read as usual.
Jobs paste in session order.
The target check does not count the key events that Bloop posts. Thus one paste does not block the paste of the next job.

Measured on the reference Mac (macOS 27, 2026-11-01), with the Phase 1 test text: six pastes in a terminal, WhatsApp,
Firefox, and TextEdit gave `Landed`. The target read the item 10–27 ms after the `Insert` command.
This time includes the target check and the paste command. The Tabi reference is 101 ms from paste command to read.
After a paste in TextEdit, the clipboard contained the copied word or the copied image again.
After two pastes in a row, it contained the word that was copied before the first paste.

Known limits:

- A clipboard manager that ignores the markers can read the item first. Then Bloop reports `Landed` without a paste.
- A slow target can read the item after 1 second. Then the field gets the text and the card also shows. This has no bad effect.
- If another application changes the clipboard within the read time limit, the text is not in the clipboard.
  The fallback card still shows the text, and "Copy last result" copies it.
- The save step reads all types of the clipboard. At that time, another application must supply data that it promised.
  For a large image, this can take some time.
- Key code 9 is the V key on the AZERTY and QWERTY layouts. Other layouts are later work.
- A terminal without bracketed paste can execute pasted text that contains line breaks. A correction prompt can return line breaks.
  Phase 3 checks the terminal of the reference Mac.

## Settings and status menu

Phase 3 adds one correction toggle to the existing menu. It is off at each application start.
`bloop-app` captures its value when a generation starts. A change affects only subsequent generations.
Phase 4 adds the design below. Prompt and model submenus depend on useful alternatives from the evaluation.

- `bloop-app` owns the settings. The core does not know them.
- The settings file is `~/Library/Application Support/Bloop/config.toml`. The prompt files are in the `prompts` folder next to it.
- The user and Bloop both edit `config.toml`. For a menu change, Bloop changes one key with `toml_edit`.
  Bloop writes a temporary file and renames it. Thus the comments and the other keys stay.
- At the dictation key press and when the menu opens, Bloop reads a file again if its modification time changed.
  Bloop does not watch the files.
- At the first start, Bloop writes a commented template, an empty dictionary, and the default prompts. If a default prompt file is missing, Bloop writes it again.
- Each generation uses the settings snapshot that exists when the core creates it.
  Thus a change never affects an active capture or a running job.
- If the settings are invalid or the selected prompt is missing, the menu shows the error and its line. A capture then fails with the error view.
- `bloop-app` builds the menu content as a data model. `bloop-macos` renders the model each time the menu opens (`menuNeedsUpdate:`).
  Thus a new feature does not change AppKit code.
- A click in the menu bar during a capture is other input. Thus it cancels the capture.
- When a provider refuses the API key, the status icon changes and the menu shows a status line.

The planned menu:

```text
Status lines: permissions, API key, settings errors
──────────
✓ Correct text
  Prompt ▸        natural-light, natural-full, prompts of the user
  Model ▸         the models of the configured chat provider
──────────
Copy last result
Open config.toml…
Open dictionary…
Open prompts folder…
──────────
Quit Bloop
```

## Dictionary and learning from fixes

The dictionary is the file `~/Library/Application Support/Bloop/dictionary.toml`.
Each entry has a written form. An entry can also have the form that Bloop writes now.

```toml
[[entry]]
written = "objc2"

[[entry]]
heard = "comme Tabby"
written = "comme Taby"
```

- Phase 3 reads the dictionary at start. From Phase 4, the hub reads it again at the dictation key press if it changed.
- The written forms go to the speech provider in `TranscriptionOptions::vocabulary`. The Mistral adapter sends them as `context_bias`.
  Thus the transcription spells them correctly, also when the correction is off. A provider without this feature ignores them.
- When the correction is on, `LlmRefine` adds the pairs to the system message after the prompt.
  The model applies a pair only when the sentence fits.
- Bloop does not replace text directly from the dictionary. A direct replacement can change a correct phrase.
- If the dictionary file is invalid, a capture fails with the error view. The log gives the line. From Phase 4, the menu also shows it.

Phase 6 adds learning from fixes:

1. After a `Landed` paste, Bloop reads the value of the focused element through Accessibility and finds the pasted text in it.
2. Bloop watches the field until the focus leaves it, the pasted text disappears, a new dictation starts, or 60 seconds pass.
3. Bloop compares the pasted text with the edited text, word by word. A replacement of one to three words is a suggestion.
   Bloop ignores changes of punctuation or capital letters only, deletions, long rewrites, and pairs that the user ignored before.
4. A card near the orb shows the suggestion with "Save" and "Ignore". The card accepts clicks and never takes keyboard focus.
5. After 8 seconds without a click, the card closes. The suggestion then stays in the status menu.
6. "Save" adds the pair to the dictionary with `toml_edit`. "Ignore" records the pair. Thus Bloop does not propose it again.

- Bloop never saves an entry without a user action.
- The field text stays in memory during the watch only. The log records only the number of suggestions.
- If Accessibility cannot read the field, for example in some Electron applications or terminals, Bloop proposes nothing.
- During a capture, the card waits. When the capture ends, the card shows, with the same rules as the fallback card.
- The 60-second watch and the 8-second card are initial values. Phase 6 checks them during daily use.

## Data and logs

| Data | Lifetime |
| --- | --- |
| Audio chunk | Until sent, or until cancellation discards the queue |
| Transcript | Until the correction completes |
| Last final text | Until a newer final text replaces it or the application exits, for "Copy last result" |

- Bloop keeps no history on disk.
- The log file is `~/Library/Logs/Bloop/bloop.log`.
- The log records state changes, step durations, fallback reasons, error categories, and panics.
- For each correction, the log records the model identifier, the prompt name, and the prompt hash.
- The log does not contain audio, text, or credentials.
- The settings file is `~/Library/Application Support/Bloop/config.toml`.
- The settings file contains nonsecret settings only: key, hold threshold, language, streaming delay, models, correction state, and prompt name.
- Without the settings file, Bloop writes a template with the default values (Phase 4).
- The prompt files are in `~/Library/Application Support/Bloop/prompts/`. They contain no dictated text.
- The dictionary file contains the user words and the saved or ignored pairs. It contains no other dictated text.

## Bundle and permissions

- `Info.plist` sets `LSUIElement` to true. Bloop has no Dock icon.
- `Info.plist` contains `NSMicrophoneUsageDescription` and the bundle identifier `io.github.robinber.bloop`.
- `scripts/bundle.sh` builds `target/release/Bloop.app` and signs it with `Bloop Dev`,
  a self-signed code signing certificate in the login keychain.
- The designated requirement of the signature is the bundle identifier and the certificate.
  Thus macOS keeps the permissions after each build.
  With an ad hoc signature, macOS removes the permissions after each build.
- Bloop needs three permissions: Microphone, Accessibility (active event tap and paste command), and Input Monitoring (key events).
- The menu shows each missing permission and opens the related System Settings pane.
  Without a missing permission, it shows "Ready: hold Right Command to dictate".
  macOS can keep the old permission result until Bloop starts again. Thus Bloop checks the status at start.
- The menu item "Copy last result" puts the last final text in the clipboard. Before the first result, it has no effect.
- Bloop reads the microphone permission at start. If the user did not answer yet, Bloop shows the request of macOS and does not wait.
  If the user denied access, or a restriction blocks it, the menu shows the permission.
  The menu does not show the answer to the first request. A denial shows in the menu at the next start.
  Without the permission, a capture fails with the error view.
- Without the API key of the selected provider in Keychain, the menu status shows the missing key at start. A capture then fails with the error view.

## Candidate crates

| Need | Crates |
| --- | --- |
| AppKit and Foundation | `objc2`, `objc2-foundation`, `objc2-app-kit` |
| Core Animation | `objc2-quartz-core` |
| Event tap and events | `objc2-core-graphics`, or direct FFI |
| Main queue | `dispatch2` |
| Audio | `cpal`, `rubato`, `rtrb` (bounded queue without allocation in the callback) |
| Microphone permission | `objc2-av-foundation`, `block2`. The authorization methods are `unsafe`; the owner approved them on 2026-11-02 |
| Network | `tokio`, `tokio-tungstenite` with `native-tls`, `futures-util`, `serde_json`, `base64`; `reqwest` with `native-tls`, `http2`, and `json`, without default features, for correction. Both connections then use one TLS stack through `security-framework` |
| Keychain | `security-framework` |
| Settings | `serde`, `toml`, `toml_edit` |
| Prompt hash | `sha2` |
| Accessibility read | `objc2-application-services` (Phase 6; check the API coverage) |
| Errors | `anyhow` (application start only) |
| Logs | `tracing`, `tracing-subscriber`, `tracing-appender` |
| Secrets in memory | `secrecy` |

Tabi uses most of these crates in production.
Check the versions and the API coverage when the project starts.
