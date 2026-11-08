use std::assert_matches;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use bloop_audio::buffer::{self, Writer};
use bloop_audio::convert;
use bloop_core::domain::Generation;
use bloop_core::error::{AppError, AudioQueueError, ProviderError};
use bloop_core::ports::ai::Finish;
use bloop_core::ports::platform::AudioSink;
use bloop_core::session::{Command, Core, Event};
use tokio::time::sleep;

use super::fakes::{
    Answer, CORRECTED, Call, Fake, LONG, RATE, Recorder, Reply, Script, Sent, TEXT, TYPESET,
    chunk_lengths, pcm, samples, sent_audio, settle,
};
use super::{Limits, NoCorrection, Speech, Task};
use crate::prompts::NATURAL_LIGHT;

const CAPACITY: usize = 16_000;

struct Run {
    generation: Generation,
    task: Option<Task>,
    writer: Option<Writer>,
    log: Arc<Recorder<Call>>,
    rates: Arc<Recorder<NonZeroU32>>,
    events: Arc<Recorder<Event>>,
}

impl Run {
    fn start(script: Script, capacity: usize, limits: Limits) -> Self {
        Self::start_at(RATE, script, capacity, limits)
    }

    fn start_at(rate: NonZeroU32, script: Script, capacity: usize, limits: Limits) -> Self {
        let (writer, reader) = buffer::new(capacity);
        let fake = Fake::default();
        let generation = generation();
        let speech = Ok(Arc::new(fake.speech(script, limits)));
        let task = fake
            .spawner()
            .spawn(generation, rate, reader, &speech)
            .unwrap();
        let Fake {
            log, rates, events, ..
        } = fake;
        Self {
            generation,
            task: Some(task),
            writer: Some(writer),
            log,
            rates,
            events,
        }
    }

    fn push(&mut self, samples: &[f32]) -> Result<(), AudioQueueError> {
        self.writer.as_mut().expect("a writer").try_push(samples)
    }

    fn end_capture(&mut self) {
        self.task.as_ref().expect("a task").end_capture();
        self.writer = None;
    }

    fn only_event(&self) -> Event {
        let [event] = <[_; 1]>::try_from(self.events.take()).expect("one event");
        event
    }
}

fn generation() -> Generation {
    let commands = Core::new().handle(Event::DictationKeyDown);
    let [Command::StartHoldTimer(generation)] = commands.as_slice() else {
        panic!("{commands:?}");
    };
    *generation
}

/// Runs one capture of `speech` without samples and returns its only event.
async fn job(fake: &Fake, speech: Speech) -> Event {
    let (writer, reader) = buffer::new(CAPACITY);
    let task = fake
        .spawner()
        .spawn(generation(), RATE, reader, &Ok(Arc::new(speech)))
        .unwrap();
    task.end_capture();
    drop(writer);
    settle().await;
    let [event] = <[_; 1]>::try_from(fake.events.take()).expect("one event");
    event
}

#[tokio::test(start_paused = true)]
async fn with_the_correction_off_the_job_keeps_the_raw_transcript_without_a_chat_request() {
    let fake = Fake::default();

    let event = job(&fake, fake.speech(Script::default(), LONG)).await;

    assert_matches!(event, Event::TextReady(_, text) if text.as_str() == TEXT);
    assert_eq!(fake.chat.sent(), [] as [Sent; 0]);
}

#[tokio::test(start_paused = true)]
async fn with_the_correction_on_the_job_gives_the_corrected_text_after_typography_and_trim() {
    let fake = Fake::default();
    fake.chat.answer(Answer {
        text: "\nBonjour à toutes\u{a0}!\n",
        ..Answer::default()
    });

    let event = job(&fake, fake.corrected_speech(Script::default(), LONG)).await;

    assert_matches!(event, Event::TextReady(_, text) if text.as_str() == CORRECTED);
    assert_eq!(
        fake.chat.sent(),
        [Sent {
            system: NATURAL_LIGHT.text.to_owned(),
            user: format!("<transcript>\n{TEXT}\n</transcript>"),
        }]
    );
    assert_eq!(fake.chat.open(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_failed_correction_keeps_the_raw_transcript() {
    let failures = [
        Answer {
            reply: Reply::Fail(ProviderError::Timeout),
            ..Answer::default()
        },
        Answer {
            reply: Reply::Fail(ProviderError::Unauthorized),
            ..Answer::default()
        },
        Answer {
            text: "Bonjour",
            finish: Finish::Truncated,
            ..Answer::default()
        },
    ];
    for answer in failures {
        let fake = Fake::default();
        fake.chat.answer(answer);

        let event = job(&fake, fake.corrected_speech(Script::default(), LONG)).await;

        assert_matches!(
            event,
            Event::TextReady(_, text) if text.as_str() == TYPESET,
            "{answer:?}"
        );
        assert_eq!(fake.chat.sent().len(), 1, "{answer:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn without_a_chat_model_a_job_with_the_correction_on_keeps_the_raw_transcript() {
    let fake = Fake::default();
    let mut speech = fake.speech(Script::default(), LONG);
    speech.correction = Err(NoCorrection::ChatUnavailable);

    let event = job(&fake, speech).await;

    assert_matches!(event, Event::TextReady(_, text) if text.as_str() == TEXT);
    assert_eq!(fake.chat.sent(), [] as [Sent; 0]);
}

#[tokio::test(start_paused = true)]
async fn with_an_invalid_prompt_and_the_correction_on_the_capture_fails_before_its_task() {
    let fake = Fake::default();
    let mut speech = fake.speech(Script::default(), LONG);
    speech.correction = Err(NoCorrection::InvalidPrompt);
    let (_writer, reader) = buffer::new(CAPACITY);

    let result = fake
        .spawner()
        .spawn(generation(), RATE, reader, &Ok(Arc::new(speech)));

    assert_eq!(result.err(), Some(AppError::InvalidSettings));
    settle().await;
    assert_eq!(fake.rates.take(), [] as [NonZeroU32; 0]);
    assert!(fake.events.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_speech_error_of_the_snapshot_fails_the_capture_before_its_task() {
    for error in [
        AppError::Provider(ProviderError::Unauthorized),
        AppError::InvalidSettings,
    ] {
        let fake = Fake::default();
        let (_writer, reader) = buffer::new(CAPACITY);

        let result = fake
            .spawner()
            .spawn(generation(), RATE, reader, &Err(error));

        assert_eq!(result.err(), Some(error));
        settle().await;
        assert_eq!(fake.rates.take(), [] as [NonZeroU32; 0], "{error:?}");
        assert!(fake.events.take().is_empty(), "{error:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn audio_reaches_the_provider_in_order_and_the_session_finishes_after_it() {
    let mut run = Run::start(Script::default(), CAPACITY, LONG);
    let audio = samples(4_000);

    run.push(&audio[..1_600]).unwrap();
    settle().await;
    let calls = run.log.take();
    assert_eq!(sent_audio(&calls), pcm(&audio[..1_600]));
    assert!(!calls.contains(&Call::Finish));
    assert!(run.events.take().is_empty());
    run.push(&audio[1_600..]).unwrap();
    run.end_capture();
    settle().await;

    let calls = run.log.take();
    assert_eq!(sent_audio(&calls), pcm(&audio[1_600..]));
    assert_matches!(
        calls.as_slice(),
        [.., Call::Audio(_), Call::Finish, Call::Drop]
    );
    assert_matches!(
        run.only_event(),
        Event::TextReady(generation, text) if generation == run.generation && text.as_str() == TEXT
    );
}

#[tokio::test(start_paused = true)]
async fn the_capture_audio_goes_in_chunks_of_100_ms_and_the_end_sends_the_rest() {
    let mut run = Run::start(Script::default(), CAPACITY, LONG);
    let audio = samples(4_900);

    for part in audio.chunks(700) {
        run.push(part).unwrap();
        settle().await;
    }
    run.end_capture();
    settle().await;

    let calls = run.log.take();
    assert_eq!(chunk_lengths(&calls), [3_200, 3_200, 3_200, 200]);
    assert_eq!(sent_audio(&calls), pcm(&audio));
}

#[tokio::test(start_paused = true)]
async fn a_48_khz_capture_reaches_the_session_at_16_khz_in_chunks_of_100_ms() {
    let microphone = NonZeroU32::new(48_000).unwrap();
    let mut run = Run::start_at(microphone, Script::default(), 48_000, LONG);

    run.push(&vec![0.1; 24_000]).unwrap();
    settle().await;
    let during = chunk_lengths(&run.log.take());
    run.end_capture();
    settle().await;
    let after = chunk_lengths(&run.log.take());

    assert_eq!(run.rates.take(), [convert::RATE]);
    assert!(
        !during.is_empty() && during.iter().all(|&bytes| bytes == 3_200),
        "{during:?}"
    );
    assert_eq!([during, after].concat(), [3_200; 5]);
}

#[tokio::test(start_paused = true)]
async fn a_slow_connection_keeps_the_audio_of_the_capture() {
    let script = Script {
        start: Reply::After(Duration::from_secs(2)),
        ..Script::default()
    };
    let mut run = Run::start(script, CAPACITY, LONG);
    let audio = samples(3_000);

    run.push(&audio).unwrap();
    run.end_capture();
    settle().await;

    let calls = run.log.take();
    assert_eq!(sent_audio(&calls), pcm(&audio));
    assert_matches!(calls.as_slice(), [.., Call::Finish, Call::Drop]);
    assert_matches!(run.only_event(), Event::TextReady(..));
}

#[tokio::test(start_paused = true)]
async fn a_full_queue_fails_the_capture_without_finishing() {
    let mut run = Run::start(Script::default(), 8, LONG);

    run.push(&samples(4)).unwrap();
    assert_eq!(run.push(&samples(8)), Err(AudioQueueError::Full));
    settle().await;

    assert_eq!(run.log.take(), [Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Full))
    );
}

#[tokio::test(start_paused = true)]
async fn a_full_queue_fails_the_capture_during_a_slow_connection() {
    let script = Script {
        start: Reply::Never,
        ..Script::default()
    };
    let mut run = Run::start(script, 8, LONG);

    run.push(&samples(4)).unwrap();
    assert_eq!(run.push(&samples(8)), Err(AudioQueueError::Full));
    settle().await;

    assert_eq!(run.log.take(), [] as [Call; 0]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Full))
    );
}

#[tokio::test(start_paused = true)]
async fn an_overflow_after_the_end_mark_fails_the_capture() {
    let mut run = Run::start(Script::default(), 8, LONG);

    run.push(&samples(4)).unwrap();
    run.task.as_ref().expect("a task").end_capture();
    assert_eq!(run.push(&samples(8)), Err(AudioQueueError::Full));
    run.writer = None;
    settle().await;

    assert_eq!(run.log.take(), [Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Full))
    );
}

#[tokio::test(start_paused = true)]
async fn a_lost_microphone_fails_the_capture_without_finishing() {
    let mut run = Run::start(Script::default(), CAPACITY, LONG);

    run.push(&samples(1_600)).unwrap();
    settle().await;
    run.writer = None;
    settle().await;

    assert_matches!(run.log.take().as_slice(), [Call::Audio(_), Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Closed))
    );
}

#[tokio::test(start_paused = true)]
async fn a_lost_microphone_fails_the_capture_during_a_blocked_send() {
    let script = Script {
        send: Reply::Never,
        ..Script::default()
    };
    let mut run = Run::start(script, CAPACITY, LONG);

    run.push(&samples(1_600)).unwrap();
    settle().await;
    run.writer = None;
    settle().await;

    assert_eq!(run.log.take(), [Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(_, AppError::AudioQueue(AudioQueueError::Closed))
    );
}

#[tokio::test(start_paused = true)]
async fn a_provider_failure_while_streaming_fails_the_capture() {
    let failures = [
        (
            Script {
                start: Reply::Fail(ProviderError::Unauthorized),
                ..Script::default()
            },
            vec![],
            ProviderError::Unauthorized,
        ),
        (
            Script {
                send: Reply::Fail(ProviderError::Network),
                ..Script::default()
            },
            vec![Call::Drop],
            ProviderError::Network,
        ),
    ];
    for (script, calls, expected) in failures {
        let mut run = Run::start(script, CAPACITY, LONG);

        run.push(&samples(1_600)).unwrap();
        settle().await;

        assert_eq!(run.log.take(), calls, "{script:?}");
        assert_matches!(
            run.only_event(),
            Event::CaptureFailed(_, AppError::Provider(error)) if error == expected,
            "{script:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_provider_failure_after_the_key_release_fails_the_capture() {
    let script = Script {
        send: Reply::Fail(ProviderError::Network),
        ..Script::default()
    };
    let mut run = Run::start(script, CAPACITY, LONG);

    run.push(&samples(100)).unwrap();
    run.end_capture();
    settle().await;

    assert_eq!(run.log.take(), [Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(_, AppError::Provider(ProviderError::Network))
    );
}

#[tokio::test(start_paused = true)]
async fn a_capture_without_samples_finishes_once() {
    let mut run = Run::start(Script::default(), CAPACITY, LONG);

    run.end_capture();
    settle().await;

    assert_eq!(run.log.take(), [Call::Finish, Call::Drop]);
    assert_matches!(run.only_event(), Event::TextReady(..));
}

#[tokio::test(start_paused = true)]
async fn a_failed_finish_fails_the_job() {
    let script = Script {
        finish: Reply::Fail(ProviderError::Timeout),
        ..Script::default()
    };
    let mut run = Run::start(script, CAPACITY, LONG);

    run.push(&samples(100)).unwrap();
    run.end_capture();
    settle().await;

    assert_matches!(
        run.log.take().as_slice(),
        [Call::Audio(_), Call::Finish, Call::Drop]
    );
    assert_matches!(
        run.only_event(),
        Event::WorkFailed(_, AppError::Provider(ProviderError::Timeout))
    );
}

#[tokio::test(start_paused = true)]
async fn a_panic_in_the_provider_fails_the_capture() {
    let script = Script {
        send: Reply::Panic,
        ..Script::default()
    };
    let mut run = Run::start(script, CAPACITY, LONG);

    run.push(&samples(1_600)).unwrap();
    settle().await;

    assert_eq!(run.log.take(), [Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(generation, AppError::Internal) if generation == run.generation
    );
}

#[tokio::test(start_paused = true)]
async fn dropping_the_task_releases_the_session_during_send_and_finish() {
    let waits = [
        Script {
            send: Reply::Never,
            ..Script::default()
        },
        Script {
            finish: Reply::Never,
            ..Script::default()
        },
    ];
    for script in waits {
        let mut run = Run::start(script, CAPACITY, LONG);

        run.push(&samples(100)).unwrap();
        run.end_capture();
        settle().await;
        assert!(!run.log.take().contains(&Call::Drop), "{script:?}");
        run.task = None;
        settle().await;

        assert_eq!(run.log.take(), [Call::Drop], "{script:?}");
        assert!(run.events.take().is_empty(), "{script:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_start_that_never_answers_fails_the_capture_after_the_connection_limit() {
    let script = Script {
        start: Reply::Never,
        ..Script::default()
    };
    let limits = Limits {
        connection: Duration::from_secs(2),
        ..LONG
    };
    let mut run = Run::start(script, CAPACITY, limits);

    run.push(&samples(100)).unwrap();
    sleep(Duration::from_millis(1_900)).await;
    assert_matches!(run.log.take().as_slice(), []);
    assert!(run.events.take().is_empty());
    sleep(Duration::from_millis(200)).await;

    assert_eq!(run.log.take(), [] as [Call; 0]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(generation, AppError::Provider(ProviderError::Timeout))
            if generation == run.generation
    );
}

#[tokio::test(start_paused = true)]
async fn a_send_that_never_answers_fails_the_capture_after_the_inactivity_limit() {
    let script = Script {
        send: Reply::Never,
        ..Script::default()
    };
    let limits = Limits {
        inactivity: Duration::from_secs(2),
        ..LONG
    };
    let mut run = Run::start(script, CAPACITY, limits);

    run.push(&samples(1_600)).unwrap();
    sleep(Duration::from_millis(1_900)).await;
    assert_matches!(run.log.take().as_slice(), []);
    assert!(run.events.take().is_empty());
    sleep(Duration::from_millis(200)).await;

    assert_eq!(run.log.take(), [Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::CaptureFailed(generation, AppError::Provider(ProviderError::Timeout))
            if generation == run.generation
    );
}

#[tokio::test(start_paused = true)]
async fn a_finish_that_never_answers_fails_the_job_after_the_completion_limit() {
    let script = Script {
        finish: Reply::Never,
        ..Script::default()
    };
    let limits = Limits {
        completion: Duration::from_secs(2),
        ..LONG
    };
    let mut run = Run::start(script, CAPACITY, limits);

    run.push(&samples(100)).unwrap();
    run.end_capture();
    sleep(Duration::from_millis(1_900)).await;
    assert_matches!(run.log.take().as_slice(), [Call::Audio(_), Call::Finish]);
    assert!(run.events.take().is_empty());
    sleep(Duration::from_millis(200)).await;

    assert_eq!(run.log.take(), [Call::Drop]);
    assert_matches!(
        run.only_event(),
        Event::WorkFailed(generation, AppError::Provider(ProviderError::Timeout))
            if generation == run.generation
    );
}

#[tokio::test(start_paused = true)]
async fn a_slow_provider_within_each_limit_still_produces_the_transcript() {
    let slow = Reply::After(Duration::from_millis(1_900));
    let script = Script {
        start: slow,
        send: slow,
        finish: slow,
    };
    let limit = Duration::from_secs(2);
    let limits = Limits {
        connection: limit,
        inactivity: limit,
        completion: limit,
    };
    let mut run = Run::start(script, CAPACITY, limits);
    let audio = samples(4_800);

    run.push(&audio).unwrap();
    run.end_capture();
    sleep(Duration::from_secs(10)).await;

    let calls = run.log.take();
    assert_eq!(sent_audio(&calls), pcm(&audio));
    assert_matches!(
        calls.as_slice(),
        [
            Call::Audio(_),
            Call::Audio(_),
            Call::Audio(_),
            Call::Finish,
            Call::Drop
        ]
    );
    assert_matches!(
        run.only_event(),
        Event::TextReady(generation, text) if generation == run.generation && text.as_str() == TEXT
    );
}

#[tokio::test(start_paused = true)]
async fn the_inactivity_limit_applies_to_each_send_and_not_to_the_capture() {
    let script = Script {
        send: Reply::After(Duration::from_millis(600)),
        ..Script::default()
    };
    let limits = Limits {
        inactivity: Duration::from_secs(1),
        ..LONG
    };
    let mut run = Run::start(script, CAPACITY, limits);
    let audio = samples(6_400);

    run.push(&audio).unwrap();
    settle().await;
    let calls = run.log.take();
    assert_eq!(sent_audio(&calls), pcm(&audio));
    assert_matches!(
        calls.as_slice(),
        [
            Call::Audio(_),
            Call::Audio(_),
            Call::Audio(_),
            Call::Audio(_)
        ]
    );
    assert!(run.events.take().is_empty());
    run.end_capture();
    settle().await;

    assert_eq!(run.log.take(), [Call::Finish, Call::Drop]);
    assert_matches!(run.only_event(), Event::TextReady(..));
}
