//! Fake speech provider and event sink for the worker and driver tests.

use std::future;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bloop_core::domain::{AudioChunk, Transcript};
use bloop_core::error::ProviderError;
use bloop_core::ports::ai::{BoxFuture, Transcriber, TranscriptionOptions, TranscriptionSession};
use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use tokio::runtime::Handle;
use tokio::time::sleep;

use super::{Limits, Spawner, Speech};

pub(crate) const TEXT: &str = "Bonjour à tous.";
pub(crate) const RATE: NonZeroU32 = NonZeroU32::new(16_000).unwrap();
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

/// What the fake provider and the event sink of a spawner receive.
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) log: Arc<Recorder<Call>>,
    pub(crate) rates: Arc<Recorder<NonZeroU32>>,
    pub(crate) events: Arc<Recorder<Event>>,
}

impl Fake {
    /// Returns a spawner on the current runtime whose sessions follow `script`.
    pub(crate) fn spawner(&self, script: Script, limits: Limits) -> Spawner {
        let speech = Speech {
            transcriber: Box::new(FakeTranscriber {
                script,
                log: Arc::clone(&self.log),
                rates: Arc::clone(&self.rates),
            }),
            language: "fr".to_owned(),
            vocabulary: Vec::new(),
            limits,
        };
        Spawner {
            runtime: Handle::current(),
            speech: Some(Arc::new(speech)),
            events: Arc::clone(&self.events) as Arc<dyn EventSink>,
        }
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
