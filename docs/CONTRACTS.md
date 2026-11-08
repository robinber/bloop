# Behavioral contracts

These invariants apply to the current implementation.
Exact types and signatures are in [bloop-core](../crates/bloop-core/src/lib.rs).
The [architecture](ARCHITECTURE.md) describes the runtime, and the [source map](STRUCTURE.md) locates each component.

## Session and job order

| Event or condition | Required behavior |
| --- | --- |
| Dictation key down while idle | Create one generation and take its settings snapshot. Start the hold timer. |
| Short press or combination during the hold | Return to idle without recording or feedback. Ignore the stale timer. |
| Hold threshold with three existing jobs | Do not start recording. Show the error indicator. |
| Key release while listening | Record the target, stop capture, create one processing job, and close the indicator. |
| Escape or other input while listening | Cancel capture and its stream. Do not create a job. |
| Complete transcript | Process the text and make the matching job ready. Never insert a partial transcript. |
| Empty transcript | Remove the job and its target. Paste nothing. |
| Ready jobs | Insert only the first job. Preserve capture order even when work completes in reverse order. |
| Capture failure after release | Fail the matching processing job and release its resources. |
| Job failure | Remove the job and unblock later jobs. Show an error only when no capture is active. |
| System sleep | Cancel capture and all jobs. Ignore their later results. |
| Unknown generation | Ignore the event. |
| Copy last result | Copy the text of the latest insertion attempt, when one exists. |

A fallback card waits while capture is active. A newer deferred card replaces the previous one.
A capture failure or system sleep discards the deferred card.
Escape does not cancel processing jobs. A posted paste command cannot be withdrawn.

## Streaming and audio

- Each generation owns one bounded sample queue and one transcription task.
- Capture starts without waiting for the provider connection.
- The worker converts native mono samples to 16 kHz and sends chunks in capture order.
- The callback does not allocate, lock, perform I/O, or log.
- A full queue or a lost microphone fails the affected dictation. Queue overflow never silently discards samples to continue a session.
- System audio overruns are counted and logged at capture end. They do not stop capture.
- The driver sets the queue end mark before stopping the producer.
- The producer must be dropped with synchronization after that mark. A producer lost without the mark is a failure.
- Normal completion drains accepted samples, flushes conversion, and calls `finish` once.
- `finish` consumes the session and returns one complete transcript.
- Dropping the session or its pending connection or finish future releases local resources.
- Cancelling an audio send also discards the session. The chunk is never retried.
- Partial text stays inside the adapter. A failed stream cannot enter raw fallback.
- No deadline limits recording duration. Connection, send, and finish deadlines apply separately.

The provider adapter owns bounded network buffers.
The Mistral adapter drains at most 64 pending events before each audio send.
A complete transcript does not wait for a successful WebSocket close.

## Text pipeline

The pipeline applies steps in order. Each step returns an applied value, a skip reason, or a typed error.
A failed or skipped step preserves its input for the next step.
An empty transcript runs no steps and produces `NothingHeard`.

With correction off, the step list is empty and the raw transcript becomes the final text.
With correction on, the shared list is `LlmRefine`, `Typography`, then `Trim`.
A failed correction therefore retains the transcript but still applies typography and trim.

- The model receives the prompt as its system message and the transcript between delimiters as its user message.
- A reply must be complete and non-empty.
- The output budget is calculated from input length and clamped to 64–2,048 tokens.
- The short-text skip and character-ratio checks are disabled in the application.
- The correction request has an eight-second limit. A failure continues with the existing text.
- The application and `eval` use the same step constructor and pipeline.

A missing or invalid selected prompt blocks dictation when correction is enabled.
With correction disabled, the prompt is not read.
This configuration failure differs from a request failure after a complete transcript exists.

## Clipboard transaction

The paste adapter records a target for each generation and removes it when that job ends.
Before a paste, it checks the frontmost process, input counters, and secure input.
The clipboard snapshot occurs before this check because promised clipboard data can take time to arrive.

- Each generation posts at most one Command-V event pair.
- A clipboard read confirms `Landed`; it does not prove insertion into the intended field.
- The adapter restores saved contents only while its change count still owns the clipboard.
- Deferred writes to the general clipboard must check their transaction phase and clipboard ownership.
- Cancellation suppresses the result event. A paste already posted can still cause a read.
- Before a new insertion saves the clipboard, the adapter finalizes the previous transaction.
- Normal menu quit returns from the AppKit loop and drops the driver, which finalizes the paste transaction.
- The Tokio runtime then shuts down in the background, so a blocked name lookup does not delay the exit.
- Forced termination or a process crash cannot guarantee restoration.
- A deferred fallback must not overwrite a newer clipboard value. An immediate target-check fallback copies the result for manual paste.
- The recovery card must not assert that text was copied when that write is uncertain.

The clipboard snapshot is limited to 64 items and 64 MiB.
If some types cannot be read, restoration keeps only the captured types.
If no valid snapshot exists, automatic restoration is unavailable.
The latest result remains in memory for **Copy last result** until replaced or the process exits.

## Settings and menu

The hub owns parsed settings, file texts, the current snapshot, and the chat client.
It runs on the main thread. Its internal borrow must not cross an AppKit call.

- Refresh reads bounded files at key press and menu open.
- An unchanged set of file texts reuses the current snapshot.
- The driver takes one snapshot at `StartHoldTimer`; that generation uses it throughout.
- A prompt change reuses the chat client. A chat configuration change rebuilds it.
- Invalid configuration or dictionary files block capture with `InvalidSettings`.
- Menu writes parse the current file first and refuse an invalid file.
- A comment after the last table moves below a table that a menu change adds.
- A write uses a uniquely and exclusively created temporary file in the destination folder.
- The file is synchronized and closed before rename. An error triggers cleanup of the temporary file.
- Atomic replacement does not merge simultaneous edits. The last successful replacement wins.
- Startup creates missing templates and folders without overwriting existing files.
- API keys are read at startup, outside the event tap. A changed key requires a restart.

The application builds `MenuModel`; the macOS crate renders it.
The menu refreshes at each open. Action identifiers refer only to the current model.
The renderer encodes an action as its index plus one. Tag zero is invalid.
Unknown identifiers and invalid tags are logged and ignored.
The menu source and Objective-C callbacks must not panic.

## Privacy and errors

Audio, transcript, final-text, prompt, dictionary, and request types must redact their content in `Debug`.
`TranscriptionOptions` shows the vocabulary count, not the entries.
Keys use `SecretString` and remain outside settings, command arguments, logs, and error messages.

Provider errors use local categories and HTTP status codes.
Logs must not contain arbitrary provider error fields, response bodies, or unknown event and finish strings.
The HTTP adapter discards error bodies. The WebSocket adapter discards error details.
Logs may record durations, counts, controlled error categories, the frontmost application at key release, and nonsecret settings such as the model identifier.

The application installs no bridge from the `log` crate.
Do not add one without reviewing dependency logs: WebSocket handshake traces can contain authorization headers.
The evaluation tool writes its selected samples and final text to the terminal by design.

## Verification surfaces

Repository checks are listed in [AGENTS.md](../AGENTS.md).
Existing tests cover pure state transitions, fake providers, local protocol servers, settings files, and clipboard helpers.
They do not establish real permission, microphone, network, or AppKit behavior.

For a change to a critical surface, use the relevant checks:

| Surface | Required checks |
| --- | --- |
| Core and driver | Event order, stale generations, job limits, failure after release, reverse completion, and cleanup |
| Worker and streaming | Ordered audio, full queue, lost producer, finish, timeout, cancellation, and late results |
| Text pipeline | Empty text, incomplete reply, correction failure, raw fallback, typography, and trim |
| Settings and hub | Defaults, invalid and oversized files, comments, failed writes, snapshot isolation, and client reuse |
| Privacy | Sentinel values in provider responses and text-bearing `Debug` output must not enter logs |
| Paste lifecycle | Delayed restore, cancellation, a newer clipboard owner, replacement insertion, and shutdown |

Manual checks on the reference Mac must cover:

- Short hold, Right Command combinations, Escape repeats and release, and sleep during capture.
- Microphone permission and selection, silence, long capture, and consecutive dictations.
- Paste in native apps, a browser, and a terminal; changed target; secure input; and an unread clipboard item.
- Clipboard text and image restoration, consecutive pastes, cancellation, and quit during a pending transaction.
- Menu refresh, checkmarks, submenus, keyboard quit, and file and permission links.
- Correction on and off, a failed correction, and manual recovery.

Miri does not cover calls to macOS frameworks. The workspace has no CI.
Record checks that actually ran, and report all execution gaps.
