//! The correction: a chat model changes the text with a prompt.

use std::num::NonZeroU32;
use std::ops::RangeInclusive;
use std::sync::Arc;

use crate::error::{Rejection, StepError};
use crate::ports::ai::{
    BoxFuture, ChatModel, ChatReply, ChatRequest, Finish, SkipReason, StepContext, StepOutcome,
    TextStep,
};

const RESERVE: NonZeroU32 = NonZeroU32::new(32).unwrap();
const FLOOR: NonZeroU32 = NonZeroU32::new(64).unwrap();
const CEILING: NonZeroU32 = NonZeroU32::new(2_048).unwrap();

/// Sends the prompt and the text to a chat model, and replaces the text with the reply.
///
/// The system message contains the prompt. The user message contains the text
/// between a `<transcript>` line and a `</transcript>` line.
/// The step rejects a reply that is not `Complete`, and a reply that is empty or contains only whitespace.
///
/// The minimum word count and the character ratio are off by default.
pub struct LlmRefine {
    model: Arc<dyn ChatModel>,
    prompt: String,
    min_words: usize,
    ratio: Option<RangeInclusive<u16>>,
}

impl LlmRefine {
    /// Creates the step without a minimum word count and without a character ratio.
    #[must_use]
    pub fn new(model: Arc<dyn ChatModel>, prompt: String) -> Self {
        Self {
            model,
            prompt,
            min_words: 0,
            ratio: None,
        }
    }

    /// Skips a text with fewer than `words` words, without a request.
    /// The words are the parts between whitespace. The value 0 turns the skip off.
    #[must_use]
    pub fn with_min_words(mut self, words: usize) -> Self {
        self.min_words = words;
        self
    }

    /// Rejects a reply whose character count is outside `percent` of the character count of the text.
    /// For example, with `50..=160`, a reply to a text of 100 characters has from 50 to 160 characters.
    /// An empty range rejects each reply.
    #[must_use]
    pub fn with_ratio(mut self, percent: RangeInclusive<u16>) -> Self {
        self.ratio = Some(percent);
        self
    }

    fn check(&self, text: &str, reply: &ChatReply) -> Result<(), Rejection> {
        if reply.finish != Finish::Complete {
            return Err(Rejection::Incomplete);
        }
        if reply.text.trim().is_empty() {
            return Err(Rejection::Empty);
        }
        if let Some(percent) = &self.ratio {
            let text_chars = text.chars().count();
            let reply_chars = reply.text.chars().count().saturating_mul(100);
            if reply_chars < text_chars.saturating_mul(usize::from(*percent.start())) {
                return Err(Rejection::TooShort);
            }
            if reply_chars > text_chars.saturating_mul(usize::from(*percent.end())) {
                return Err(Rejection::TooLong);
            }
        }
        Ok(())
    }
}

impl TextStep for LlmRefine {
    fn name(&self) -> &'static str {
        "llm_refine"
    }

    fn apply<'a>(
        &'a self,
        text: &'a str,
        _context: &'a StepContext,
    ) -> BoxFuture<'a, Result<StepOutcome, StepError>> {
        Box::pin(async move {
            if text.split_whitespace().take(self.min_words).count() < self.min_words {
                return Ok(StepOutcome::Skipped(SkipReason::TooFewWords));
            }
            let user = format!("<transcript>\n{text}\n</transcript>");
            let request = ChatRequest {
                system: &self.prompt,
                user: &user,
                max_tokens: max_tokens(text.chars().count()),
            };
            let reply = self
                .model
                .complete(request)
                .await
                .map_err(StepError::Provider)?;
            self.check(text, &reply).map_err(StepError::Rejected)?;
            Ok(StepOutcome::Applied(reply.text))
        })
    }
}

/// Returns the reply limit for a text of `chars` characters: ceil(chars / 2.5 × 1.6) + 32, from 64 to 2,048.
/// The calculation saturates, because a longer text gets the maximum.
fn max_tokens(chars: usize) -> NonZeroU32 {
    let chars = u32::try_from(chars).unwrap_or(u32::MAX);
    RESERVE
        .saturating_add(chars.saturating_mul(16).div_ceil(25))
        .clamp(FLOOR, CEILING)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::sync::Arc;

    use super::{LlmRefine, max_tokens};
    use crate::error::{ProviderError, Rejection, StepError};
    use crate::fakes::{FakeChat, Sent, context, resolve};
    use crate::ports::ai::{ChatModel, Finish, SkipReason, StepOutcome, TextStep};

    const PROMPT: &str = "Corrige la ponctuation.";

    fn refine(model: &Arc<FakeChat>) -> LlmRefine {
        LlmRefine::new(Arc::clone(model) as Arc<dyn ChatModel>, PROMPT.to_owned())
    }

    #[test]
    fn the_request_holds_the_prompt_and_the_text_between_delimiters() {
        let model = FakeChat::replying("Bonjour à tous.", Finish::Complete);

        resolve(refine(&model).apply("bonjour à tous", &context())).unwrap();

        assert_eq!(
            model.sent(),
            [Sent {
                system: PROMPT.to_owned(),
                user: "<transcript>\nbonjour à tous\n</transcript>".to_owned(),
                max_tokens: 64,
            }]
        );
    }

    #[test]
    fn max_tokens_counts_the_characters_of_the_text_and_not_the_delimiters() {
        let model = FakeChat::replying("É.", Finish::Complete);

        resolve(refine(&model).apply(&"é".repeat(100), &context())).unwrap();

        let [sent] = <[_; 1]>::try_from(model.sent()).unwrap();
        assert_eq!(sent.max_tokens, 96);
    }

    #[test]
    fn max_tokens_follows_the_formula_between_its_limits() {
        for (chars, tokens) in [
            (0, 64),
            (50, 64),
            (51, 65),
            (100, 96),
            (1_000, 672),
            (3_150, 2_048),
            (3_151, 2_048),
            (usize::MAX, 2_048),
        ] {
            assert_eq!(max_tokens(chars).get(), tokens, "{chars} characters");
        }
    }

    #[test]
    fn a_complete_reply_replaces_the_text() {
        let model = FakeChat::replying("Bonjour à tous.", Finish::Complete);

        let outcome = resolve(refine(&model).apply("bonjour à tous", &context()));

        assert_matches!(outcome, Ok(StepOutcome::Applied(text)) if text == "Bonjour à tous.");
    }

    #[test]
    fn a_reply_that_is_not_complete_is_rejected() {
        for finish in [Finish::Truncated, Finish::Other] {
            let model = FakeChat::replying("Bonjour à", finish);

            let outcome = resolve(refine(&model).apply("bonjour à tous", &context()));

            assert_matches!(
                outcome,
                Err(StepError::Rejected(Rejection::Incomplete)),
                "{finish:?}"
            );
        }
    }

    #[test]
    fn an_empty_reply_is_rejected() {
        for reply in ["", " ", "\n\u{a0}\t"] {
            let model = FakeChat::replying(reply, Finish::Complete);

            let outcome = resolve(refine(&model).apply("bonjour à tous", &context()));

            assert_matches!(
                outcome,
                Err(StepError::Rejected(Rejection::Empty)),
                "{reply:?}"
            );
        }
    }

    #[test]
    fn an_empty_reply_is_rejected_with_a_ratio_that_accepts_it() {
        let model = FakeChat::replying(" ", Finish::Complete);
        let step = refine(&model).with_ratio(0..=1_000);

        let outcome = resolve(step.apply("bonjour à tous", &context()));

        assert_matches!(outcome, Err(StepError::Rejected(Rejection::Empty)));
    }

    #[test]
    fn a_provider_failure_fails_the_step() {
        for error in [ProviderError::Timeout, ProviderError::Unauthorized] {
            let model = FakeChat::failing(error);

            let outcome = resolve(refine(&model).apply("bonjour à tous", &context()));

            assert_matches!(
                outcome,
                Err(StepError::Provider(failure)) if failure == error
            );
        }
    }

    #[test]
    fn the_word_count_and_the_ratio_are_off_by_default() {
        let text = "à 2 heures, non, 3 heures";
        for reply in ["3", "à 3 heures", &"à 3 heures ".repeat(10)] {
            let model = FakeChat::replying(reply, Finish::Complete);

            let outcome = resolve(refine(&model).apply(text, &context()));

            assert_matches!(outcome, Ok(StepOutcome::Applied(applied)) if applied == reply);
        }
        let model = FakeChat::replying("Oui.", Finish::Complete);
        let outcome = resolve(refine(&model).apply("oui", &context()));
        assert_matches!(outcome, Ok(StepOutcome::Applied(_)));
        assert_eq!(model.sent().len(), 1);
    }

    #[test]
    fn a_text_below_the_minimum_word_count_is_skipped_without_a_request() {
        let model = FakeChat::replying("Texte.", Finish::Complete);
        let step = refine(&model).with_min_words(4);

        for text in ["", "à 3 heures", "trois\u{a0}mots\nici"] {
            let outcome = resolve(step.apply(text, &context()));

            assert_matches!(
                outcome,
                Ok(StepOutcome::Skipped(SkipReason::TooFewWords)),
                "{text:?}"
            );
        }
        assert_eq!(model.sent(), [] as [Sent; 0]);
        let outcome = resolve(step.apply("il est 3 heures", &context()));
        assert_matches!(outcome, Ok(StepOutcome::Applied(_)));
        assert_eq!(model.sent().len(), 1);
    }

    #[test]
    fn a_reply_outside_the_ratio_is_rejected() {
        let text = "0123456789";
        for (reply, expected) in [
            ("0123", Some(Rejection::TooShort)),
            ("01234", None),
            ("0123456789012345", None),
            ("01234567890123456", Some(Rejection::TooLong)),
        ] {
            let model = FakeChat::replying(reply, Finish::Complete);
            let step = refine(&model).with_ratio(50..=160);

            let outcome = resolve(step.apply(text, &context()));

            match expected {
                Some(rejection) => assert_matches!(
                    outcome,
                    Err(StepError::Rejected(reason)) if reason == rejection,
                    "{reply:?}"
                ),
                None => assert_matches!(
                    outcome,
                    Ok(StepOutcome::Applied(applied)) if applied == reply,
                    "{reply:?}"
                ),
            }
        }
    }

    #[test]
    fn the_ratio_counts_characters_and_not_bytes() {
        let model = FakeChat::replying(&"é".repeat(16), Finish::Complete);
        let step = refine(&model).with_ratio(50..=160);

        let outcome = resolve(step.apply("0123456789", &context()));

        assert_matches!(outcome, Ok(StepOutcome::Applied(_)));
    }

    #[test]
    fn the_step_name_is_llm_refine() {
        let model = FakeChat::replying("", Finish::Complete);

        assert_eq!(refine(&model).name(), "llm_refine");
    }
}
