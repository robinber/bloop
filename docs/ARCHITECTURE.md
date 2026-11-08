# Architecture

Bloop is a personal macOS dictation app. It uses Rust, native AppKit, and direct connections to the Mistral API.
The interface is a menu bar item and a small indicator, the orb. There is no window, no web view, and no application server.

This document describes the crates, the runtime, the invariants, and the limits of the current implementation.
For each invariant, it names the component that enforces it and the tests that check it. Exact types and signatures are in the source.

## Terms

| Term | Meaning |
| --- | --- |
| Dictation key | Right Command, held during speech input |
| Hold threshold | The minimum hold time before the orb opens and the capture starts: `hold_ms`, 220 ms by default |
| Capture | The microphone recording, from the hold threshold to key release, cancellation, or failure |
| Job | The work after the capture: transcription completion, correction, and paste |
| Generation | The number that identifies one dictation; the core ignores events of an unknown or cancelled generation |
| Settings snapshot | The settings, and the objects built from them, that one generation uses |
| Transcript | The complete text that the speech provider returns after the audio ends |
| Final text | The text that Bloop pastes: the corrected text or the raw transcript |
| Target check | The checks before the paste: same frontmost process, no new input, no secure input |
| Landed | The result when the target reads the clipboard item after the paste command |
| Fallback card | The orb view that shows the final text when a paste is not possible |
| Core | The pure state machine in `bloop-core` that receives events and returns commands |
| Driver | The `bloop-app` component that gives events to the core and executes its commands |
| Hub | The `bloop-app` component that owns the settings files and the snapshots |
| Port | A trait in `bloop-core` that an adapter implements |
| Adapter | The implementation of a port for a provider or a system API |

## Crates

```text
                    bloop-app
                  /     |     \
        bloop-macos bloop-audio bloop-providers
                  \     |     /
                    bloop-core
```

- `bloop-core` has no I/O, no async runtime, and no platform API. Its only external dependency is `secrecy`.
- `bloop-macos`, `bloop-audio`, and `bloop-providers` do not depend on each other.
- `bloop-app` builds the adapters from the settings and connects them to the driver.
- HTTP, WebSocket, and vendor-specific code stay in `bloop-providers`. One Cargo feature selects each vendor. `mistral` is the default.
- Only `bloop-macos` contains `unsafe` code. Its approved surfaces are in [AGENTS.md](../AGENTS.md). The other crates have `#![forbid(unsafe_code)]`.
- The AI ports return `BoxFuture`, so that the settings select the adapter at run time through trait objects.

### Source map

| Crate | Modules |
| --- | --- |
| `bloop-core` | `domain`: audio and text types; `session`: events, commands, and job order; `ports/ai` and `ports/platform`: the traits; `error`: typed failures; `pipeline`: ordered text steps; `steps/`: `LlmRefine`, `Typography`, `Trim`; `dictionary`: entries and written forms |
| `bloop-audio` | `capture`: the microphone stream; `device`: the input device choice; `buffer`: the bounded sample queue; `convert`: the conversion to 16 kHz |
| `bloop-providers` | `lib`: provider settings and constructors; `http`: the shared HTTP client; `mistral/`: settings, `transcribe` over WebSocket, and `chat` over HTTP |
| `bloop-macos` | `app`: the AppKit run loop; `events` and `scheduler`: the main queue; `keys`: the event tap; `sleep`: the sleep observer; `paste` and `paste/clipboard`: the target check and the clipboard transaction; `orb/`: the indicator and the card; `menu`: the AppKit menu renderer; `keychain`: the API keys; `permissions`: checks and System Settings links; `workspace`: file and folder opening, the frontmost application, and the application names |
| `bloop-app` | `main`: construction; `driver`: command execution and generation resources; `worker`: the streaming task and the text pipeline; `settings/`: schema, template, and edits; `files`: bounded reads and atomic writes; `hub`: refresh, snapshots, and adapter reuse; `menu`: the menu model and actions; `prompts`: built-in and user prompts, and the application lists; `dictionary`: file parsing; `logging`: the log file; `bin/eval/`: the evaluation tool |

Both binaries use `steps::correction` and the same `Pipeline`.

## Dictation flow

```text
Right Command down -> frontmost application -> settings snapshot -> hold timer
Hold threshold     -> microphone -> bounded queue -> converter -> provider stream
Right Command up   -> target snapshot -> stop microphone -> drain and finish stream
Complete transcript -> optional correction -> ordered insertion -> clipboard restore or fallback card
```

The capture starts after the hold threshold, so the user waits for the orb before speaking. Right Command combinations stay available to the applications.
Escape or other input during the capture cancels it. System sleep cancels the capture and all jobs. Recording has no maximum duration.

## Threads and ownership

| Context | Work |
| --- | --- |
| Main thread | AppKit, the event tap, the core, the driver, the hub, the menu, the orb, and the clipboard |
| Audio callback | Mono mixing, sample queue writes, and atomic level and counter updates |
| Tokio worker | Incremental conversion, network I/O, and correction |

- `main` gets `MainThreadMarker` one time and passes it to the modules that need it.
- The audio callback uses preallocated storage. It does not allocate, lock, log, or wait for network I/O (`bloop-audio/src/capture.rs`, `rtrb` queue in `buffer.rs`).
- One Tokio worker thread, named `bloop-worker`, runs the asynchronous tasks. The worker waits 20 ms between queue drains, and checks the queue failure at the same interval while a provider operation runs.
- The driver owns one transcription task for each generation. A dropped task aborts its work and releases its queue and connection (`Task::drop`; `worker/tests.rs`: `dropping_the_task_releases_the_session_during_send_and_finish`).
- A report task converts a panic of the work task into `CaptureFailed` with `AppError::Internal` (`worker/tests.rs`: `a_panic_in_the_provider_fails_the_capture`).
- Events return to the main thread through `MainSink`, a block on the main dispatch queue. If the handler is running, the event goes to the end of the queue, so the handler never runs inside itself (`bloop-macos/src/events.rs`).
- The hub and the menu source run on the main thread only. A hub borrow contains file I/O, parsing, and object construction, and never an AppKit call. A failed borrow is logged, never a panic (`hub/tests.rs`: `the_driver_gets_the_last_snapshot_when_the_hub_is_in_use`, `the_view_of_a_hub_in_use_is_none`).

## Core state machine and job order

The core has one capture state, `Idle`, `Pending`, or `Listening`, and a queue of at most three jobs. A job moves through `Processing`, `Ready`, and `Inserting`.
The core enforces the table below, and `bloop-core/src/session/tests.rs` checks each row.

| Event or condition | Behavior |
| --- | --- |
| Dictation key down while idle | Create one generation and start the hold timer. The driver takes the settings snapshot at `StartHoldTimer`. |
| Short press or other input during the hold | Return to idle without commands. Ignore the stale timer. |
| Hold threshold with three jobs | Do not start the capture. Show the error orb. |
| Hold threshold otherwise | Start the capture and show the listening orb. |
| Key release while listening | Record the target, stop the capture, create one processing job, and close the orb. |
| Escape or other input while listening | Cancel the capture and close the orb. Create no job. Escape does not cancel the jobs. |
| Complete transcript | Make the matching job ready. Insert only the first job of the queue. |
| Empty transcript | Remove the job (`CancelWork`). Paste nothing. |
| Jobs that complete in reverse order | Paste in capture order (`jobs_paste_in_session_order`; `driver/tests.rs`: `corrections_that_end_in_reverse_order_paste_once_each_in_session_order`). |
| Capture failure after release | Fail the matching processing job. |
| Job failure | Remove the job and unblock the next one. Show the error orb only when no capture is active. |
| `InsertDone` | End the first job only when it is inserting. A fallback shows the card, or defers it during a capture. |
| Deferred card | A newer card replaces it. A capture failure or system sleep drops it. |
| System sleep | Cancel the capture and all jobs. Their later results have no effect. |
| Unknown or stale generation | Ignore the event. |
| Copy last result | Copy the text of the latest insertion attempt, when one exists. |

The driver gives each generation its own settings snapshot at the key press. A settings change applies to later generations only (`driver/tests.rs`: `a_settings_change_after_the_key_press_applies_only_to_later_generations`).
A cancelled job gives no late text and no paste (`driver/tests.rs`: `a_cancelled_job_during_its_correction_gives_no_late_text_and_no_paste`).

## Audio and streaming

The microphone adapter uses the default input device. When the default input is a Bluetooth device, it uses the built-in microphone when one exists (`bloop-audio/src/device.rs`, tests in the module).
The device runs at its native rate. The callback mixes the channels to mono and pushes the samples to the bounded queue of the capture.

- Each generation owns one bounded queue and one transcription task (`driver.rs`).
- The queue holds 960,000 samples: 20 s at 48 kHz. Its capacity does not grow with the recording, because the worker drains it every 20 ms. A push that does not fit marks the queue as full and fails the dictation. Bloop never drops samples silently (`buffer.rs` tests: `a_push_that_does_not_fit_fails_the_queue_for_good`; `worker/tests.rs`: `a_full_queue_fails_the_capture_without_finishing`).
- System audio overruns are counted and logged at capture end. They do not stop the capture (`capture.rs`).
- The driver sets the end mark before it stops the microphone. A writer that disappears without the end mark is a `Closed` failure (`buffer.rs` tests: `the_queue_closes_when_the_writer_goes_without_the_end_mark`; `worker/tests.rs`: `a_lost_microphone_fails_the_capture_without_finishing`).
- The capture starts without waiting for the provider connection. The samples wait in the queue (`worker/tests.rs`: `a_slow_connection_keeps_the_audio_of_the_capture`).
- The worker converts the native rate to 16 kHz with an FFT resampler on 20 ms blocks. It removes the resampler delay, and the output does not depend on how the pushes divide the input (`convert.rs` tests).
- The worker encodes the samples as signed 16-bit little-endian PCM and sends complete chunks of 100 ms in capture order. At release, it flushes the converter and sends the remaining samples without waiting for a full chunk (`worker/tests.rs`: `the_capture_audio_goes_in_chunks_of_100_ms_and_the_end_sends_the_rest`, `a_48_khz_capture_reaches_the_session_at_16_khz_in_chunks_of_100_ms`).
- Normal completion drains the accepted samples, flushes the conversion, and calls `finish` one time. `finish` consumes the session and returns one complete transcript (`worker/tests.rs`: `a_capture_without_samples_finishes_once`).
- A provider failure during the capture fails the capture. A failure after the release fails the job (`worker/tests.rs`: `a_provider_failure_while_streaming_fails_the_capture`, `a_failed_finish_fails_the_job`).
- The connection, each send, and the finish have separate time limits. No limit applies to the recording duration (`worker/tests.rs`: `a_start_that_never_answers_fails_the_capture_after_the_connection_limit`, `the_inactivity_limit_applies_to_each_send_and_not_to_the_capture`, `a_finish_that_never_answers_fails_the_job_after_the_completion_limit`).
- An expired operation is dropped, which releases what it holds. A cancelled send discards the session, and the chunk is never retried.

The Mistral adapter (`bloop-providers/src/mistral/transcribe.rs`, tests in `transcribe/tests.rs`) opens one WebSocket connection for each dictation.
It applies the sample rate, the delay setting, the language, and the dictionary as the session settings before the first chunk. Then it sends each chunk as base64 PCM.

- Partial transcripts stay inside the adapter. Only the complete transcript of `transcription.done` reaches the pipeline (`deltas_during_audio_and_before_done_are_ignored`).
- Before each audio send, the adapter reads at most 64 pending provider events, so that a flood of events cannot block the send (`send_audio_reads_at_most_the_drain_limit_and_the_next_call_reads_the_other_events`).
- A complete transcript does not wait for a successful WebSocket close.
- Dropping the session, the pending connection, or the pending finish closes the connection (`dropping_the_session_during_audio_closes_the_connection` and the two other `dropping_*` tests).
- An HTTP refusal of the handshake maps 401 and 403 to `Unauthorized` and the other statuses to `Http(status)`. An `error` event or an unexpected event is `InvalidResponse`. A lost connection is `Network`.
- Bloop does not reconnect, replay audio, or upload a recording after a streaming failure. It stores no audio file.

## Correction pipeline

The pipeline (`bloop-core/src/pipeline.rs`) applies the text steps of a generation in order. Each step returns an applied text, a skip reason, or a typed error.
A failed or skipped step preserves its input for the next step. An empty transcript runs no step and gives `NothingHeard` (`pipeline.rs` tests).

With the correction off, the step list is empty and the raw transcript is the final text. With the correction on, `steps::correction` gives:

1. `LlmRefine` sends the prompt as the system message and the transcript between a `<transcript>` line and a `</transcript>` line as the user message. It rejects a reply that is not `Complete` or that contains only whitespace. The reply limit is `ceil(chars × 16 / 25) + 32` tokens, clamped to 64 to 2,048 (`steps/refine.rs` tests).
2. `Typography` replaces a no-break space before `?`, `!`, `:`, `;`, or `»`, or after `«`, with a normal space, and replaces `’` with `'` (`steps/typography.rs` tests).
3. `Trim` removes `\n` and `\r` at the start and the end. It keeps the spaces (`steps/trim.rs` tests).

- A failed correction therefore keeps the transcript, and typography and trim still apply (`pipeline.rs` tests: `a_failed_correction_keeps_the_raw_transcript_and_the_next_steps_run`; `worker/tests.rs`: `a_failed_correction_keeps_the_raw_transcript`).
- The minimum word count and the character ratio of `LlmRefine` are off in the application and in `eval` (`steps/mod.rs` tests).
- Each correction request has a limit of 8 s, including its response body (`CORRECTION_LIMIT` in `main.rs`, applied by the HTTP client; `mistral/chat.rs` tests: `a_provider_slower_than_the_limit_gives_timeout`, `a_body_that_stalls_after_the_headers_gives_timeout`).
- The chat adapter sends `model`, the two messages, `max_tokens`, `temperature` 0, `reasoning_effort`, and `stream: false`. It maps `stop` to `Complete`, `length` and `model_length` to `Truncated`, and other reasons to `Other`.
- Without a chat model, for example without an API key, a job with the correction on keeps the raw transcript (`worker/tests.rs`: `without_a_chat_model_a_job_with_the_correction_on_keeps_the_raw_transcript`).
- A missing or invalid prompt with the correction on fails each capture that uses it with `InvalidSettings` before the microphone task (`worker/tests.rs`: `with_an_invalid_prompt_and_the_correction_on_the_capture_fails_before_its_task`). With the correction off, the hub does not read a prompt (`hub/tests.rs`: `with_the_correction_off_the_prompt_is_not_read`, `with_the_correction_off_a_routed_app_reads_no_prompt_and_the_view_shows_no_error`).

The built-in prompts are `natural-light` and `code`, included from `prompts/`. A file of the user prompts folder with the same name replaces a built-in prompt (`prompts.rs` tests).

## Insertion and the clipboard transaction

At key release, the paste adapter (`bloop-macos/src/paste.rs`) records the frontmost process and the counters of key down, left, right, and other mouse down events. The counters count hardware events only, so the paste commands of Bloop do not change them.
Each generation keeps its target until its job ends (`paste/tests.rs`: `take_returns_the_target_of_the_generation`, `repeated_jobs_without_a_paste_keep_no_target`).

A paste runs one transaction with the phases `Reading`, `Restoring`, and `Done`:

1. End the previous transaction and restore its saved clipboard, so that the new save takes the content of the user, not the item of Bloop (`a_new_paste_after_a_cancelled_read_restores_the_original_clipboard`).
2. Save all clipboard items with the data of all their types. The save comes before the target check, because promised clipboard data can wait for another application.
3. Run the target check: no secure input, the same frontmost process, and unchanged input counters. A failed check copies the text as a normal clipboard item and reports `Fallback(TargetChanged)` or `Fallback(SecureInput)`. Bloop then writes no item and posts no paste command.
4. Write one pasteboard item whose string is supplied on demand by a data provider, with the `org.nspasteboard.TransientType` and `org.nspasteboard.ConcealedType` markers. Then post one Command-V key down and key up pair, key code 9, with the Command flag only. The events carry a marker, and the event tap ignores them (`only_marked_events_are_posted_by_bloop`).
5. When the target reads the item, the provider supplies the text and starts the restore delay. 300 ms later, if the clipboard change count is still the one that Bloop wrote, Bloop restores the saved items with the transient marker and reports `Landed` (`a_read_reports_landed_only_after_the_restore`, `restore_writes_the_saved_items_back_with_the_transient_marker`).
6. Without a read within 1 s, Bloop replaces its item with a normal text item, if it still owns the clipboard, and reports `Fallback(NotRead)` (`without_a_read_bloop_gives_up_once_and_restores_nothing`).

Invariants, enforced by `Paster`, `TextProvider`, and `paste/clipboard.rs`:

- Each generation posts at most one Command-V pair. A posted paste command cannot be withdrawn.
- `Landed` confirms a read of the clipboard item. It does not prove an insertion into the intended field. Any application that reads the item counts as the read.
- Bloop restores or rewrites the clipboard only while its change count still owns it (`restore_keeps_a_clipboard_that_changed_after_the_paste`). A deferred fallback never overwrites a newer clipboard value.
- Without a clipboard snapshot, Bloop leaves the text as a normal clipboard item (`finalize_without_a_saved_clipboard_leaves_the_text_as_a_normal_item`).
- Cancellation suppresses the result event. The read and the restore still follow their steps (`a_cancel_keeps_the_restore_and_drops_the_result`).
- Abandoning a transaction restores one time and stops its timers (`abandoning_a_transaction_restores_once_and_stops_its_timers`).
- The snapshot is limited to 64 items and 64 MiB. Above a limit, or when macOS cannot list the items, no snapshot exists and automatic restoration is unavailable. A type that cannot be read is left out, and the other types are restored (`save_within_*` tests).
- The latest result stays in memory for **Copy last result** until a newer result replaces it or the process exits (core `last_result`; `session/tests.rs`: `copy_last_result_copies_the_last_inserted_text`).

The target check does not inspect the focused field. The paste uses key code 9, the V key on the QWERTY and AZERTY layouts.
Forced termination or a crash cannot guarantee the restoration.

## Orb

The orb (`bloop-macos/src/orb/`) is a borderless, nonactivating `NSPanel` at the bottom center of the screen that contains the mouse pointer, 16 points above the visible frame. It never takes the focus.
The listening view shows the icon of the frontmost application and seven bars that follow the level meter every 33 ms. The error view shows red bars for 1 s. The card shows the first 240 characters of the final text, with `…` when the text is longer, for 8 s.
Each view change starts a new timer epoch, so the timers of a previous view have no effect (`orb/mod.rs` tests).

## Settings hub and menu

The hub (`bloop-app/src/hub.rs`) owns the parsed settings, the file texts, the current snapshot, and the chat model. It runs on the main thread.

- At each dictation key press and each menu open, the hub reads `config.toml` and `dictionary.toml` with bounded reads, and one prompt when the correction is on. Unchanged texts and an unchanged prompt reuse the current snapshot (`hub/tests.rs`: `an_unchanged_text_builds_no_new_snapshot`, `a_changed_config_builds_a_new_snapshot`).
- A prompt change reuses the chat model. A change of the `[chat]` table rebuilds it (`a_changed_prompt_file_builds_a_new_snapshot_with_the_same_chat_model`, `a_changed_chat_table_builds_a_new_chat_model`, `alternating_apps_reuse_the_chat_model`).
- At a key press, `HubSnapshots` reads the bundle identifier of the frontmost application through `NSWorkspace` before the hub borrow. Bloop itself counts as no application (`workspace.rs` tests: `only_the_process_identifier_of_bloop_is_its_own`). The prompt is the prompt whose list contains the application, else `[refine] prompt`. A menu open and a menu change read `[refine] prompt` (`hub/tests.rs`: `a_routed_app_uses_the_prompt_of_its_list`, `an_unknown_app_and_no_app_use_the_selected_prompt`, `the_driver_snapshot_uses_the_frontmost_app_and_a_view_does_not_change_it`). The log gives the prompt name, `route = "app"` or `route = "default"`, and the bundle identifier.
- The lists are the keys of `[refine.apps]`, then the built-in lists of `code` and `natural-light` for a prompt without a key (`prompts.rs`: `app_lists`). A key replaces the built-in list, even when it is empty. An application of a key leaves every built-in list. The same application in two keys makes the file invalid, so no application is in two lists (`prompts.rs` tests; `hub/tests.rs`: `a_key_replaces_the_built_in_list_and_an_empty_key_disables_it`, `an_app_of_a_key_wins_over_its_built_in_list`, `the_same_app_in_two_keys_gives_invalid_settings`).
- A missing or invalid prompt of a list fails the dictations in its applications only. With the correction on, the view reports it with its own name (`a_routed_missing_prompt_fails_its_app_only_and_the_view_names_it`, `the_view_names_each_routed_prompt_error_once_and_never_the_selected_prompt`).
- An invalid or unreadable settings or dictionary file gives a snapshot that fails each capture with `InvalidSettings` (`an_invalid_config_gives_invalid_settings_and_the_default_hold`, `an_invalid_dictionary_gives_invalid_settings_with_the_hold_of_the_file`).
- The API keys are read one time at start, outside the event tap, because a Keychain read can wait for a dialog. A changed key needs a restart (`the_keys_are_read_one_time_at_start`). Without a key, each dictation fails with `Unauthorized`. A refused key stays marked until the next start (`a_refused_key_stays_until_the_next_start`).
- Startup writes the settings template, the empty dictionary, and the prompts folder when they are missing. It never changes an existing file (`settings/mod.rs` tests: `a_later_start_keeps_the_existing_files_byte_for_byte`).

The event tap sees keys, modifier changes, and mouse down events only. A trackpad swipe between Spaces, or an application that activates itself during a capture, changes the frontmost application without cancelling the capture.
The prompt then comes from the application at the key press, and the paste target from the application at the key release. A browser uses the default prompt: the lists hold applications, not websites.

A menu change writes one key of `config.toml`, or the lists of `[refine.apps]` that an application change affects, in one replacement (`settings/edit.rs`, `files.rs`):

- The write parses the current file first and refuses an invalid file or a result larger than 64 KiB (`settings/mod.rs` tests: `the_write_does_not_change_an_invalid_file`, `a_change_that_makes_the_file_larger_than_the_limit_is_not_saved`).
- The change keeps the comments, the spacing, and the other keys. A missing table goes at the end. A comment after the last table moves below a table that the change adds (`settings/edit/tests.rs`).
- A new `[refine.apps]` table goes directly after `[refine]`. Without `[refine]`, its parent table is implicit, so the file gets no empty `[refine]` header (`a_new_apps_table_goes_directly_after_the_refine_table`, `a_new_apps_table_without_the_refine_table_has_no_empty_refine_header`).
- An application change computes the new lists from the current file, never from the lists of an older menu. An addition writes the list of its prompt and each list that loses the application, and a list without a key gets its key with the whole list. A removal writes the rest of the list. A change that changes no list keeps the text (`add_moves_the_app_from_its_list_and_writes_both_keys`, `add_to_a_list_without_a_key_writes_the_built_in_list_and_the_list_that_loses_the_app`, `remove_from_a_built_in_list_writes_the_rest_of_the_list`, `a_change_that_changes_no_list_keeps_the_text_byte_for_byte`).
- A replaced list keeps the comment above its key. The comments inside a list on several lines are lost (`remove_writes_the_rest_of_the_list_and_keeps_the_comment_above_the_key`, `the_comments_inside_a_multi_line_list_are_lost`).
- The write creates a temporary file in the destination folder, exclusively and with mode 0600, synchronizes and closes it, then renames it over the file. A failure removes the temporary file (`files.rs` tests: `overlapping_writers_cannot_change_a_file_that_another_writer_published`, `a_failed_rename_removes_its_temporary_file`).
- The replacement does not merge simultaneous edits. The last successful replacement wins.

The application builds the `MenuModel` (`bloop-app/src/menu.rs`), and `bloop-macos/src/menu.rs` renders it in AppKit:

- The menu refreshes at each open. The status lines come first: the missing permissions, the API key, the event tap, then the file errors with their line, the selected prompt error, and the error of each other prompt of a list (`menu/tests.rs`: `each_routed_prompt_error_follows_the_selected_prompt_error`).
- An action identifier is the index of the action in the last model. The renderer encodes it as the item tag, index plus one. Tag 0 is never an action. Unknown identifiers and invalid tags are logged and ignored (`bloop-macos/src/menu.rs` tests: `the_tag_is_the_action_plus_one_and_gives_the_action_back`, `the_tag_0_and_negative_tags_give_no_action`; `menu/tests.rs`: `an_unknown_action_has_no_effect`).
- The model is a tree: a submenu holds items, and a list of choices is a submenu of checked items. The renderer builds each submenu with the same tags and the same `menuAction:` selector. The delegate is on the top menu only.
- **Prompt by app** starts with a status line for the frontmost application: `<App> uses <prompt>` or `<App> uses the default prompt`. Then it has one submenu for each name of the **Prompt** list, and for each `[refine.apps]` key of another name with applications. A submenu lists the applications of the prompt with a checkmark, and a click removes one. **Add <App>** adds the frontmost application. It is absent when the list contains the application, when no application is in front, and for a key of another name. An empty submenu shows `No application` (`menu/tests.rs`: `prompt_by_app_lists_the_apps_of_each_prompt_with_a_checkmark`, `the_frontmost_app_gets_a_status_line_and_an_add_item_in_each_list_without_it`, `the_status_line_names_the_prompt_of_a_listed_frontmost_app_and_its_list_has_no_add_item`, `a_key_of_another_prompt_name_lists_its_apps_without_an_add_item`, `a_click_on_add_moves_the_frontmost_app_and_the_next_model_shows_it`).
- `AppMenu::model` reads the frontmost application and the application names through `NSWorkspace` after the hub borrow. A name is the file name of the installed application without `.app`, else the name in the built-in list, else the bundle identifier (`an_app_name_is_the_name_of_macos_then_the_built_in_name_then_the_bundle_identifier`; `workspace.rs` tests: `the_name_of_an_installed_application_is_its_file_name_without_app`). Two installed applications with the same file name show the same name.
- The renderer adds a separator and **Quit Bloop** with ⌘Q after the items. The menu source and the Objective-C callbacks never panic.

## Event tap

The event tap (`bloop-macos/src/keys.rs`) is an active session tap on key down, key up, flags changed, and mouse down events.

- Right Command alone, key code `0x36`, is the dictation key. Right Command pressed with Shift, Control, Option, or Left Command is not (`right_command_after_another_modifier_is_not_the_dictation_key`).
- Another key, modifier, or click during the hold is `OtherInputDuringHold`. The tap passes it to the applications.
- Escape during a capture is blocked completely: the key down, the repeats, and the key up. Outside a capture, Escape passes (`escape_is_blocked_only_during_a_capture`, `a_blocked_escape_press_stays_blocked_after_the_capture`).
- The tap ignores the paste commands that Bloop posts.
- When macOS disables the tap, the tap enables itself again, ends a hold as a combination, and resets its key state (`an_interruption_during_the_hold_ends_it_and_ignores_the_missed_release`).
- The callback catches a panic of the handler and passes the event.

## Privacy rules

- `Transcript`, `FinalText`, `AudioChunk`, `ChatRequest`, `ChatReply`, `StepOutcome`, `Prompt`, `Dictionary`, and the provider `Text` type redact their content in `Debug`. `TranscriptionOptions` shows the vocabulary count, not the entries (tests in `domain.rs`, `ports/ai.rs`, `prompts.rs`, `dictionary.rs`, and `transcribe/tests.rs`).
- API keys are `SecretString`. The `Authorization` header is marked sensitive, and the adapter `Debug` shows `REDACTED` (`chat.rs` tests: `the_debug_of_the_adapter_shows_no_key`). The key never enters settings, command arguments, logs, or error messages.
- Provider errors use local categories and HTTP status codes. The HTTP adapter does not read error bodies. The WebSocket adapter logs no error details, unknown event types, or finish strings (`chat.rs` tests: `an_http_error_body_never_enters_logs`, `an_unknown_finish_reason_is_logged_only_as_a_local_category`; `transcribe/tests.rs`: `provider_errors_and_unknown_event_types_never_enter_logs`).
- Logs record durations, counts, controlled error categories, the bundle identifier of the frontmost application at key press with the correction on, the bundle identifier of an application that a menu change adds or removes, the frontmost application at key release, and nonsecret settings such as the model identifiers. They never contain audio, text, prompt text, dictionary entries, or file content. Settings errors give a line, never the parser message.
- The panic hook logs a literal panic message. A formatted message, which can quote dictated text, gives only its length (`logging.rs` tests).
- The application installs no bridge from the `log` crate. WebSocket handshake traces can contain authorization headers.
- The `eval` tool writes its samples and results to the terminal by design.

## Time and size limits

These are implementation limits, not latency guarantees.

| Operation | Limit | Where |
| --- | --- | --- |
| Hold threshold | `hold_ms`, 100 to 1000 ms, 220 ms by default | `settings/schema.rs` |
| Concurrent jobs | 3 | `session.rs` |
| Sample queue | 960,000 samples | `driver.rs` |
| Worker poll interval | 20 ms | `worker.rs` |
| Audio chunk | 100 ms | `domain.rs` |
| Transcription connection | 5 s | `worker.rs` |
| Each audio send | 5 s | `worker.rs` |
| Transcription finish, after the queue drains | 15 s | `worker.rs` |
| Provider events read before a send | 64 | `mistral/transcribe.rs` |
| WebSocket frame and message | 1 MiB | `mistral/transcribe.rs` |
| Correction request, including the body | 8 s | `main.rs` |
| HTTP TCP connection | 5 s | `http.rs` |
| HTTP response body | 1 MiB | `http.rs` |
| Correction reply | 64 to 2,048 tokens | `steps/refine.rs` |
| Clipboard read | 1 s | `paste.rs` |
| Clipboard restore delay | 300 ms | `paste.rs` |
| Clipboard snapshot | 64 items and 64 MiB | `paste/clipboard.rs` |
| Error orb, card | 1 s, 8 s | `orb/mod.rs` |
| Card excerpt | 240 characters | `orb/layers.rs` |
| `config.toml`, `dictionary.toml`, prompt file | 64 KiB, 256 KiB, 32 KiB | `settings/mod.rs`, `dictionary.rs`, `prompts.rs` |
| Log rotation | 5 MiB at start | `logging.rs` |

There is no total deadline for the audio backlog after the release. Each send has its own deadline before `finish` starts, so a slow connection can delay the completion and the later insertions.

## Quit path

**Quit Bloop** sends `stop:` to `NSApplication`, and `App::run` returns. Then `main`:

1. Removes the event handler, which drops the driver and the components it owns. The drop of `Paster` abandons a pending clipboard transaction: it restores the saved clipboard, or leaves the text as a normal item without a snapshot, and stops its timers.
2. Shuts the Tokio runtime down in the background, so that a blocked name lookup does not delay the exit.

A quit before the target reads the paste item restores the saved clipboard. A paste command still pending in the target then pastes that content.
The sleep observer and the event tap are removed when their values drop.
