//! AI provider ports. They return `BoxFuture`, so the settings can select a provider at run time.

use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;

use crate::domain::{AudioChunk, Transcript};
use crate::error::{ProviderError, StepError};

/// A future that can move between threads, as a trait object.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The settings that a transcription session applies before the first audio chunk.
///
/// `Debug` shows the vocabulary count, never the dictionary terms.
#[derive(Clone, Copy)]
pub struct TranscriptionOptions<'a> {
    /// The language of the speech, for example `fr`.
    pub language: &'a str,
    /// Words that the provider recognizes in preference to similar words.
    pub vocabulary: &'a [String],
    /// The sample rate of the audio chunks, in Hz.
    pub sample_rate: NonZeroU32,
}

impl fmt::Debug for TranscriptionOptions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranscriptionOptions")
            .field("language", &self.language)
            .field("vocabulary_entries", &self.vocabulary.len())
            .field("sample_rate", &self.sample_rate)
            .finish()
    }
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

/// One request to a chat model. It borrows its text.
///
/// `Debug` shows only the number of characters of each text, never the text.
pub struct ChatRequest<'a> {
    /// The instructions of the system message.
    pub system: &'a str,
    /// The user message.
    pub user: &'a str,
    /// The maximum number of tokens of the reply.
    pub max_tokens: NonZeroU32,
}

impl fmt::Debug for ChatRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatRequest")
            .field("system_chars", &self.system.chars().count())
            .field("user_chars", &self.user.chars().count())
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

/// How the reply of a chat model ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// The provider ended the reply normally (`stop` for Mistral).
    Complete,
    /// The reply stopped at the token limit.
    Truncated,
    /// The reply ended for another reason.
    Other,
}

/// The reply of a chat model.
///
/// `Debug` shows only the number of characters and the finish reason, never the text.
pub struct ChatReply {
    /// The text of the reply.
    pub text: String,
    /// How the reply ended. A step uses only a `Complete` reply.
    pub finish: Finish,
}

impl fmt::Debug for ChatReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatReply")
            .field("chars", &self.text.chars().count())
            .field("finish", &self.finish)
            .finish()
    }
}

/// A chat provider that a text step uses.
///
/// The adapter applies the time limit that it gets at construction.
/// An expired limit returns `ProviderError::Timeout`.
pub trait ChatModel: Send + Sync {
    /// Sends one request and waits for the complete reply.
    fn complete<'a>(
        &'a self,
        request: ChatRequest<'a>,
    ) -> BoxFuture<'a, Result<ChatReply, ProviderError>>;
}

/// The data of the dictation that the text steps can use.
#[derive(Debug, Clone)]
pub struct StepContext {
    /// The language of the dictation, for example `fr`.
    pub language: String,
}

/// One change to the text after the transcription, for example the correction.
///
/// A step writes no logs. It returns its outcome, and the pipeline adds it to the step report.
pub trait TextStep: Send + Sync {
    /// The name of the step in the step report.
    fn name(&self) -> &'static str;

    /// Applies the step to `text`, the text after the previous step, and returns the text that replaces it.
    fn apply<'a>(
        &'a self,
        text: &'a str,
        context: &'a StepContext,
    ) -> BoxFuture<'a, Result<String, StepError>>;
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::{ChatReply, ChatRequest, Finish, TranscriptionOptions};

    #[test]
    fn transcription_options_debug_hides_the_dictionary_terms() {
        let vocabulary = vec!["private name".to_owned(), "private term".to_owned()];
        let options = TranscriptionOptions {
            language: "fr",
            vocabulary: &vocabulary,
            sample_rate: NonZeroU32::new(16_000).unwrap(),
        };

        let debug = format!("{options:?}");

        assert_eq!(
            debug,
            "TranscriptionOptions { language: \"fr\", vocabulary_entries: 2, sample_rate: 16000 }"
        );
        assert!(!debug.contains("private"));
    }

    #[test]
    fn debug_shows_the_length_of_the_texts_but_not_the_texts() {
        let request = ChatRequest {
            system: "Corrige le texte.",
            user: "mot de passe secret",
            max_tokens: NonZeroU32::new(64).unwrap(),
        };
        let reply = ChatReply {
            text: "Mot de passe secret.".to_owned(),
            finish: Finish::Complete,
        };

        assert_eq!(
            format!("{request:?}"),
            "ChatRequest { system_chars: 17, user_chars: 19, max_tokens: 64 }"
        );
        assert_eq!(
            format!("{reply:?}"),
            "ChatReply { chars: 20, finish: Complete }"
        );
    }
}
