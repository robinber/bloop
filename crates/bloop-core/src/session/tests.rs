use super::{Command, Core, Event, FallbackReason, InsertOutcome, OrbView};
use crate::domain::{FinalText, Generation};
use crate::error::{AppError, ProviderError};

fn text(value: &str) -> FinalText {
    FinalText::new(value.to_owned())
}

fn press(core: &mut Core) -> Generation {
    let commands = core.handle(Event::DictationKeyDown);
    let [Command::StartHoldTimer(generation)] = commands.as_slice() else {
        panic!("expected one StartHoldTimer command, got {commands:?}");
    };
    *generation
}

fn start_listening(core: &mut Core) -> Generation {
    let generation = press(core);
    assert_eq!(
        core.handle(Event::HoldElapsed(generation)),
        vec![
            Command::StartCapture(generation),
            Command::ShowOrb(OrbView::Listening)
        ]
    );
    generation
}

fn start_job(core: &mut Core) -> Generation {
    let generation = start_listening(core);
    assert_eq!(
        core.handle(Event::DictationKeyUp),
        vec![
            Command::RecordTarget(generation),
            Command::StopCaptureAndProcess(generation),
            Command::CloseOrb
        ]
    );
    generation
}

#[test]
fn key_down_starts_the_hold_timer_and_activates_the_capture() {
    let mut core = Core::new();

    press(&mut core);

    assert!(core.capture_active());
}

#[test]
fn short_press_emits_nothing_and_ignores_the_late_timer() {
    let mut core = Core::new();
    let generation = press(&mut core);

    assert_eq!(core.handle(Event::DictationKeyUp), vec![]);
    assert_eq!(core.handle(Event::HoldElapsed(generation)), vec![]);
    assert!(!core.capture_active());
}

#[test]
fn hold_threshold_starts_the_capture_and_opens_the_orb() {
    let mut core = Core::new();

    start_listening(&mut core);

    assert!(core.capture_active());
}

#[test]
fn key_release_records_the_target_and_starts_a_job() {
    let mut core = Core::new();

    start_job(&mut core);

    assert!(!core.capture_active());
}

#[test]
fn combination_during_pending_cancels_without_commands() {
    let mut core = Core::new();
    let generation = press(&mut core);

    assert_eq!(core.handle(Event::OtherInputDuringHold), vec![]);
    assert_eq!(core.handle(Event::HoldElapsed(generation)), vec![]);
    assert!(!core.capture_active());
}

#[test]
fn combination_during_listening_cancels_the_capture_and_closes_the_orb() {
    let mut core = Core::new();
    start_listening(&mut core);

    assert_eq!(
        core.handle(Event::OtherInputDuringHold),
        vec![Command::CancelCapture, Command::CloseOrb]
    );
    assert_eq!(core.handle(Event::DictationKeyUp), vec![]);
}

#[test]
fn escape_cancels_the_capture_but_not_the_jobs() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    start_listening(&mut core);

    assert_eq!(
        core.handle(Event::Escape),
        vec![Command::CancelCapture, Command::CloseOrb]
    );
    assert_eq!(
        core.handle(Event::TextReady(job, text("bonjour"))),
        vec![Command::Insert(job, text("bonjour"))]
    );
}

#[test]
fn escape_without_capture_has_no_effect() {
    let mut core = Core::new();

    assert_eq!(core.handle(Event::Escape), vec![]);
    assert!(!core.capture_active());
}

#[test]
fn escape_and_sleep_during_pending_are_silent() {
    let mut core = Core::new();
    press(&mut core);
    assert_eq!(core.handle(Event::Escape), vec![]);
    press(&mut core);

    assert_eq!(core.handle(Event::SystemSleep), vec![]);
    assert!(!core.capture_active());
}

#[test]
fn key_down_during_a_capture_has_no_effect() {
    let mut core = Core::new();
    let generation = press(&mut core);
    assert_eq!(core.handle(Event::DictationKeyDown), vec![]);
    assert_eq!(
        core.handle(Event::HoldElapsed(generation)),
        vec![
            Command::StartCapture(generation),
            Command::ShowOrb(OrbView::Listening)
        ]
    );

    assert_eq!(core.handle(Event::DictationKeyDown), vec![]);
}

#[test]
fn stale_hold_timer_does_not_start_a_newer_capture() {
    let mut core = Core::new();
    let stale = press(&mut core);
    assert_eq!(core.handle(Event::DictationKeyUp), vec![]);
    let current = press(&mut core);

    assert_eq!(core.handle(Event::HoldElapsed(stale)), vec![]);
    assert_eq!(
        core.handle(Event::HoldElapsed(current)),
        vec![
            Command::StartCapture(current),
            Command::ShowOrb(OrbView::Listening)
        ]
    );
}

#[test]
fn second_hold_timer_during_listening_has_no_effect() {
    let mut core = Core::new();
    let generation = start_listening(&mut core);

    assert_eq!(core.handle(Event::HoldElapsed(generation)), vec![]);
    assert!(core.capture_active());
    assert_eq!(
        core.handle(Event::DictationKeyUp),
        vec![
            Command::RecordTarget(generation),
            Command::StopCaptureAndProcess(generation),
            Command::CloseOrb
        ]
    );
}

#[test]
fn stale_capture_events_do_not_touch_the_active_capture() {
    let mut core = Core::new();
    let stale = start_listening(&mut core);
    assert_eq!(
        core.handle(Event::Escape),
        vec![Command::CancelCapture, Command::CloseOrb]
    );
    let current = start_listening(&mut core);

    assert_eq!(
        core.handle(Event::CaptureFailed(stale, AppError::Microphone)),
        vec![]
    );
    assert_eq!(
        core.handle(Event::DictationKeyUp),
        vec![
            Command::RecordTarget(current),
            Command::StopCaptureAndProcess(current),
            Command::CloseOrb
        ]
    );
}

#[test]
fn each_session_gets_a_new_generation() {
    let mut core = Core::new();
    let first = start_job(&mut core);

    let second = press(&mut core);

    assert_ne!(first, second);
}

#[test]
fn landed_insertion_ends_the_job_without_orb_commands() {
    let mut core = Core::new();
    let job = start_job(&mut core);

    assert_eq!(
        core.handle(Event::TextReady(job, text("bonjour"))),
        vec![Command::Insert(job, text("bonjour"))]
    );
    assert_eq!(
        core.handle(Event::InsertDone(job, InsertOutcome::Landed)),
        vec![]
    );
    assert_eq!(
        core.handle(Event::InsertDone(job, InsertOutcome::Landed)),
        vec![]
    );
}

#[test]
fn jobs_paste_in_session_order() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);

    assert_eq!(core.handle(Event::TextReady(second, text("deux"))), vec![]);
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    assert_eq!(
        core.handle(Event::InsertDone(first, InsertOutcome::Landed)),
        vec![Command::Insert(second, text("deux"))]
    );
}

#[test]
fn full_job_queue_blocks_the_capture_at_the_hold_threshold() {
    let mut core = Core::new();
    for _ in 0..3 {
        start_job(&mut core);
    }
    let generation = press(&mut core);

    assert_eq!(
        core.handle(Event::HoldElapsed(generation)),
        vec![Command::ShowOrb(OrbView::Error)]
    );
    assert!(!core.capture_active());
}

#[test]
fn full_job_queue_still_lets_a_shortcut_pass_silently() {
    let mut core = Core::new();
    for _ in 0..3 {
        start_job(&mut core);
    }
    press(&mut core);

    assert_eq!(core.handle(Event::OtherInputDuringHold), vec![]);
}

#[test]
fn nothing_heard_removes_the_job_and_releases_the_next_one() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);
    assert_eq!(core.handle(Event::TextReady(second, text("deux"))), vec![]);

    assert_eq!(
        core.handle(Event::NothingHeard(first)),
        vec![
            Command::CancelWork(first),
            Command::Insert(second, text("deux"))
        ]
    );
}

#[test]
fn work_failure_shows_the_error_view_when_no_capture_is_active() {
    let mut core = Core::new();
    let job = start_job(&mut core);

    assert_eq!(
        core.handle(Event::WorkFailed(
            job,
            AppError::Provider(ProviderError::Timeout)
        )),
        vec![Command::CancelWork(job), Command::ShowOrb(OrbView::Error)]
    );
    assert_eq!(core.handle(Event::TextReady(job, text("tard"))), vec![]);
}

#[test]
fn work_failure_does_not_interrupt_an_active_capture() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    start_listening(&mut core);

    assert_eq!(
        core.handle(Event::WorkFailed(
            job,
            AppError::Provider(ProviderError::Network)
        )),
        vec![Command::CancelWork(job)]
    );
}

#[test]
fn capture_failure_cancels_the_capture_and_shows_the_error_view() {
    let mut core = Core::new();
    let generation = start_listening(&mut core);

    assert_eq!(
        core.handle(Event::CaptureFailed(generation, AppError::Microphone)),
        vec![Command::CancelCapture, Command::ShowOrb(OrbView::Error)]
    );
    assert!(!core.capture_active());
}

#[test]
fn capture_failure_after_release_ends_the_job() {
    let mut core = Core::new();
    let failed = start_job(&mut core);
    let next = start_job(&mut core);
    assert_eq!(core.handle(Event::TextReady(next, text("suite"))), vec![]);

    assert_eq!(
        core.handle(Event::CaptureFailed(failed, AppError::Microphone)),
        vec![
            Command::CancelWork(failed),
            Command::ShowOrb(OrbView::Error),
            Command::Insert(next, text("suite"))
        ]
    );
}

#[test]
fn late_failures_of_a_pasting_job_are_ignored() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    assert_eq!(core.handle(Event::TextReady(second, text("deux"))), vec![]);

    assert_eq!(
        core.handle(Event::WorkFailed(
            first,
            AppError::Provider(ProviderError::Network)
        )),
        vec![]
    );
    assert_eq!(core.handle(Event::NothingHeard(first)), vec![]);
    assert_eq!(
        core.handle(Event::CaptureFailed(first, AppError::Microphone)),
        vec![]
    );
    assert_eq!(
        core.handle(Event::InsertDone(first, InsertOutcome::Landed)),
        vec![Command::Insert(second, text("deux"))]
    );
}

#[test]
fn work_failure_of_a_ready_job_behind_the_head_keeps_the_queue() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);
    let third = start_job(&mut core);
    assert_eq!(core.handle(Event::TextReady(second, text("deux"))), vec![]);

    assert_eq!(
        core.handle(Event::WorkFailed(
            second,
            AppError::Provider(ProviderError::Network)
        )),
        vec![]
    );
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    assert_eq!(core.handle(Event::TextReady(third, text("trois"))), vec![]);
    assert_eq!(
        core.handle(Event::InsertDone(first, InsertOutcome::Landed)),
        vec![Command::Insert(second, text("deux"))]
    );
    assert_eq!(
        core.handle(Event::InsertDone(second, InsertOutcome::Landed)),
        vec![Command::Insert(third, text("trois"))]
    );
}

#[test]
fn fallback_shows_the_card_with_the_final_text() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(job, text("bonjour"))),
        vec![Command::Insert(job, text("bonjour"))]
    );

    assert_eq!(
        core.handle(Event::InsertDone(
            job,
            InsertOutcome::Fallback(FallbackReason::TargetChanged)
        )),
        vec![Command::ShowOrb(OrbView::Card(text("bonjour")))]
    );
}

#[test]
fn fallback_during_a_capture_shows_the_card_when_the_capture_ends() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(job, text("bonjour"))),
        vec![Command::Insert(job, text("bonjour"))]
    );
    let current = start_listening(&mut core);

    assert_eq!(
        core.handle(Event::InsertDone(
            job,
            InsertOutcome::Fallback(FallbackReason::NotRead)
        )),
        vec![]
    );
    assert_eq!(
        core.handle(Event::DictationKeyUp),
        vec![
            Command::RecordTarget(current),
            Command::StopCaptureAndProcess(current),
            Command::CloseOrb,
            Command::ShowOrb(OrbView::Card(text("bonjour")))
        ]
    );
}

#[test]
fn deferred_card_shows_after_a_short_press() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(job, text("bonjour"))),
        vec![Command::Insert(job, text("bonjour"))]
    );
    press(&mut core);

    assert_eq!(
        core.handle(Event::InsertDone(
            job,
            InsertOutcome::Fallback(FallbackReason::SecureInput)
        )),
        vec![]
    );
    assert_eq!(
        core.handle(Event::DictationKeyUp),
        vec![Command::ShowOrb(OrbView::Card(text("bonjour")))]
    );
}

#[test]
fn deferred_card_keeps_its_text_when_a_later_job_pastes() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    start_listening(&mut core);

    assert_eq!(
        core.handle(Event::InsertDone(
            first,
            InsertOutcome::Fallback(FallbackReason::TargetChanged)
        )),
        vec![]
    );
    assert_eq!(
        core.handle(Event::TextReady(second, text("deux"))),
        vec![Command::Insert(second, text("deux"))]
    );
    assert_eq!(
        core.handle(Event::Escape),
        vec![
            Command::CancelCapture,
            Command::CloseOrb,
            Command::ShowOrb(OrbView::Card(text("un")))
        ]
    );
    assert_eq!(
        core.handle(Event::CopyLastResult),
        vec![Command::CopyToClipboard(text("deux"))]
    );
}

#[test]
fn a_newer_fallback_card_replaces_the_kept_card() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    assert_eq!(core.handle(Event::TextReady(second, text("deux"))), vec![]);
    let current = start_listening(&mut core);
    assert_eq!(
        core.handle(Event::InsertDone(
            first,
            InsertOutcome::Fallback(FallbackReason::TargetChanged)
        )),
        vec![Command::Insert(second, text("deux"))]
    );

    assert_eq!(
        core.handle(Event::InsertDone(
            second,
            InsertOutcome::Fallback(FallbackReason::NotRead)
        )),
        vec![]
    );
    assert_eq!(
        core.handle(Event::DictationKeyUp),
        vec![
            Command::RecordTarget(current),
            Command::StopCaptureAndProcess(current),
            Command::CloseOrb,
            Command::ShowOrb(OrbView::Card(text("deux")))
        ]
    );
}

#[test]
fn capture_failure_drops_the_deferred_card() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(job, text("bonjour"))),
        vec![Command::Insert(job, text("bonjour"))]
    );
    let current = start_listening(&mut core);
    assert_eq!(
        core.handle(Event::InsertDone(
            job,
            InsertOutcome::Fallback(FallbackReason::TargetChanged)
        )),
        vec![]
    );

    assert_eq!(
        core.handle(Event::CaptureFailed(current, AppError::Microphone)),
        vec![Command::CancelCapture, Command::ShowOrb(OrbView::Error)]
    );
    assert_eq!(core.handle(Event::DictationKeyUp), vec![]);
    start_job(&mut core);
    assert_eq!(
        core.handle(Event::CopyLastResult),
        vec![Command::CopyToClipboard(text("bonjour"))]
    );
}

#[test]
fn system_sleep_drops_the_deferred_card() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(job, text("bonjour"))),
        vec![Command::Insert(job, text("bonjour"))]
    );
    start_listening(&mut core);
    assert_eq!(
        core.handle(Event::InsertDone(
            job,
            InsertOutcome::Fallback(FallbackReason::NotRead)
        )),
        vec![]
    );

    assert_eq!(
        core.handle(Event::SystemSleep),
        vec![Command::CancelCapture, Command::CloseOrb]
    );
    start_job(&mut core);
    assert_eq!(
        core.handle(Event::CopyLastResult),
        vec![Command::CopyToClipboard(text("bonjour"))]
    );
}

#[test]
fn copy_last_result_copies_the_last_inserted_text() {
    let mut core = Core::new();
    assert_eq!(core.handle(Event::CopyLastResult), vec![]);
    let first = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    assert_eq!(
        core.handle(Event::InsertDone(first, InsertOutcome::Landed)),
        vec![]
    );
    let second = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(second, text("deux"))),
        vec![Command::Insert(second, text("deux"))]
    );

    assert_eq!(
        core.handle(Event::CopyLastResult),
        vec![Command::CopyToClipboard(text("deux"))]
    );
}

#[test]
fn system_sleep_cancels_the_capture_and_all_jobs() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    start_listening(&mut core);

    assert_eq!(
        core.handle(Event::SystemSleep),
        vec![
            Command::CancelCapture,
            Command::CloseOrb,
            Command::CancelWork(first),
            Command::CancelWork(second)
        ]
    );
    assert_eq!(
        core.handle(Event::InsertDone(first, InsertOutcome::Landed)),
        vec![]
    );
    assert_eq!(core.handle(Event::TextReady(second, text("deux"))), vec![]);
    assert!(!core.capture_active());
    assert_eq!(
        core.handle(Event::CopyLastResult),
        vec![Command::CopyToClipboard(text("un"))]
    );
}

#[test]
fn system_sleep_without_a_capture_cancels_every_queued_job() {
    let mut core = Core::new();
    let first = start_job(&mut core);
    let second = start_job(&mut core);
    let third = start_job(&mut core);
    assert_eq!(
        core.handle(Event::TextReady(first, text("un"))),
        vec![Command::Insert(first, text("un"))]
    );
    assert_eq!(core.handle(Event::TextReady(second, text("deux"))), vec![]);

    assert_eq!(
        core.handle(Event::SystemSleep),
        vec![
            Command::CancelWork(first),
            Command::CancelWork(second),
            Command::CancelWork(third)
        ]
    );
    assert_eq!(
        core.handle(Event::InsertDone(
            first,
            InsertOutcome::Fallback(FallbackReason::NotRead)
        )),
        vec![]
    );
    assert_eq!(core.handle(Event::TextReady(third, text("trois"))), vec![]);
    start_listening(&mut core);
}

#[test]
fn events_for_an_unknown_generation_are_ignored() {
    let mut core = Core::new();
    let job = start_job(&mut core);
    let current = start_listening(&mut core);
    let unknown = Generation(42);

    for event in [
        Event::HoldElapsed(unknown),
        Event::CaptureFailed(unknown, AppError::Microphone),
        Event::TextReady(unknown, text("x")),
        Event::NothingHeard(unknown),
        Event::WorkFailed(unknown, AppError::Provider(ProviderError::Timeout)),
        Event::InsertDone(unknown, InsertOutcome::Landed),
    ] {
        assert_eq!(core.handle(event), vec![]);
    }
    assert_eq!(
        core.handle(Event::DictationKeyUp),
        vec![
            Command::RecordTarget(current),
            Command::StopCaptureAndProcess(current),
            Command::CloseOrb
        ]
    );
    assert_eq!(
        core.handle(Event::TextReady(job, text("ok"))),
        vec![Command::Insert(job, text("ok"))]
    );
}

#[test]
fn repeated_empty_jobs_end_each_job() {
    let mut core = Core::new();

    for _ in 0..10 {
        let job = start_job(&mut core);
        assert_eq!(
            core.handle(Event::NothingHeard(job)),
            vec![Command::CancelWork(job)]
        );
    }
}
