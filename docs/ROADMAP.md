# Roadmap

Status: Design revision 16 (2026-11-07). Phase 1 and Phase 2 are complete.
On 2026-11-06, the owner declared the functional scope of Phase 3 complete. Phase 4 is now. Later phases depend on daily use.

Phase numbers identify work areas. Follow the priorities below, not the numerical order alone.
Complete the acceptance checks of the selected work before adding more scope.
Record actual results separately from proposed targets.
The [implementation plan](STRUCTURE.md#implementation-plan) gives the crates and files for each phase.

First, prove the key, orb, and insertion with fixed text.
Then add streaming transcription and insert the complete raw transcript. Add correction after that path works.
Streaming is the only STT path. There is no file transcription stage or later streaming migration.
After Phase 2, each phase adds features to a path that operates.
A feature that is off does not change the dictation path. Phase 3 adds one correction toggle to the status menu.

## Priorities

| Priority | Work | Reason to start |
| --- | --- | --- |
| Scope complete | Phase 3: one correction prompt, raw fallback, a toggle, the dictionary, and daily use | Prove that correction reduces manual edits at an acceptable delay. Names and terms need their written form from the start. On 2026-11-06, the owner declared the functional scope complete. |
| Now | Phase 4: settings file, lists in the status menu, and clear errors | The owner decided its interface on 2026-11-06 and validated its design on 2026-11-07. Frequent choices go in the menu. A file change applies without restart. |
| Next if needed | Phase 5: capture at key press | First-word loss still interrupts daily use. Check the loss again after Phase 4 iteration 2. |
| After Phase 4 | Phase 6: learn from fixes | The owner saves a correction from the field with one click. The file writes come from Phase 4. |
| Later | Phase 7: sounds, other audio, and input choices | A specific daily-use problem justifies one feature. |
| Later | Phase 8: other Accessibility features | Define one concrete need before designing or implementing a feature. |

Bloop is useful after Phase 3. Each later phase starts when the reason in this table applies.
Keep the five-crate workspace and the existing ports. Do not add a framework for future features.

## Phase 0: Design

- [x] Select the name Bloop.
- [x] Limit the first release to personal use on one Mac.
- [x] Inspect the Tabi interface, bundle, settings, and log.
- [x] Select a native orb without a web view.
- [x] Select Right Command as the first dictation key.
- [x] Simplify the architecture and the Rust contracts.
- [x] Define the crate structure and the extension procedures.
- [x] Keep capture without a maximum duration, as the owner decided on 2026-11-05.
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
- [x] Cancel capture and its stream on Escape, other input, or microphone loss.
  The owner checked Escape on the real app. Unit tests cover other input and microphone loss.
  On 2026-11-05, the owner decided to do no manual check for these two cases.
- [x] Cancel all work on system sleep. Observe `NSWorkspaceWillSleepNotification`.
  When it arrives, send `SystemSleep` to the core.
- [x] For a short or silent capture, paste nothing. Voxtral returns an empty transcript, and the pipeline sends `NothingHeard`.
  On 2026-11-05, the owner decided to apply no duration or level threshold.
- [x] Let a new capture start while a job runs. Paste the texts in session order.
- [x] Handle a full queue, connection loss, and timeouts without incomplete insertion or automatic replay.
  Fake provider tests cover each case. On 2026-11-05, the owner decided to do no manual network test.
- [x] Select queue capacities and connection, inactivity, and completion time limits from measurements.
  On 2026-11-05, the owner kept the initial values: a queue of 960,000 samples, and limits of 5 s, 5 s, and 15 s.
  The measured session opening (282 ms to 1,001 ms) and completion (376 ms to 1,612 ms) stay below these limits.
- [x] Select `target_streaming_delay_ms` from accuracy and completion time measurements.
  On 2026-11-05, the owner kept 480 ms: the transcripts were satisfactory.
- [x] Record the provider behavior that its documentation does not give: sample rates, session length, inactivity limit, silent session.
  The measured sections below record the sample rates and the silent session.
  The maximum session length and the inactivity limit stay unknown. The owner does not need them:
  dictations are short, and Bloop sends audio without a pause during a capture.
- [x] Log the duration of each step.
  The log gives the microphone start, the session opening, the capture, the drain, the completion, and the paste.
  Phase 3 logs the duration of the correction step.

Acceptance:

- The transcript contains the first word of each test phrase. If not, start the capture at key press.
- Audio reaches the provider before key release during a normal dictation.
- A slow connection does not lose the first word. Queue exhaustion fails the session instead of silently dropping samples.
- An interruption releases the microphone and the affected stream. Each cancelled generation ignores later results.
- Key release stops capture and finalizes transcription once. A repeated key release creates no second job.
- Partial or failed transcripts cause no insertion. A failure during capture does not leave a job waiting in the queue.
- A failure after capture removes its job and releases the next job for insertion.
- Without the API key, the menu asks to add it. If the user denies access, the menu asks to allow it.
- After the documented `security` command, Bloop reads the key without an access dialog, also after a new build.
- The log never contains the API key.
- Before the first answer, macOS shows the microphone request at start, and the menu shows Ready.
  After a denial, the menu shows the microphone permission at the next start. After a new build, macOS keeps the permission.
- Measure ten live dictations. Record the median from key release to complete transcript beside the Tabi reference of 512 ms.

Results on the reference Mac (macOS 27, 2026-11-05): all acceptance checks passed, except the first word.
The owner accepted the lost first word. The owner also accepted the menu tests for the denied key access and microphone.

- First word: the owner lost the first word in some dictations. The microphone records 272 ms (AirPods, median of 21)
  and 292 ms (built-in microphone, median of 26) after the key press: the 220 ms hold, then the microphone start.
  The owner kept the capture start at the hold threshold. Tabi also has a minimum hold of 220 ms.
- Audio before key release: the session opened in 282 to 1,001 ms, before each key release.
  The transcript of an 8.58 s capture was complete 376 ms after the capture ended. A worker test also covers this case.
- Slow connection: a session that opened in 1,001 ms gave a complete text. Tests cover a full queue.
- Interruption, key release, partial or failed transcripts, and failure after capture: core, driver, and worker tests
  cover these cases. The owner checked Escape on the real app. The log shows one `StopCaptureAndProcess` for each dictation.
- API key: on 2026-11-02, the menu asked for the missing key. After that, Bloop read the key at each start,
  also after new builds, with no access dialog. A menu test covers the denied access.
- The complete log contains no API key, no `Bearer` value, and no authorization field.
- Microphone: macOS showed the request on 2026-11-02. The permission stayed after each new build.
  A menu test covers the denial.
- 31 live dictations at 16 kHz: the median from key release to complete transcript is 847 ms (416 to 1,622 ms).
  The Tabi reference is 512 ms. Bloop posts Command-V when the transcript is complete.

Note added later on 2026-11-05: in the Phase 2 tables, the paste time is the time of the `InsertDone` event.
Bloop sends this event after the target read and the 300 ms clipboard restore delay.
In four logged dictations, the `Insert` command came less than 1 ms after the complete transcript.
`InsertDone` came 317 to 328 ms after it. Thus the paste command came about 320 ms before the recorded paste time.

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

Measured on the reference Mac (macOS 27) with Voxtral at 16 kHz, on 2026-11-05, after the AirPods became the default input:

| Microphone | Captures | Peak | Transcript |
| --- | --- | --- | --- |
| AirPods, 24 kHz | 1.72 s, 2.26 s, and 2.62 s | 0.67 to 0.73 | empty, three times |
| AirPods, 24 kHz | 3.58 s to 19.58 s, four captures | 0.71 to 0.77 | 33 to 272 characters |
| Built-in microphone, 48 kHz | 1.25 s to 2.93 s, five captures | 0.09 to 0.11 | 21 to 71 characters |

- The microphone started 245 to 333 ms after the key press with both microphones: the 220 ms hold, then the device start.
- Decision: the capture uses the built-in microphone when the default input is a Bluetooth device.
  The [architecture](ARCHITECTURE.md#audio) gives the rule.
- Owner report on 2026-11-05, after two hours of use with the rule: no lost first word, and faster starts.
  A video in the AirPods kept its sound during a capture, because the AirPods stayed in their high-quality profile.
  The owner prefers this: a capture must not change the sound of the AirPods. Phase 7 keeps this when it adds the audio pause.
  This report is not a measurement. Check the first-word loss again before Phase 5 starts.
- Known limits of the rule. Keep them in view for Phase 5 and Phase 7:
  - A Bluetooth device with a good microphone, for example a conference speaker, also gives way to the built-in microphone.
  - With a USB microphone and a Bluetooth default input, the rule selects the built-in microphone, not the USB microphone.
  - Away from the Mac, the AirPods microphone is not available until the Phase 7 submenu adds the choice.
  - The device reads at capture start cost 135 ms for the first capture after the application start, then 58 ms: no measurable change.
  - Only the selection logic has unit tests. The Core Audio reads and the name match of cpal have manual checks only.

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

Measured on the reference Mac (macOS 27) with Voxtral, silent captures, on 2026-11-05:

| Microphone | Capture | Peak | Deltas | Transcript | Capture end to complete transcript |
| --- | --- | --- | --- | --- | --- |
| AirPods, 24 kHz | 1.34 s | 0.0046 | 0 | empty | 748 ms |
| AirPods, 24 kHz | 2.80 s | 0.0000 | 0 | empty | 861 ms |
| Built-in microphone, 48 kHz | 0.37 s | 0.0024 | 0 | empty | 605 ms |

- For each silent capture, the pipeline sent `NothingHeard`, and Bloop pasted nothing.
- A short "oui" with the built-in microphone: 0.89 s of capture, peak 0.131, and a 4-character transcript that Bloop pasted.
- On 2026-11-05, the peak of speech was 0.05 to 0.15 with the built-in microphone, and 0.46 to 0.94 with the AirPods.
  On 2026-11-04, four AirPods captures for dictation had peaks from 0.013 to 0.148.
  Their speech is not known, because Voxtral returned no text at 24 kHz.

## Phase 3: Optional correction and dictionary

Priority: functional scope complete (owner decision on 2026-11-06). The open items below stay open.
Deliver one correction that the owner can use in the usual applications.
Start with the Mistral chat provider and `natural-light`. Keep `natural-full` as a later option.
The first corrections used `mistral-small-latest`. From 2026-11-06, the correction uses `mistral-medium-latest` (owner decision after the evaluation below).
One status menu toggle turns the correction on or off for subsequent dictations. Bloop keeps the toggle state in `config.toml`.
Phase 4 adds the other settings. They are not required for the daily-use trial.
When the correction is off, the raw transcript path does not change.
The dictionary gives the written form of names and terms. The [architecture](ARCHITECTURE.md#dictionary-and-learning-from-fixes) gives the design.

Measured on 2026-11-05, on the free Mistral tier: each request to `mistral-small-2603` and `mistral-small-latest` returned HTTP 429
`rate_limited` (code 1300), also one request of 5 tokens alone. `ministral-8b-2512` answered with the same key.
After the owner activated a paid tier, the first correction in Bloop used HTTP/2 and took 642 ms
(556 prompt tokens, 66 completion tokens, finish reason `stop`). The transcript was complete 881 ms after key release.
The paste command came 1,529 ms after key release.

Measured on 2026-11-06 with the `eval` tool, on the 25 dev samples. The raw texts have 11 commas, 10 periods, and no colon.
A word error is an added word, a replaced word, or an added "ne".
A removed word is also an error, except a hesitation, a stutter, or the replaced part of a spoken correction.
A number in another form, a technical term joined with a period or a hyphen, and "si il" to "s'il" are not word errors.
The notes below the table record them.

| Prompt (hash) | Model | Runs | Commas | Colons | Median / longest correction | Samples with a word error |
| --- | --- | --- | --- | --- | --- | --- |
| First version (`bf51838bdf5f9736`) | `mistral-small-latest` | 1 | 36 | 3 | 465 / 1,457 ms | 3 |
| Revision 1 (`171853388bc2f202`) | `mistral-small-latest` | 1 | 19 | 2 | 476 / 1,439 ms | 2 |
| Revision 2 (`3ee33f2c08f4acc4`) | `mistral-small-latest` | 2 | 13, 15 | 0 | 538, 428 / 991, 1,225 ms | 2, 4 |
| Revision 3 (`1195ade82806826b`) | `mistral-small-latest` | 2 | 15, 16 | 1, 2 | 439, 488 / 1,395, 1,550 ms | 5, 4 |
| Revision 2 (`3ee33f2c08f4acc4`) | `mistral-medium-latest` | 1 | 15 | 0 | 581 / 2,100 ms | 1 |
| Revision 4 (`716b99c624b435c5`) | `mistral-medium-latest` | 2 | 14 | 0 | 504, 466 / 1,078, 1,352 ms | 0 |

- No run had a fallback. Each reply ended with `stop`.
- With the first version, the model copied words from a prompt example: "à 2 heures, non, 3 heures" became "On se retrouve à 3 heures."
  The prompt then also contained this sample. The new examples contain no test sentence.
- With `mistral-small-latest`, two runs of the same prompt gave different text for up to 4 samples, at temperature 0.
  Its word errors included removed words ("Réponds-moi juste par") and replaced words ("je rentre" to "J'entre").
  The two runs of revision 4 with `mistral-medium-latest` gave the same text.
- A rule that wrote file names with their extension made both models add extensions, for example `structure.md`.
  Revision 4 removes this rule. The dictionary gives written forms such as `Cargo.toml`.
- Revision 4 with `mistral-medium-latest` still writes "deux" as "2" in one sample, and writes `Cargo.toml` and `rt-multi-thread` in another.
  It omits one sentence break in a long sample.
- On the 9 validation samples, revision 4 with `mistral-medium-latest` made no meaning error. It wrote "si il" as "s'il".
  The median correction took 471 ms and the longest 844 ms.
- With the fixed identifier `mistral-medium-3-5`, revision 4 gave the same text as the alias for 24 of 25 samples.
- `mistral-large-latest` returned HTTP 429 `rate_limited` (code 1300) for each request on the owner's plan.

Owner decisions on 2026-11-06:

- The limit was 3 prompt revisions. After revision 3, the owner approved revision 4, which only removes the file name rule.
- The correction uses `mistral-medium-latest`, because it made no word error in the samples.
- No step compares the words of the corrected text with the transcript after the correction.

### Required work

- [x] Remove the legacy event `CaptureLimitReached` from the core, its tests, and the contracts.
- [x] Log the paste command and the target read with the generation.
  Measured on 2026-11-05, three raw dictations after this change: key release to paste command 708, 744, and 1,158 ms.
  The target read the text 3 to 10 ms after the paste command. `InsertDone` came 315 to 318 ms after the read.
- [x] Add the text step contracts to `bloop-core`: `ChatModel`, `ChatReply`, `TextStep`, `StepOutcome`, `StepError`, and the step report.
- [x] Add the `LlmRefine`, `Typography`, and `Trim` steps. Test them with a fake chat model.
- [x] Add the shared HTTP client and the Mistral chat adapter.
  Test them with a fake HTTP server: reply, truncated reply, 401, 403, other errors, and timeout.
- [x] Add `TranscriberConfig`, `ChatConfig`, `build_transcriber`, and `build_chat_model` to `bloop-providers`.
  `bloop-app` builds the providers from typed settings and names no provider. Phase 4 reads these settings from the file.
  `bloop-app` builds the transcriber and the chat model one time at start.
- [x] Write `natural-light` in `prompts/`.
- [x] Add the dictionary types to `bloop-core`. Read `dictionary.toml` at start in `bloop-app`.
- [x] Send the written forms of the dictionary as `TranscriptionOptions::vocabulary`. The Mistral adapter sends them as `context_bias`.
  Remove the empty word list constant.
- [ ] Parked on 2026-11-06 (owner decision): give the dictionary pairs to the correction.
  Since `mistral-medium-latest` does the correction, daily use shows no incorrect dictionary word.
  The owner prefers to work on other parts of Bloop first.
  Start again only when daily use shows a dictionary word that the speech model writes incorrectly again and again.
  Findings of 2026-11-06 with `mistral-medium-latest`:
  - Pairs in the correction prompt failed with three wordings. The model replaced forms in sentences that did not fit.
    For example, it wrote "château de Vercel". It also wrote "toquio" for "tokio" and lost punctuation in other samples.
  - A separate closed question was correct for 6 of 6 dictionary samples, median 372 ms.
    The question asked which pairs the speaker meant, with numbers as the answer.
  - Proposed design: a `DictionaryCheck` step after `Typography`.
    It asks the question only when a heard form occurs in the text, as a whole word and without case.
    Then the code writes the written form of each confirmed pair. A failed question keeps the corrected text.
  - A review approved this design with four changes. The evaluation does not count a dictionary skip as a fallback.
    The match compares one character at a time in the original text. The parser accepts a final period. `max_tokens` is 64.
- [x] If the dictionary file is invalid, log the error with its line. A capture then fails with the error view.
- [x] Write about 30 French samples in `prompts/test-set-fr.toml`. Reserve some samples for validation after prompt changes.
  Include questions, negations, numbers, names, technical terms, spoken corrections, hesitations, dictated instructions, and long dictations.
  Include dictionary pairs in a sentence that fits and in a sentence that does not fit.
  The set has 34 invented samples in 12 categories, with 9 reserved for validation, and 3 dictionary pairs.
  It has 5 punctuation samples for the punctuation task below.
- [x] Add a small evaluation tool for one prompt and one model through the real API.
  Show sample differences for manual review. Record timing, fallback reasons, finish reasons, the provider, the model identifier, and the prompt hash.
  The `eval` binary of `bloop-app` runs the text steps of the application on the samples. The [setup guide](SETUP.md#evaluate-the-correction) gives the command.
  For each sample, it also compares the punctuation of the raw and the final text.
- [x] Reduce the punctuation that the correction adds. On 2026-11-05, the owner found the corrected text too punctuated:
  commas and periods inside spoken sentences make it sound robotic. Repetitions and hesitations were removed correctly.
  The evaluation compares the punctuation of the raw and the corrected text for each sample.
  Revision 4 of `natural-light` gives rules for few commas and no colons. The measurements above give the result.
- [ ] Review the short-text skip and length ratio rules against the samples before enabling them.
  Include "à 2 heures, non, 3 heures" to "à 3 heures": the correct result has 40% of the input length.
  These rules detect some abnormal replies. They do not verify meaning.
- [ ] Select the correction timeout from short and long dictations. Compare the delay with an immediate raw result.
  Check `max_tokens`. Never insert a truncated reply. Long captures do not require correction in separate parts.
- [x] Run the pipeline in the existing worker after `finish`.
- [x] Add one correction toggle to the existing status menu. Do not add the general menu model or settings hub yet.
  Capture its value for each generation. A toggle change must not affect an active capture or a running job.
- [x] Test correction failure and timeout, sleep during correction, and two corrections that finish in reverse order.
  Verify raw fallback, no late insertion after cancellation, and one ordered insertion for each job.
- [x] Log the step report of each job: step name, result, and fallback reason.
  The chat adapter logs the request duration, the token counts, and the finish reason.
- [x] Check if the terminal of the reference Mac protects a paste that contains line breaks (bracketed paste).
  If it does not, define the insertion protection before testing corrected text there.
  On 2026-11-05, Ghostty 1.4.0 with zsh kept two pasted lines without executing them, because zsh turns on bracketed paste.
  Thus Bloop needs no protection step for this terminal.
- [ ] Use Bloop with the correction on for one working day on the reference Mac.

### Acceptance and stopping rule

- Unit tests cover the raw path, an empty transcript, each step result, and each raw fallback reason.
- For each sample, compare the manual edits needed with and without correction: fewer, equal, or more.
  Record changes to names, numbers, negations, questions, and certainty separately.
- The evaluation applies each dictionary pair in a sentence that fits, and keeps the text in a sentence that does not fit.
  This check is parked with the dictionary pairs task.
- For five dictionary words, record how many of ten raw dictations write each word correctly, with and without the dictionary.
- Fix the test set and the maximum number of prompt revisions before evaluation. Keep the reserved validation samples separate.
  Do not continue prompt changes without a new decision when this limit is reached.
- Use a prompt with no known meaning errors in the validation samples. This does not prove correctness for all dictations.
- Record the median and the longest observed delay from key release to paste command, with correction on and off.
  Use comparable dictations of about 10 seconds. The median target remains 1.5 seconds with correction on.
  Record the correction duration, fallback frequency, model identifier, and prompt hash.
- The trial needs fewer manual edits and an acceptable delay. No paste goes to the wrong application during the trial.
- The log contains no transcript, no corrected text, and no dictionary entry.

If the benefit is insufficient, reduce the prompt scope or keep correction off. Record the result and the remaining limits.
An evaluation that selects the raw path is a valid outcome. Do not add optimizations only to keep the phase open.

### Optional work after the first trial

These items do not block Phase 3 completion. Start one only when the measurements justify it.

- Compare correction with and without predicted outputs. Change one factor at a time.
- Measure prompt caching or connection warm-up only if the affected stage contributes enough delay.
- Evaluate `natural-full` or another model only if the first prompt leaves useful corrections undone.
- Check the access to Mistral Large 4 on the owner's plan. If it answers, evaluate it with `eval`.
  Do this before the retirement of Mistral Medium 3.5, which the Mistral documentation gives as 2027-04-30.
- If transcription dominates the delay, evaluate streaming settings as a separate experiment. Keep the Phase 2 baseline.
  Candidates include `target_streaming_delay_ms` at 240, 320, and 480 ms.
  Establish the behavior of `input_audio.flush` before testing it with `input_audio.end`.

## Phase 4: Settings and status menu

Priority: now. Bloop and the user edit one settings file. The status menu shows the frequent choices as lists.
The [architecture](ARCHITECTURE.md#settings-and-status-menu) gives the design.

Owner decisions on 2026-11-06:

- The functional scope is complete. Phase 4 adds configuration and user experience only. The dictation path does not change.
- The interface is the status menu and `config.toml`. The frequent choices are lists in the menu: prompt, correction model, and hold delay.
  The other settings are in the file.
- Bloop has no settings window.
- The dictation key choice is optional and comes last. It accepts modifier keys only.

Owner decisions on 2026-11-07, each one as the design recommended:

- A. The correction time limit stays a constant of 8 s. Nobody requested a change, it is not measured, and it caused no problem.
- B. A missing or invalid prompt affects only the correction. With "Correct text" on, the capture fails with the error view.
  Bloop does not silently paste the raw text. With "Correct text" off, dictation operates.
- C. The built-in prompts stay in the application. The prompts folder holds the prompts of the owner.
  A file with the name of a built-in prompt replaces it.
- E. The menu delegate for iteration 3 is approved in the "status item and menu" surface. `AGENTS.md` records the scope.

Each iteration leaves Bloop usable all day. The owner tests each iteration before the next one starts.

### Iteration 0: design

Iteration 0 records the validated design in the documents, with the owner decisions and the approval.

- [x] Get the owner approval for `NSMenuDelegate` and the menu action selector in the "status item and menu" surface.
  Record it in `AGENTS.md`. Approved on 2026-11-07.

### Iteration 1: settings file at start

The menu does not change. A change in the file applies at the next start.

- [x] Persist the existing "Correct text" toggle in `config.toml` (`[refine] enabled`).
- [ ] Add the settings schema to `bloop-app`, with default values and `schema = 1`. Each error gives the line of the key or of its table.
- [ ] Add `Deserialize` under the vendor feature, `Clone`, `PartialEq`, the `[chat] models` list, and `template()` to the provider configs.
- [ ] Add `files.rs`: the bounded read, the write of a missing file, and the atomic replace.
- [ ] At the first start, write the commented `config.toml` template with `enabled = false`, and create the prompts folder.
  Do not change an existing file.
- [ ] Select the prompt from a user file of the prompts folder, else from the built-in prompts.
  Add `NoCorrection::{ChatUnavailable, InvalidPrompt}`.
- [ ] Replace `HOLD` and `LANGUAGE` with settings. Keep `CORRECTION_LIMIT` as a constant.
- [ ] Select the speech provider and the chat provider in `config.toml`. Add a menu choice when a second adapter exists.

### Iteration 2: hub and settings snapshot

The menu does not change yet. After a hand edit of `[refine] enabled`, the checkmark can be wrong until iteration 3.

- [ ] Add the settings hub. At the dictation key press, read the files again: settings, selected prompt, and dictionary.
  Compare the text, not the modification time.
- [ ] Give each generation the settings snapshot that exists when the core creates it (`StartHoldTimer`). Add the `Snapshots` trait to the driver.
- [ ] Build the chat model again only when `[chat]` changes. Build the correction of each snapshot from the selected prompt and the chat model.
- [ ] Replace the toggle cell with `NoCorrection::Off`. The toggle writes its key through the hub.
- [ ] `Hub::set` writes one key with the write rule, then refreshes. Today, the "Correct text" toggle uses this rule for `[refine] enabled`.

### Iteration 3: menu model

- [ ] Add the menu model types to `bloop-core`. Render the model in `bloop-macos` each time the menu opens.
- [ ] Build the model in `bloop-app` with the items of today. Refresh the settings when the menu opens.
- [ ] Show the status lines: missing permissions, missing or refused API key, and invalid settings, prompt, or dictionary.
- [ ] If the provider refuses the API key (401 or 403) for a transcription, show it in the menu. The line stays until the restart.
- [ ] Add "Open config.toml…", "Open prompts folder…", and "Open dictionary…".
- [ ] At the first start, write a commented empty `dictionary.toml`.
- [ ] Check the name and the System Settings link of the Input Monitoring permission on macOS 27.
- [ ] Do the manual checks of the menu renderer in the [code structure](STRUCTURE.md#tests).
  Add `bloop-macos/src/menu.rs` to the critical surfaces of `AGENTS.md`.

### Iteration 4: lists

- [ ] Add the "Prompt", "Model", and "Hold delay" submenus. Each choice writes one key.
  The owner decided these lists on 2026-11-06. Only `bloop-app` changes.

### Iteration 5: dictation key (optional)

- [ ] Select the dictation key in `config.toml`: modifier keys only. The change is in `keys.rs`.

### Acceptance

- A menu change applies from the next dictation. It does not change an active capture or a running job.
- A change to `config.toml`, to a prompt file, or to the dictionary in an editor applies from the next dictation, without restart.
- Bloop reads the files again at the dictation key press and when the menu opens. A file without a change builds no new snapshot.
- After a menu change, `config.toml` keeps its comments and its other keys.
- With invalid settings or an invalid dictionary, the menu shows the error and its line. A capture fails with the error view.
- With a missing or invalid prompt and the correction on, the menu shows the error, and a capture fails with the error view.
  With the correction off, dictation operates.
- A file over its size limit is invalid, and the menu shows the error. Bloop reads at most one byte over the limit.
- The tag guard ignores a menu item with tag 0. A click that gives no valid action has no effect, and the log records it.
- Check that the settings integration preserves the Phase 3 correction and raw paths.

## Phase 5: Capture at key press

Priority: next if first-word loss still interrupts daily use. It does not depend on the Phase 4 settings.
On 2026-11-05, the owner reopened the capture start at the hold threshold, because some dictations lost their first word.
Later that day, after the built-in microphone rule, the owner reported no lost words. Check the loss again before this phase starts.

- [ ] On the reference Mac, check the effect of a short microphone start at each Right Command press.
  Check the microphone indicator in the menu bar and the audio of AirPods.
- [ ] From these results, design the start at key press. Update the architecture and the contracts.
- [ ] Test a short press, a combination, and a capture after the hold threshold with core and driver tests.

Acceptance:

- The transcript contains the first word of each test phrase.
- A Right Command shortcut or a short press sends no audio to the provider.
- Record the effect on the microphone indicator and on AirPods audio.

## Phase 6: Learn from fixes

Priority: after Phase 4, because a saved entry needs the file writes of Phase 4.
After a paste, the user corrects a word in the field. Bloop proposes to save the correction in the dictionary.
The [architecture](ARCHITECTURE.md#dictionary-and-learning-from-fixes) gives the design.

- [ ] Get the owner approval for the Accessibility read of the focused element as a new `unsafe` surface.
  Record it in `AGENTS.md`.
- [ ] Review the signatures of the comparison function, the field watch port, and the suggestion orb view.
- [ ] Add the comparison of the pasted text and the edited text to `bloop-core`, with unit tests.
  Test a replaced word, a replaced group of words, punctuation only, capital letters only, a deletion, and a long rewrite.
- [ ] After a `Landed` paste, watch the focused field until the focus leaves it, the pasted text disappears,
  a new dictation starts, or 60 seconds pass.
- [ ] Show the suggestion card with "Save" and "Ignore". The card accepts clicks and never takes keyboard focus.
- [ ] After 8 seconds without a click, close the card and keep the suggestion in the status menu.
- [ ] Save or ignore the pair in the dictionary file with `toml_edit`. Do not propose an ignored pair again.

Acceptance:

- In TextEdit, a Firefox field, and the applications of daily use, a replaced word gives one suggestion.
- Bloop never saves an entry without a click on "Save" or on the menu item.
- A click on the card does not move the keyboard focus or the text cursor.
- A field that Accessibility cannot read gives no suggestion and no error view.
- The log and the disk never contain the field text. Only a saved or ignored pair goes to the dictionary file.

Optional: snippets, a spoken trigger that inserts a stored text. Add them only when a repeated use case needs them.
Apply them in a text step after the correction. Thus the model cannot change them.

## Phase 7: Sounds, other audio, and input choices

Priority: later. Select one feature for a specific daily-use problem; do not treat this list as one required delivery.

- [ ] Add start and stop sounds, with a menu toggle.
- [ ] Pause or lower other audio during the capture, with a menu toggle.
- [ ] Add a "Microphone" submenu with the input devices. A selected device replaces the built-in microphone rule.
- [ ] Add a "Language" submenu. The language goes to the transcription session and to the text steps.
- [ ] Get the owner approval for each new `unsafe` surface, for example `NSSound` or Core Audio.

## Phase 8: Other Accessibility features

Priority: later.
Select one need, define its acceptance checks, and assess its cost before implementation.
These features reuse the Accessibility read of Phase 6.
Each new `unsafe` surface needs the owner approval.

- [ ] Fit the text to the cursor: first letter, spaces, and final period.
- [ ] Read the names in the front window. Give them to the transcription and to the correction.
- [ ] Undo a cancel: for 5 seconds after Escape, let the user get the text of the cancelled session.
- [ ] Edit the selection by voice: replace the selected text with the result of a dictated instruction.

Phases 7 and 8 list candidate features. Detail only the selected feature before work starts.
