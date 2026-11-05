# Roadmap

Status: Design revision 7. Phase 1 is complete. Phase 2 is in progress. Phase 3 is planned.

Complete the acceptance checks for each phase before the next phase.
Record actual results separately from proposed targets.
The [implementation plan](STRUCTURE.md#implementation-plan) gives the crates and files for each phase.

First, prove the key, orb, and insertion with fixed text.
Then add streaming transcription and insert the complete raw transcript. Add correction after that path works.
Streaming is the only STT path. There is no file transcription stage or later streaming migration.

## Phase 0: Design

- [x] Select the name Bloop.
- [x] Limit the first release to personal use on one Mac.
- [x] Inspect the Tabi interface, bundle, settings, and log.
- [x] Select a native orb without a web view.
- [x] Select Right Command as the first dictation key.
- [x] Simplify the architecture and the Rust contracts.
- [x] Define the crate structure and the extension procedures.
- [x] Define a configurable capture duration, a warning before the limit, and processing of the captured audio at the limit.
- [x] Select streaming as the only STT path from Phase 2.
- [x] Review the Phase 2 streaming contracts before implementation.

Acceptance: The owner approves [Scope](SCOPE.md), [Architecture](ARCHITECTURE.md), [Code structure](STRUCTURE.md), and [Contracts](CONTRACTS.md).

## Phase 1: Key, orb, and paste

This phase uses fixed test text. It does not use audio.

- [x] Create the Cargo workspace and the five crates from [Code structure](STRUCTURE.md),
  with the lint floor, `rustfmt.toml`, and `clippy.toml` from `AGENTS.md`.
- [x] Add the script that builds the application bundle.
- [x] Sign the bundle with a stable identity.
- [x] Start the `AppKit` application. Add the menu bar item with "Quit Bloop".
- [x] Add the status and "Copy last result" to the menu.
- [x] Check the Accessibility and Input Monitoring permissions.
- [x] Add the event tap for Right Command, other input, and Escape.
- [x] Write the core state machine and its unit tests.
- [x] Show the orb after 220 ms. Animate it with a test level.
- [x] Paste the test text through the on-demand clipboard item.
- [x] Restore the previous clipboard content.
- [x] Show the fallback card when the target check fails.

Acceptance:

- Right Command with C copies text and shows no orb.
- A press shorter than 220 ms shows no orb and pastes nothing.
- The orb never takes keyboard focus. The text lands in TextEdit, a Firefox field, and a terminal.
- After the paste, the clipboard contains the previous text or image.
- After a landed paste, a clipboard manager, if installed, shows no Bloop entry.
- Escape stops the session and does not go to the target application.

Results on the reference Mac (macOS 27, 2026-11-01): all acceptance checks passed.

- Right Command with C copied the text and showed no orb. A short press showed no orb and pasted nothing.
- The text landed in TextEdit, a Firefox field, a terminal, and WhatsApp, with the focus unchanged.
- The clipboard contained the copied word or image again after the paste, also after two pastes in a row.
- The installed clipboard manager showed no Bloop entry.
- Escape during a capture stopped the session and did not reach the application. Escape at other times operated as usual.

## Phase 2: Microphone and streaming transcription

- [x] Review `Transcriber`, `TranscriptionSession`, `AudioSink`, and `AudioInput` before their adapters.
- [x] Add the audio and streaming contracts to `bloop-core`, with `Transcript` and a pipeline without text steps.
- [x] Test start, ordered audio, finish, cancellation, and failure of the worker with a fake provider.
- [x] Read the API key from Keychain. Without the key, show it in the menu status.
- [x] Pass the key that Bloop reads at start to the Mistral adapter. Do not read Keychain a second time.
- [x] Check the microphone permission at start. First, check if `objc2-av-foundation` gives the check without `unsafe`.
  It does not. The owner approved the `unsafe` calls.
- [x] Start the capture at the hold threshold. Send the audio level to the orb, and remove the test level.
- [x] Open the provider session while capture starts. Buffer initial samples in a bounded queue.
- [x] Convert audio incrementally in the worker, from f32 at the native rate of the microphone to 16-bit PCM at 16 kHz.
  Resample with the FFT resampler. Keep the conversion and network work outside the audio callback.
- [x] Try the realtime API at the native sample rate. If it accepts that rate, the owner decides if Bloop keeps the resampler.
  Voxtral accepted 24 kHz without an error, but it returned no text for AirPods audio. It transcribed 48 kHz.
  On 2026-11-05, the owner decided to convert all captures to 16 kHz, the default rate of the API.
- [x] Send audio chunks to Voxtral during capture, with French and the word list configured at session start.
- [x] Test the Mistral adapter with a fake WebSocket server: setup, audio, end, deltas, done, errors, and a refused handshake.
- [x] At capture end, drain queued audio, flush the converter, and request the complete transcript.
- [x] Paste the complete raw transcript, and remove the fixed test text. Keep partial transcripts outside the insertion path.
- [ ] Cancel capture and its stream on Escape, other input, or microphone loss.
- [x] Cancel all work on system sleep. Observe `NSWorkspaceWillSleepNotification`.
  When it arrives, send `SystemSleep` to the core.
- [ ] For a short or silent capture, cancel transcription and paste nothing. A network session can already be open.
- [x] Let a new capture start while a job runs. Paste the texts in session order.
- [ ] Read and validate `dictation.max_duration_secs`. Reject zero and unsupported values with a clear error.
- [ ] Warn before the configured limit. Stop at the limit and finalize the stream once.
- [ ] Handle a full queue, connection loss, and timeouts without incomplete insertion or automatic replay.
- [ ] Select queue capacities and connection, inactivity, and completion time limits from measurements.
- [ ] Select `target_streaming_delay_ms` from accuracy and completion time measurements.
- [ ] Select the default duration, supported range, and warning interval from tests at 5, 10, and 20 minutes.
- [ ] Record the provider behavior that its documentation does not give: sample rates, session length, inactivity limit, silent session.
- [ ] Log the duration of each step.

Acceptance:

- The transcript contains the first word of each test phrase. If not, start the capture at key press.
- Audio reaches the provider before key release during a normal dictation.
- A slow connection does not lose the first word. Queue exhaustion fails the session instead of silently dropping samples.
- An interruption releases the microphone and the affected stream. Each cancelled generation ignores later results.
- A configured limit stops capture and finalizes transcription once. A later key release creates no second job.
- Partial or failed transcripts cause no insertion. A failure during capture does not leave a job waiting in the queue.
- A failure after capture removes its job and releases the next job for insertion.
- A warning keeps keyboard focus unchanged. A warning from a finished capture has no effect.
- A settings change does not change an active capture. Invalid values prevent capture with a clear error.
- Without the API key, the menu asks to add it. If the user denies access, the menu asks to allow it.
- After the documented `security` command, Bloop reads the key without an access dialog, also after a new build.
- The log never contains the API key.
- Before the first answer, macOS shows the microphone request at start, and the menu shows Ready.
  After a denial, the menu shows the microphone permission at the next start. After a new build, macOS keeps the permission.
- Record memory use and completion time at 5, 10, and 20 minutes. Check provider and queue limits.
- Measure ten live dictations. Record the median from key release to complete transcript beside the Tabi reference of 512 ms.

Measured on the reference Mac (macOS 27, 2026-11-03), with the capture at the hold threshold and the fixed test text:

| Microphone | Format | Start of the microphone | Capture peak |
| --- | --- | --- | --- |
| AirPods | 24 kHz, mono | 109 ms for the first capture, then 34 ms | 0.62 |
| Built-in microphone | 48 kHz, mono | 62 ms | 0.08 |

- The capture started 228 to 230 ms after the key press. The target is the 220 ms hold threshold.
- Escape released the microphone. The log showed no overrun and no queue failure.
- With the automatic gain of the orb, the bars follow both microphones. They react too slowly: a later task tunes them.

Measured on the reference Mac (macOS 27) with Voxtral, before the 16 kHz conversion:

- AirPods at 24 kHz, 2026-11-04, four dictations of 1.1 to 2.7 s: Voxtral echoed `sample_rate` 24000 and sent no delta.
  It returned an empty transcript without an error. The session opened in 315 to 932 ms.
- Built-in microphone at 48 kHz, 2026-11-05, one dictation of 4.6 s: 14 deltas whose text matched the final transcript (61 characters).
  The session opened in 330 ms. The transcript was complete 578 ms after the capture ended.
  The paste landed 912 ms after the capture ended. For comparison, the Tabi log shows 1,143 ms from key up to keystroke for a recent dictation.

Measured on the reference Mac (macOS 27) with Voxtral, with the 16 kHz conversion, on 2026-11-05:

| Microphone | Capture | Deltas | Transcript | Capture end to complete transcript | Capture end to paste |
| --- | --- | --- | --- | --- | --- |
| AirPods, 24 kHz | 8.58 s | 31 | 127 characters | 376 ms | 716 ms |
| Built-in microphone, 48 kHz | 2.97 s | 9 | 38 characters | 680 ms | 1,013 ms |

- The session opened in 324 ms for the built-in microphone.
- The sample count at 16 kHz matched the capture duration: 47,446 samples for 2.965 s.

Measured on the reference Mac (macOS 27) with Voxtral and the built-in microphone at 48 kHz, on 2026-11-05.
Four chained dictations: each capture started while the job of the previous capture waited for its transcript or its paste.
The owner confirmed that the texts landed complete, in session order, in the correct field.

| Capture | Deltas | Transcript | Capture end to complete transcript | Capture end to paste |
| --- | --- | --- | --- | --- |
| 2.45 s | 11 | 41 characters | 1,155 ms | 1,551 ms |
| 4.06 s | 20 | 88 characters | 1,295 ms | 1,605 ms |
| 2.84 s | 14 | 71 characters | 1,001 ms | 1,319 ms |
| 0.80 s | 6 | 17 characters | 1,612 ms | 1,929 ms |

- Three single dictations on the same day: 10.36 s, 8.50 s, and 1.25 s of capture.
  The transcript was complete 451, 966, and 791 ms after the capture ended. The paste landed after 782, 1,283, and 1,141 ms.
- The session opened in 282 to 372 ms, and one time in 1,001 ms.
- The time from capture end to complete transcript changes from 451 to 1,612 ms. These values do not show a relation to the capture duration.

## Phase 3: Correction and daily use

- [ ] Add the Natural Clean prompt to version control.
- [ ] Send the transcript to Mistral Small.
- [ ] Apply the raw fallback rules.
- [ ] Compare raw and final text on a fixed set of French dictations.
- [ ] Check correction completeness and request time limits with long dictations up to the supported capture duration.
- [ ] Use Bloop for one working day on the reference Mac.

Include questions, negations, numbers, names, technical terms, and spoken corrections in the test set.

Acceptance:

- Record the median time from key release to paste. The target is 1.5 seconds or less for ten-second dictations.
- The reviewed samples keep the original meaning. Record every meaning error and fix its cause.
- No paste goes to the wrong application during the test day.
- Record the model identifiers and the prompt version with the results.

If latency exceeds the target, identify the slow stage and measure each adjustment. Record quality and failures with the latency results.
