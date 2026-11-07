//! Fake speech provider, chat model, and event sink for the worker and driver tests.

use std::cell::Cell;
use std::collections::VecDeque;
use std::future;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bloop_audio::convert;
use bloop_core::domain::{AudioChunk, Transcript};
use bloop_core::error::ProviderError;
use bloop_core::ports::ai::{
    BoxFuture, ChatModel, ChatReply, ChatRequest, Finish, Transcriber, TranscriptionOptions,
    TranscriptionSession,
};
use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use tokio::runtime::Handle;
use tokio::time::sleep;

use super::{Correction, Limits, Spawner, Speech};
use crate::prompts::NATURAL_LIGHT;

/// The transcript of each fake session. `Typography` changes its no-break space.
pub(crate) const TEXT: &str = "Bonjour à tous\u{a0}!";
/// The transcript after `Typography` and `Trim`: the text of a job whose correction failed.
pub(crate) const TYPESET: &str = "Bonjour à tous !";
/// The reply of the fake chat model when no answer is scheduled.
pub(crate) const CORRECTED: &str = "Bonjour à toutes !";
/// The rate of the test microphone: the rate of the provider, so that the worker sends the samples unchanged.
pub(crate) const RATE: NonZeroU32 = convert::RATE;
const HOUR: Duration = Duration::from_secs(3_600);
/// Limits that no test reaches, so that `Reply::Never` means no event within `settle`.
pub(crate) const LONG: Limits = Limits {
    connection: HOUR,
    inactivity: HOUR,
    completion: HOUR,
};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum Reply {
    #[default]
    Ok,
    After(Duration),
    Fail(ProviderError),
    Never,
    Panic,
}

impl Reply {
    async fn wait(self) -> Result<(), ProviderError> {
        match self {
            Self::Ok => Ok(()),
            Self::After(delay) => {
                sleep(delay).await;
                Ok(())
            }
            Self::Fail(error) => Err(error),
            Self::Never => future::pending().await,
            Self::Panic => panic!("the fake provider panicked"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Script {
    pub(crate) start: Reply,
    pub(crate) send: Reply,
    pub(crate) finish: Reply,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Call {
    Audio(Vec<u8>),
    Finish,
    Drop,
}

pub(crate) struct Recorder<T>(Mutex<Vec<T>>);

impl<T> Default for Recorder<T> {
    fn default() -> Self {
        Self(Mutex::new(Vec::new()))
    }
}

impl<T> Recorder<T> {
    fn push(&self, item: T) {
        self.0.lock().unwrap().push(item);
    }

    pub(crate) fn take(&self) -> Vec<T> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl EventSink for Recorder<Event> {
    fn send(&self, event: Event) {
        self.push(event);
    }
}

/// What the fake providers and the event sink of a spawner receive.
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) log: Arc<Recorder<Call>>,
    pub(crate) rates: Arc<Recorder<NonZeroU32>>,
    pub(crate) events: Arc<Recorder<Event>>,
    pub(crate) chat: Arc<FakeChat>,
}

impl Fake {
    /// Returns a spawner on the current runtime whose sessions follow `script`.
    /// Its correction uses the fake chat model, and its toggle is off.
    pub(crate) fn spawner(&self, script: Script, limits: Limits) -> Spawner {
        self.spawner_for(self.speech(script, limits))
    }

    /// Returns the speech of [`Fake::spawner`], for a test that changes it.
    pub(crate) fn speech(&self, script: Script, limits: Limits) -> Speech {
        Speech {
            transcriber: Box::new(FakeTranscriber {
                script,
                log: Arc::clone(&self.log),
                rates: Arc::clone(&self.rates),
            }),
            correction: Ok(Correction::new(
                Arc::clone(&self.chat) as Arc<dyn ChatModel>,
                NATURAL_LIGHT.to_prompt(),
            )),
            language: "fr".to_owned(),
            vocabulary: Vec::new(),
            limits,
        }
    }

    /// Returns a spawner on the current runtime for `speech`, with the correction toggle off.
    pub(crate) fn spawner_for(&self, speech: Speech) -> Spawner {
        Spawner {
            runtime: Handle::current(),
            speech: Ok(Arc::new(speech)),
            events: Arc::clone(&self.events) as Arc<dyn EventSink>,
            correction_on: Rc::new(Cell::new(false)),
        }
    }
}

/// One reply of the fake chat model: when it comes, its text, and how it ends.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Answer {
    pub(crate) reply: Reply,
    pub(crate) text: &'static str,
    pub(crate) finish: Finish,
}

impl Default for Answer {
    fn default() -> Self {
        Self {
            reply: Reply::Ok,
            text: CORRECTED,
            finish: Finish::Complete,
        }
    }
}

/// A request that the fake chat model received.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Sent {
    pub(crate) system: String,
    pub(crate) user: String,
}

/// A chat model that gives the scheduled answers in request order, then `CORRECTED` at once.
#[derive(Default)]
pub(crate) struct FakeChat {
    answers: Mutex<VecDeque<Answer>>,
    sent: Recorder<Sent>,
    open: AtomicUsize,
}

impl FakeChat {
    /// Schedules the answer to the next request that has no answer yet.
    pub(crate) fn answer(&self, answer: Answer) {
        self.answers.lock().unwrap().push_back(answer);
    }

    /// Returns the requests since the previous call, in order.
    pub(crate) fn sent(&self) -> Vec<Sent> {
        self.sent.take()
    }

    /// Returns the number of requests whose future still exists: not complete and not dropped.
    pub(crate) fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

impl ChatModel for FakeChat {
    fn complete<'a>(
        &'a self,
        request: ChatRequest<'a>,
    ) -> BoxFuture<'a, Result<ChatReply, ProviderError>> {
        self.sent.push(Sent {
            system: request.system.to_owned(),
            user: request.user.to_owned(),
        });
        let answer = self.answers.lock().unwrap().pop_front().unwrap_or_default();
        self.open.fetch_add(1, Ordering::SeqCst);
        let open = Open(&self.open);
        Box::pin(async move {
            let _open = open;
            answer.reply.wait().await?;
            Ok(ChatReply {
                text: answer.text.to_owned(),
                finish: answer.finish,
            })
        })
    }
}

/// Counts one open request until it is dropped.
struct Open<'a>(&'a AtomicUsize);

impl Drop for Open<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct FakeTranscriber {
    script: Script,
    log: Arc<Recorder<Call>>,
    rates: Arc<Recorder<NonZeroU32>>,
}

impl Transcriber for FakeTranscriber {
    fn start<'a>(
        &'a self,
        options: TranscriptionOptions<'a>,
    ) -> BoxFuture<'a, Result<Box<dyn TranscriptionSession>, ProviderError>> {
        self.rates.push(options.sample_rate);
        Box::pin(async move {
            self.script.start.wait().await?;
            let session = FakeSession {
                script: self.script,
                log: Arc::clone(&self.log),
            };
            Ok(Box::new(session) as Box<dyn TranscriptionSession>)
        })
    }
}

struct FakeSession {
    script: Script,
    log: Arc<Recorder<Call>>,
}

impl TranscriptionSession for FakeSession {
    fn send_audio<'a>(
        &'a mut self,
        chunk: &'a AudioChunk,
    ) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move {
            self.script.send.wait().await?;
            self.log.push(Call::Audio(chunk.as_bytes().to_vec()));
            Ok(())
        })
    }

    fn finish(self: Box<Self>) -> BoxFuture<'static, Result<Transcript, ProviderError>> {
        Box::pin(async move {
            self.log.push(Call::Finish);
            self.script.finish.wait().await?;
            Ok(Transcript::new(TEXT.to_owned()))
        })
    }
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.log.push(Call::Drop);
    }
}

pub(crate) fn samples(count: u16) -> Vec<f32> {
    (0..count).map(|index| f32::from(index) / 4_096.0).collect()
}

pub(crate) fn pcm(samples: &[f32]) -> Vec<u8> {
    AudioChunk::split(samples, RATE)
        .flat_map(|chunk| chunk.as_bytes().to_vec())
        .collect()
}

pub(crate) fn chunk_lengths(calls: &[Call]) -> Vec<usize> {
    calls
        .iter()
        .filter_map(|call| match call {
            Call::Audio(pcm) => Some(pcm.len()),
            Call::Finish | Call::Drop => None,
        })
        .collect()
}

pub(crate) fn sent_audio(calls: &[Call]) -> Vec<u8> {
    calls
        .iter()
        .filter_map(|call| match call {
            Call::Audio(pcm) => Some(pcm.as_slice()),
            Call::Finish | Call::Drop => None,
        })
        .flatten()
        .copied()
        .collect()
}

/// Runs the tasks until they end or wait for a reply that never comes.
/// The paused clock advances only when all tasks wait.
pub(crate) async fn settle() {
    sleep(Duration::from_secs(5)).await;
}
