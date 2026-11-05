//! AI provider ports. They return `BoxFuture`, so the settings can select a provider at run time.

use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;

use crate::domain::{AudioChunk, Transcript};
use crate::error::ProviderError;

/// A future that can move between threads, as a trait object.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The settings that a transcription session applies before the first audio chunk.
#[derive(Debug, Clone, Copy)]
pub struct TranscriptionOptions<'a> {
    /// The language of the speech, for example `fr`.
    pub language: &'a str,
    /// Words that the provider recognizes in preference to similar words.
    pub vocabulary: &'a [String],
    /// The sample rate of the audio chunks, in Hz.
    pub sample_rate: NonZeroU32,
}

/// A speech provider that transcribes an audio stream during the capture.
pub trait Transcriber: Send + Sync {
    /// Opens a session and applies the options.
    ///
    /// The session owns its connection. It does not borrow the options or the provider.
    fn start<'a>(
        &'a self,
        options: TranscriptionOptions<'a>,
    ) -> BoxFuture<'a, Result<Box<dyn TranscriptionSession>, ProviderError>>;
}

/// One streaming transcription. Dropping the session cancels it and stops its tasks.
///
/// Partial results stay inside the session.
pub trait TranscriptionSession: Send + 'static {
    /// Sends the next chunk, in capture order.
    ///
    /// Success means that the chunk was sent, not that the provider transcribed it.
    fn send_audio<'a>(
        &'a mut self,
        chunk: &'a AudioChunk,
    ) -> BoxFuture<'a, Result<(), ProviderError>>;

    /// Ends the audio input and waits for the complete transcript.
    fn finish(self: Box<Self>) -> BoxFuture<'static, Result<Transcript, ProviderError>>;
}
