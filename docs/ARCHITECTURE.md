# Architecture

Status: Proposed design, revision 2 (2026-10-31). No components exist yet.

## Principles

- Bloop does one task: dictation into the focused field.
- The behavior follows Tabi. The design stays small.
- All code uses Rust. The interface uses AppKit through `objc2`. Bloop has no web view.
- Bloop never loses the text. If a paste is not possible, Bloop shows the text and copies it.

## Process

```text
Right Command held for 220 ms
    -> the orb opens and the capture starts
Right Command released
    -> the capture stops and the orb closes
    -> Voxtral transcription
    -> Mistral Small correction
    -> target check
    -> paste, or fallback card
    -> the orb closes
```

## Code structure

Bloop is one Cargo workspace with five crates. The [code structure](STRUCTURE.md) gives the details.

| Crate | Responsibility |
| --- | --- |
| `bloop-core` | Domain types, session state machine, pipeline, text steps, and ports |
| `bloop-providers` | AI providers; one module for each vendor |
| `bloop-audio` | Microphone capture and WAV conversion |
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
| Worker | Tokio runtime: WAV conversion and Mistral requests |

- The audio callback does not allocate memory, lock, or access files.
- The audio level is an atomic value. The orb reads it on the main thread.
- The worker sends each result to the main queue with its generation.
- The core ignores a result for an unknown generation, for example a cancelled job.
- The event tap callback returns quickly. It only sends events to the core.

## Session states

A session has two parts: the capture, then a job.
The capture uses the microphone and the orb. The job runs without a visible indicator.

```text
Capture: Idle -> Pending -> Listening -> Idle
Job:     Processing -> Ready -> Inserting -> end
```

| Capture state | Starts when | Ends when |
| --- | --- | --- |
| `Pending` | The user presses Right Command alone. | 220 ms pass: `Listening`. Key release or other input: `Idle`, with no signal. |
| `Listening` | The hold threshold passes. | Key release or 120 seconds: `Idle` and a new job. Other input or Escape: `Idle`, with no job. |

| Job status | Starts when | Ends when |
| --- | --- | --- |
| `Processing` | The capture stops. | Text: `Ready`. Nothing heard: end, with no paste. Error: end, with the error view. |
| `Ready` | The final text is available. | All earlier jobs are complete: `Inserting`. |
| `Inserting` | The paste module receives the text. | Paste or fallback card: end. |

- Bloop permits one capture at a time.
- A new capture can start while jobs run. Thus the user can dictate again before the previous text lands.
- Jobs paste their text in session order.
- Escape cancels the capture. It does not cancel a job.
- Microphone loss cancels the capture. System sleep cancels the capture and all jobs.
- Cancellation deletes the audio, stops the network task, and closes the orb.

## Dictation key

- The event tap runs on the main run loop. It is an active tap, thus it needs Accessibility.
- Right Command has key code 54. The tap reads it from `flagsChanged` events.
- A key press or a mouse click during the hold is a combination. The session stops with no signal.
  Thus Right Command shortcuts and Command-click continue to operate.
- During `Pending` and `Listening`, the tap blocks Escape. At other times, Escape goes to the application as usual.
- macOS disables a tap after a slow callback. The callback enables the tap again when it receives that event.
- Bloop marks the events that it posts. The tap ignores these events.
- Secure input stops key events to the tap. In this condition, the dictation key has no effect. This is a known limit.

## Orb

- The orb is an `NSPanel` with the borderless and nonactivating styles.
  It never becomes the key window or the main window.
- The panel ignores mouse events and has a clear background.
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
| Bars | Seven `CALayer` bars with round ends, used as the mask of one `CAGradientLayer` (orange at the bottom, yellow at the top) |

The initial dimensions come from a Tabi screen capture on the reference Mac. Adjust them by eye in Phase 1.

| Element | Initial size in points |
| --- | --- |
| Pill | 150 × 56 |
| Application icon | 20 × 20 |
| Bar | Width 4; 11.5 between bar centers; height from 6 to 24 |

- The icon shows the frontmost application when the orb opens.
- If the user activates another application during the session, the icon changes.
- For a browser, the orb shows the browser icon. Website icons are later work.
- The layer uses the application icon directly. Bloop does not convert or cache icons.

### Bar animation

- During `Listening`, a 30 Hz timer reads the audio level on the main thread.
- Bloop converts the level to decibels and smooths it: fast rise, slow fall.
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
| Closing | At key release, the bars go down, then the pill shrinks and fades out, in about 200 ms |
| Error | The pill opens with red bars for about 1 second, then closes |
| Fallback card | The pill becomes a card with the final text and "Copied, press ⌘V". It closes after 8 seconds or at the next capture. |

Bloop does not use Tabi code or assets.

## Audio

- The capture uses the system default input device in its native format.
- Bloop allocates the sample buffer once, for 120 seconds of mono audio. The capture stops at this limit.
- The callback mixes the channels to mono and calculates the level of each buffer.
- After the capture, the worker converts the samples to 16 kHz mono 16-bit WAV in memory (`rubato`, `hound`).
  120 seconds of audio gives approximately 3.8 MB.
- If the capture is shorter than 300 ms, or the level stays below a threshold, Bloop sends no request and pastes nothing.
  The bars already showed the silence during the capture.
- Bloop deletes the audio after the transcription or a cancellation. Bloop never writes audio to disk.

## Mistral requests

One HTTP client (`reqwest`) serves both requests.
The API key comes from Keychain (service `bloop`, account `mistral`).
The user stores the key with the `security add-generic-password` command.

| Request | Model | Settings | Time limit |
| --- | --- | --- | --- |
| Transcription | `voxtral-small-transcribe-3-0` | Language `fr`; one `context_bias` field for each word list entry | 15 s |
| Correction | `mistral-small-2603` | `reasoning_effort="none"`; versioned Natural Clean prompt | 8 s |

The processing has no visible indicator. Thus Bloop must not lose a dictation for a short network problem.
After a network error, a timeout, or an HTTP 429 or 5xx status, Bloop sends the transcription request again one time.
If the second request fails, the job ends with the error view.

Raw fallback rules:

- A transcript with fewer than 4 words gets no correction.
- If the correction fails, times out, or returns empty text, Bloop uses the raw transcript.
- If the final text is shorter than 50% or longer than 160% of the transcript, Bloop uses the raw transcript.
- The log records the reason for each fallback.

Latency targets, as medians for 10 seconds of speech:

| Step | Target | Tabi median |
| --- | --- | --- |
| Transcription after key release | 650 ms | 512 ms |
| Correction | 650 ms | 595 ms |
| Paste command | 150 ms | 115 ms |
| Key release to paste command | 1500 ms | 1297 ms |

If the median stays above 1.8 seconds, evaluate `voxtral-mini-transcribe-realtime-3-0` over WebSocket.
The model identifiers and request parameters come from the Mistral documentation on 2026-10-31.
No API request has run for Bloop.

References: [Mistral audio](https://docs.mistral.ai/studio/audio/overview),
[custom vocabulary](https://docs.mistral.ai/studio/audio/speech-to-text/bias-context-and-vocabulary),
[Mistral Small 4](https://docs.mistral.ai/models/mistral-small-4-0-26-03),
and [reasoning settings](https://docs.mistral.ai/studio/conversations/reasoning).

## Paste

When the user releases the dictation key, the paste module records the target:

- the process identifier of the frontmost application;
- the counters of key-down and mouse-down events (`CGEventSourceCounterForEventType`).

The target check before the paste:

1. The frontmost application has the same process identifier.
2. The event counters did not change.
3. Secure input is not active (`IsSecureEventInputEnabled`).

If the check passes:

1. Bloop saves all items of the current clipboard.
2. Bloop writes one clipboard item. Its string type uses an on-demand data provider.
   The item also has the `org.nspasteboard.TransientType` and `org.nspasteboard.ConcealedType` markers.
3. Bloop posts Command-V with key code 9 and the Command flag only.
4. The data provider supplies the text when the target reads it. This read is the `Landed` result.
5. 300 ms after the read, Bloop restores the saved items if the clipboard did not change after step 2.
6. If no read occurs within 1 second, Bloop replaces the item with a normal text item and shows the fallback card.

If the check fails, Bloop puts the text in the clipboard and shows the fallback card. Bloop posts no paste command.
Bloop posts no more than one paste command for each session.
Jobs paste in session order.
The target check does not count the key events that Bloop posts. Thus one paste does not block the paste of the next job.

Known limits:

- A clipboard manager that ignores the markers can read the item first. Then Bloop reports `Landed` without a paste.
- A slow target can read the item after 1 second. Then the field gets the text and the card also shows. This has no bad effect.
- The restore step reads all saved types. At that time, another application can supply data that it promised.
- Key code 9 is the V key on the AZERTY and QWERTY layouts. Other layouts are later work.
- A terminal without bracketed paste can execute pasted text that contains line breaks. Phase 3 checks the terminal of the reference Mac.

## Data and logs

| Data | Lifetime |
| --- | --- |
| Audio | Until the transcription completes or the session stops |
| Transcript | Until the correction completes |
| Last final text | Until a newer final text replaces it or the application exits, for "Copy last result" |

- Bloop keeps no history on disk.
- The log file is `~/Library/Logs/Bloop/bloop.log`.
- The log records state changes, step durations, fallback reasons, and error categories.
- The log does not contain audio, text, or credentials.
- The settings file is `~/Library/Application Support/Bloop/config.toml`.
- The settings file contains nonsecret settings only: key, hold threshold, language, word list, models, and prompt version.
- Without the settings file, Bloop uses default values.

## Bundle and permissions

- `Info.plist` sets `LSUIElement` to true. Bloop has no Dock icon.
- `Info.plist` contains `NSMicrophoneUsageDescription` and a stable bundle identifier.
- A stable signing identity, such as an Apple Development certificate, signs each build.
  With an ad hoc signature, macOS removes the permissions after each build.
- Bloop needs three permissions: Microphone, Accessibility (active event tap and paste command), and Input Monitoring (key events).
- The menu shows each missing permission and opens the related System Settings pane.

## Candidate crates

| Need | Crates |
| --- | --- |
| AppKit and Foundation | `objc2`, `objc2-foundation`, `objc2-app-kit` |
| Core Animation | `objc2-quartz-core` |
| Event tap and events | `objc2-core-graphics`, or direct FFI |
| Main queue | `dispatch2` |
| Audio | `cpal`, `rubato`, `hound` |
| Network | `reqwest` with `rustls`, `tokio` |
| Keychain | `security-framework` |
| Settings | `serde`, `toml` |
| Errors | `thiserror`, `anyhow` (application start only) |
| Logs | `tracing`, `tracing-subscriber`, `tracing-appender` |
| Secrets in memory | `secrecy` |

Tabi uses most of these crates in production.
Check the versions and the API coverage when the project starts.
