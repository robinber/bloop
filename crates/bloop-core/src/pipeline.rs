//! Text pipeline: turns the complete transcript of a job into its result event and its step report.

use std::fmt;

use crate::domain::{FinalText, Generation, Transcript};
use crate::error::StepError;
use crate::ports::ai::TextStep;
use crate::session::Event;

/// The text steps of one generation, in order.
///
/// `Debug` shows only the names of the steps, never a prompt.
pub struct Pipeline {
    steps: Vec<Box<dyn TextStep>>,
}

impl fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&'static str> = self.steps.iter().map(|step| step.name()).collect();
        f.debug_struct("Pipeline").field("steps", &names).finish()
    }
}

/// The result of one text step, for the log.
#[derive(Debug)]
pub struct StepReport {
    /// The name of the step.
    pub step: &'static str,
    /// What the step did.
    pub result: StepResult,
}

/// What a text step did. It contains no text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    /// The text of the step replaced the previous text.
    Applied,
    /// The step failed. The previous text continued to the next step.
    Failed(StepError),
}

/// The result event of a job and the report of each step that ran.
#[derive(Debug)]
pub struct PipelineOutput {
    /// `TextReady` or `NothingHeard`.
    pub event: Event,
    /// One report for each step, in step order.
    pub report: Vec<StepReport>,
}

impl Pipeline {
    /// Keeps the steps in the given order. Without steps, the final text is the raw transcript.
    #[must_use]
    pub fn new(steps: Vec<Box<dyn TextStep>>) -> Self {
        Self { steps }
    }

    /// Applies the steps to the complete transcript of `generation`, and gives the result event.
    ///
    /// A transcript that contains only whitespace gives no text and an empty report.
    /// Otherwise each step gets the current text, and an applied step replaces it.
    /// After a failure, the text from before that step continues to the next step.
    /// The event is `TextReady` with the final text, or `NothingHeard` when the transcript contains only whitespace.
    pub async fn run(&self, generation: Generation, transcript: Transcript) -> PipelineOutput {
        if transcript.as_str().trim().is_empty() {
            return PipelineOutput {
                event: Event::NothingHeard(generation),
                report: Vec::new(),
            };
        }
        let mut text = FinalText::from(transcript);
        let mut report = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let result = match step.apply(text.as_str()).await {
                Ok(changed) => {
                    text = FinalText::new(changed);
                    StepResult::Applied
                }
                Err(error) => StepResult::Failed(error),
            };
            report.push(StepReport {
                step: step.name(),
                result,
            });
        }
        PipelineOutput {
            event: Event::TextReady(generation, text),
            report,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::sync::Arc;

    use super::{Pipeline, PipelineOutput, StepReport, StepResult};
    use crate::domain::{Generation, Transcript};
    use crate::error::{ProviderError, Rejection, StepError};
    use crate::fakes::{FakeChat, Sent, resolve};
    use crate::ports::ai::{ChatModel, Finish, TextStep};
    use crate::session::Event;
    use crate::steps::{self, LlmRefine};

    const PROMPT: &str = "Corrige la ponctuation.";
    const RAW: &str = "euh il est trois heures\u{a0}? c’est l’heure\n";
    const RAW_FINAL: &str = "euh il est trois heures ? c'est l'heure";
    const REPLY: &str = "\nIl est 3\u{a0}heures\u{a0}? C’est l’heure\u{202f}!\n";
    const REPLY_FINAL: &str = "Il est 3\u{a0}heures ? C'est l'heure !";

    fn refine(model: &Arc<FakeChat>) -> LlmRefine {
        LlmRefine::new(Arc::clone(model) as Arc<dyn ChatModel>, PROMPT.to_owned())
    }

    fn correction(model: &Arc<FakeChat>) -> Pipeline {
        Pipeline::new(steps::correction(
            Arc::clone(model) as Arc<dyn ChatModel>,
            PROMPT.to_owned(),
        ))
    }

    fn run(pipeline: &Pipeline, transcript: &str) -> PipelineOutput {
        let transcript = Transcript::new(transcript.to_owned());
        resolve(pipeline.run(Generation(1), transcript))
    }

    fn final_text(output: &PipelineOutput) -> &str {
        let Event::TextReady(Generation(1), text) = &output.event else {
            panic!("{:?}", output.event);
        };
        text.as_str()
    }

    fn results(output: &PipelineOutput) -> Vec<(&'static str, StepResult)> {
        output
            .report
            .iter()
            .map(|report| (report.step, report.result))
            .collect()
    }

    #[test]
    fn a_transcript_without_words_is_nothing_heard_and_no_step_runs() {
        let model = FakeChat::replying(REPLY, Finish::Complete);
        let pipeline = correction(&model);

        for text in ["", " ", "\n\t ", "\u{a0}\n"] {
            let output = run(&pipeline, text);

            assert_matches!(output.event, Event::NothingHeard(Generation(1)), "{text:?}");
            assert!(output.report.is_empty(), "{text:?}");
        }
        assert_eq!(model.sent(), [] as [Sent; 0]);
    }

    #[test]
    fn without_steps_the_final_text_is_the_raw_transcript_without_a_copy() {
        let transcript = Transcript::new(RAW.to_owned());
        let address = transcript.as_str().as_ptr();

        let output = resolve(Pipeline::new(Vec::new()).run(Generation(1), transcript));

        assert_eq!(final_text(&output), RAW);
        assert_eq!(final_text(&output).as_ptr(), address);
        assert!(output.report.is_empty());
    }

    #[test]
    fn each_step_gets_the_text_of_the_previous_step() {
        let model = FakeChat::replying(REPLY, Finish::Complete);

        let output = run(&correction(&model), RAW);

        assert_eq!(final_text(&output), REPLY_FINAL);
        assert_eq!(
            model.sent()[0].user,
            format!("<transcript>\n{RAW}\n</transcript>")
        );
        assert_matches!(
            output.report.as_slice(),
            [
                StepReport {
                    step: "llm_refine",
                    result: StepResult::Applied
                },
                StepReport {
                    step: "typography",
                    result: StepResult::Applied
                },
                StepReport {
                    step: "trim",
                    result: StepResult::Applied
                },
            ]
        );
    }

    #[test]
    fn a_failed_correction_keeps_the_raw_transcript_and_the_next_steps_run() {
        let failures = [
            (
                FakeChat::replying("Il est", Finish::Truncated),
                StepError::Rejected(Rejection::Incomplete),
            ),
            (
                FakeChat::replying(REPLY, Finish::Other),
                StepError::Rejected(Rejection::Incomplete),
            ),
            (
                FakeChat::replying("\n \n", Finish::Complete),
                StepError::Rejected(Rejection::Empty),
            ),
            (
                FakeChat::failing(ProviderError::Timeout),
                StepError::Provider(ProviderError::Timeout),
            ),
            (
                FakeChat::failing(ProviderError::Unauthorized),
                StepError::Provider(ProviderError::Unauthorized),
            ),
        ];
        for (model, error) in failures {
            let output = run(&correction(&model), RAW);

            assert_eq!(final_text(&output), RAW_FINAL, "{error:?}");
            assert_eq!(
                results(&output),
                [
                    ("llm_refine", StepResult::Failed(error)),
                    ("typography", StepResult::Applied),
                    ("trim", StepResult::Applied),
                ]
            );
        }
    }

    #[test]
    fn a_step_after_a_failure_gets_the_text_from_before_the_failure() {
        let first = FakeChat::failing(ProviderError::Network);
        let second = FakeChat::replying("Deuxième version.", Finish::Complete);
        let pipeline = Pipeline::new(vec![
            Box::new(refine(&first)) as Box<dyn TextStep>,
            Box::new(refine(&second)),
        ]);

        let output = run(&pipeline, RAW);

        assert_eq!(final_text(&output), "Deuxième version.");
        assert_eq!(
            second.sent()[0].user,
            format!("<transcript>\n{RAW}\n</transcript>")
        );
        assert_eq!(
            results(&output),
            [
                (
                    "llm_refine",
                    StepResult::Failed(StepError::Provider(ProviderError::Network))
                ),
                ("llm_refine", StepResult::Applied),
            ]
        );
    }

    #[test]
    fn the_pipeline_debug_shows_the_step_names_and_no_prompt() {
        let model = FakeChat::replying(REPLY, Finish::Complete);

        let debug = format!("{:?}", correction(&model));

        assert_eq!(
            debug,
            r#"Pipeline { steps: ["llm_refine", "typography", "trim"] }"#
        );
    }

    #[test]
    fn the_output_debug_shows_no_text() {
        let model = FakeChat::replying(REPLY, Finish::Complete);

        let output = run(&correction(&model), RAW);

        let debug = format!("{output:?}");
        assert!(!debug.contains("heure"), "{debug}");
        assert!(debug.contains("FinalText { chars: 33 }"), "{debug}");
    }
}
