//! Text steps: the changes to the text after the transcription.

mod refine;
mod trim;
mod typography;

use std::sync::Arc;

pub(crate) use refine::LlmRefine;
use trim::Trim;
use typography::Typography;

use crate::ports::ai::{ChatModel, TextStep};

/// Returns the steps of the correction, in order: `LlmRefine` with `model` and `prompt`, `Typography`, and `Trim`.
#[must_use]
pub fn correction(model: Arc<dyn ChatModel>, prompt: String) -> Vec<Box<dyn TextStep>> {
    vec![
        Box::new(LlmRefine::new(model, prompt)),
        Box::new(Typography),
        Box::new(Trim),
    ]
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::sync::Arc;

    use super::correction;
    use crate::fakes::{FakeChat, resolve};
    use crate::ports::ai::{ChatModel, Finish, TextStep};

    const PROMPT: &str = "Corrige la ponctuation.";

    fn steps(model: &Arc<FakeChat>) -> Vec<Box<dyn TextStep>> {
        correction(Arc::clone(model) as Arc<dyn ChatModel>, PROMPT.to_owned())
    }

    #[test]
    fn the_correction_is_llm_refine_then_typography_then_trim() {
        let model = FakeChat::replying("", Finish::Complete);

        let names: Vec<_> = steps(&model).iter().map(|step| step.name()).collect();

        assert_eq!(names, ["llm_refine", "typography", "trim"]);
    }

    #[test]
    fn the_correction_sends_each_text_with_the_prompt() {
        let model = FakeChat::replying("À 3 heures.", Finish::Complete);
        let steps = steps(&model);

        for text in ["oui", "à 2 heures, non, 3 heures"] {
            let outcome = resolve(steps[0].apply(text));

            assert_matches!(
                outcome,
                Ok(applied) if applied == "À 3 heures.",
                "{text:?}"
            );
        }
        let sent = model.sent();
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|request| request.system == PROMPT));
    }
}
