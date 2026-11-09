//! The correction: a chat model changes the text with a prompt.

use std::num::NonZeroU32;
use std::sync::Arc;

use crate::error::{Rejection, StepError};
use crate::ports::ai::{BoxFuture, ChatModel, ChatReply, ChatRequest, Finish, TextStep};

const RESERVE: NonZeroU32 = NonZeroU32::new(32).unwrap();
const FLOOR: NonZeroU32 = NonZeroU32::new(64).unwrap();
const CEILING: NonZeroU32 = NonZeroU32::new(2_048).unwrap();

/// Sends the prompt and the text to a chat model, and replaces the text with the reply.
///
/// The system message contains the prompt. The user message contains the text
/// between a `<transcript>` line and a `</transcript>` line.
/// The step rejects a reply that is not `Complete`, and a reply that is empty or contains only whitespace.
pub(crate) struct LlmRefine {
    model: Arc<dyn ChatModel>,
    prompt: String,
}

impl LlmRefine {
    /// Creates the step with the chat model and the prompt.
    #[must_use]
    pub(crate) fn new(model: Arc<dyn ChatModel>, prompt: String) -> Self {
        Self { model, prompt }
    }
}

impl TextStep for LlmRefine {
    fn name(&self) -> &'static str {
        "llm_refine"
    }

    fn apply<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<String, StepError>> {
        Box::pin(async move {
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
            check(&reply).map_err(StepError::Rejected)?;
            Ok(reply.text)
        })
    }
}

fn check(reply: &ChatReply) -> Result<(), Rejection> {
    if reply.finish != Finish::Complete {
        return Err(Rejection::Incomplete);
    }
    if reply.text.trim().is_empty() {
        return Err(Rejection::Empty);
    }
    Ok(())
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
    use crate::fakes::{FakeChat, Sent, resolve};
    use crate::ports::ai::{ChatModel, Finish, TextStep};

    const PROMPT: &str = "Corrige la ponctuation.";

    fn refine(model: &Arc<FakeChat>) -> LlmRefine {
        LlmRefine::new(Arc::clone(model) as Arc<dyn ChatModel>, PROMPT.to_owned())
    }

    #[test]
    fn the_request_holds_the_prompt_and_the_text_between_delimiters() {
        let model = FakeChat::replying("Bonjour à tous.", Finish::Complete);

        resolve(refine(&model).apply("bonjour à tous")).unwrap();

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

        resolve(refine(&model).apply(&"é".repeat(100))).unwrap();

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

        let outcome = resolve(refine(&model).apply("bonjour à tous"));

        assert_matches!(outcome, Ok(text) if text == "Bonjour à tous.");
    }

    #[test]
    fn a_reply_that_is_not_complete_is_rejected() {
        for finish in [Finish::Truncated, Finish::Other] {
            let model = FakeChat::replying("Bonjour à", finish);

            let outcome = resolve(refine(&model).apply("bonjour à tous"));

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

            let outcome = resolve(refine(&model).apply("bonjour à tous"));

            assert_matches!(
                outcome,
                Err(StepError::Rejected(Rejection::Empty)),
                "{reply:?}"
            );
        }
    }

    #[test]
    fn a_provider_failure_fails_the_step() {
        for error in [ProviderError::Timeout, ProviderError::Unauthorized] {
            let model = FakeChat::failing(error);

            let outcome = resolve(refine(&model).apply("bonjour à tous"));

            assert_matches!(
                outcome,
                Err(StepError::Provider(failure)) if failure == error
            );
        }
    }

    #[test]
    fn a_reply_of_any_length_replaces_the_text_and_a_one_word_text_is_sent() {
        let text = "à 2 heures, non, 3 heures";
        for reply in ["3", "à 3 heures", &"à 3 heures ".repeat(10)] {
            let model = FakeChat::replying(reply, Finish::Complete);

            let outcome = resolve(refine(&model).apply(text));

            assert_matches!(outcome, Ok(applied) if applied == reply);
        }
        let model = FakeChat::replying("Oui.", Finish::Complete);
        let outcome = resolve(refine(&model).apply("oui"));
        assert_matches!(outcome, Ok(_));
        assert_eq!(model.sent().len(), 1);
    }

    #[test]
    fn the_step_name_is_llm_refine() {
        let model = FakeChat::replying("", Finish::Complete);

        assert_eq!(refine(&model).name(), "llm_refine");
    }
}
