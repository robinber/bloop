# Roadmap

Status: Design revision 3. Phase 1 is in progress. Phases 2 and 3 are planned.

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
- [ ] Review the Phase 2 streaming contracts before implementation.

Acceptance: The owner approves [Scope](SCOPE.md), [Architecture](ARCHITECTURE.md), [Code structure](STRUCTURE.md), and [Contracts](CONTRACTS.md).

## Phase 1: Key, orb, and paste

This phase uses fixed test text. It does not use audio.

- [x] Create the Cargo workspace and the five crates from [Code structure](STRUCTURE.md),
  with the lint floor, `rustfmt.toml`, and `clippy.toml` from `AGENTS.md`.
- [x] Add the script that builds the application bundle.
- [x] Sign the bundle with a stable identity.
- [x] Start the `AppKit` application. Add the menu bar item with "Quit Bloop".
- [ ] Add the status and "Copy last result" to the menu.
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

## Phase 2: Microphone and streaming transcription

- [ ] Review `Transcriber`, `TranscriptionSession`, `AudioSink`, and `AudioInput` before their adapters.
- [ ] Test start, ordered audio, finish, cancellation, and failure with a fake provider.
- [ ] Check the microphone permission.
- [ ] Read the API key from Keychain.
- [ ] Start the capture at the hold threshold. Send the audio level to the orb.
- [ ] Open the provider session while capture starts. Buffer initial samples in a bounded queue.
- [ ] Convert audio incrementally to 16 kHz mono PCM in the worker. Keep network work outside the audio callback.
- [ ] Send audio chunks to Voxtral during capture, with French and the word list configured at session start.
- [ ] Read and validate `dictation.max_duration_secs`. Reject zero and unsupported values with a clear error.
- [ ] Warn before the configured limit. Stop at the limit and finalize the stream once.
- [ ] At capture end, drain queued audio, flush the converter, and request the complete transcript.
- [ ] Cancel capture and its stream on Escape, other input, or microphone loss. Cancel all work on system sleep.
- [ ] Handle a full queue, connection loss, and timeouts without incomplete insertion or automatic replay.
- [ ] Select queue capacities and connection, inactivity, and completion time limits from measurements.
- [ ] Select the default duration, supported range, and warning interval from tests at 5, 10, and 20 minutes.
- [ ] For a short or silent capture, cancel transcription and paste nothing. A network session can already be open.
- [ ] Paste the complete raw transcript. Keep partial transcripts outside the insertion path.
- [ ] Let a new capture start while a job runs. Paste the texts in session order.
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
- Record memory use and completion time at 5, 10, and 20 minutes. Check provider and queue limits.
- Measure ten live dictations. Record the median from key release to complete transcript beside the Tabi reference of 512 ms.

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
