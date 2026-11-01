# Roadmap

Status: Design revision 2. All development phases are open.

Complete the acceptance checks for each phase before the next phase.
Record actual results separately from proposed targets.
The [implementation plan](STRUCTURE.md#implementation-plan) gives the crates and files for each phase.

## Phase 0: Design

- [x] Select the name Bloop.
- [x] Limit the first release to personal use on one Mac.
- [x] Inspect the Tabi interface, bundle, settings, and log.
- [x] Select a native orb without a web view.
- [x] Select Right Command as the first dictation key.
- [x] Simplify the architecture and the Rust contracts.
- [x] Define the crate structure and the extension procedures.
- [ ] Approve revision 2 of the design.

Acceptance: The owner approves [Scope](SCOPE.md), [Architecture](ARCHITECTURE.md), [Code structure](STRUCTURE.md), and [Contracts](CONTRACTS.md).

## Phase 1: Key, orb, and paste

This phase uses fixed test text. It does not use audio.

- [x] Create the Cargo workspace and the five crates from [Code structure](STRUCTURE.md),
  with the lint floor, `rustfmt.toml`, and `clippy.toml` from `AGENTS.md`.
- [x] Add the script that builds the application bundle.
- [x] Sign the bundle with a stable identity.
- [x] Start the `AppKit` application. Add the menu bar item with "Quit Bloop".
- [ ] Add the status and "Copy last result" to the menu.
- [ ] Check the Accessibility and Input Monitoring permissions.
- [ ] Add the event tap for Right Command, other input, and Escape.
- [x] Write the core state machine and its unit tests.
- [ ] Show the orb after 220 ms. Animate it with a test level.
- [ ] Paste the test text through the on-demand clipboard item.
- [ ] Restore the previous clipboard content.
- [ ] Show the fallback card when the target check fails.

Acceptance:

- Right Command with C copies text and shows no orb.
- A press shorter than 220 ms shows no orb and pastes nothing.
- The orb never takes keyboard focus. The text lands in TextEdit, a Firefox field, and a terminal.
- After the paste, the clipboard contains the previous text or image.
- A clipboard manager, if installed, shows no Bloop entry.
- Escape stops the session and does not go to the target application.

## Phase 2: Microphone and transcription

- [ ] Check the microphone permission.
- [ ] Start the capture at the hold threshold. Send the audio level to the orb.
- [ ] Convert the audio to 16 kHz mono WAV in memory.
- [ ] Stop the capture at 120 seconds, on system sleep, and on microphone loss.
- [ ] Detect a low audio level without a network request.
- [ ] Read the API key from Keychain.
- [ ] Send the WAV data to Voxtral with French and the word list.
- [ ] Paste the raw transcript.
- [ ] Let a new capture start while a job runs. Paste the texts in session order.
- [ ] Log the duration of each step.

Acceptance:

- The transcript contains the first word of each test phrase. If not, start the capture at key press.
- An interruption releases the microphone and returns Bloop to `Idle`.
- Ten dictations give a median transcription time. Record it next to the Tabi median of 512 ms.

## Phase 3: Correction and daily use

- [ ] Add the Natural Clean prompt to version control.
- [ ] Send the transcript to Mistral Small.
- [ ] Apply the raw fallback rules.
- [ ] Compare raw and final text on a fixed set of French dictations.
- [ ] Use Bloop for one working day on the reference Mac.

Include questions, negations, numbers, names, technical terms, and spoken corrections in the test set.

Acceptance:

- The median time from key release to paste is 1.5 seconds or less. If not, start the real-time stream work.
- The reviewed samples keep the original meaning. Record every meaning error and fix its cause.
- No paste goes to the wrong application during the test day.
- Record the model identifiers and the prompt version with the results.
