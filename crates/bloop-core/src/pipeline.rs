//! Text pipeline: turns the complete transcript of a job into its result event and its step report.

use std::sync::Arc;

use crate::domain::{FinalText, Generation, Transcript};
use crate::error::StepError;
use crate::ports::ai::{SkipReason, StepContext, StepOutcome, TextStep};
use crate::session::Event;

/// The text steps of one generation, in order.
pub struct Pipeline {
    steps: Vec<Arc<dyn TextStep>>,
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
#[derive(Debug)]
pub enum StepResult {
    /// The text of the step replaced the previous text.
    Applied,
    /// The step did not change the text.
    Skipped(SkipReason),
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
    pub fn new(steps: Vec<Arc<dyn TextStep>>) -> Self {
        Self { steps }
    }

    /// Applies the steps to the complete transcript of `generation`.
    ///
    /// A transcript that contains only whitespace gives `NothingHeard` and an empty report.
    /// Otherwise each step gets the current text, and an applied step replaces it.
    /// After a skip or a failure, the text from before that step continues to the next step.
    /// The result is `TextReady` with the text after the last step.
    pub async fn run(
        &self,
        generation: Generation,
        transcript: Transcript,
        context: &StepContext,
    ) -> PipelineOutput {
        if transcript.as_str().trim().is_empty() {
            return PipelineOutput {
                event: Event::NothingHeard(generation),
                report: Vec::new(),
            };
        }
        let mut text = FinalText::from(transcript);
        let mut report = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let outcome = step.apply(text.as_str(), context).await;
            let result = match outcome {
                Ok(StepOutcome::Applied(changed)) => {
                    text = FinalText::new(changed);
                    StepResult::Applied
                }
                Ok(StepOutcome::Skipped(reason)) => StepResult::Skipped(reason),
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
    use crate::error::ProviderError;
    use crate::fakes::{FakeChat, Sent, context, resolve};
    use crate::ports::ai::{ChatModel, Finish, SkipReason, TextStep};
    use crate::session::Event;
    use crate::steps::{LlmRefine, Trim, Typography};

    const PROMPT: &str = "Corrige la ponctuation.";
    const RAW: &str = "euh il est trois heures\u{a0}? c’est l’heure\n";
    const RAW_FINAL: &str = "euh il est trois heures ? c'est l'heure";
    const REPLY: &str = "\nIl est 3\u{a0}heures\u{a0}? C’est l’heure\u{202f}!\n";
    const REPLY_FINAL: &str = "Il est 3\u{a0}heures ? C'est l'heure !";

    fn refine(model: &Arc<FakeChat>) -> LlmRefine {
        LlmRefine::new(Arc::clone(model) as Arc<dyn ChatModel>, PROMPT.to_owned())
    }

    fn correction(refine: LlmRefine) -> Pipeline {
        Pipeline::new(vec![Arc::new(refine), Arc::new(Typography), Arc::new(Trim)])
    }

    fn run(pipeline: &Pipeline, transcript: &str) -> PipelineOutput {
        let transcript = Transcript::new(transcript.to_owned());
        resolve(pipeline.run(Generation(1), transcript, &context()))
    }

    fn final_text(output: &PipelineOutput) -> &str {
        let Event::TextReady(Generation(1), text) = &output.event else {
            panic!("{:?}", output.event);
        };
        text.as_str()
    }

    fn results(output: &PipelineOutput) -> Vec<(&'static str, String)> {
        output
            .report
            .iter()
            .map(|report| (report.step, format!("{:?}", report.result)))
            .collect()
    }

    #[test]
    fn a_transcript_without_words_is_nothing_heard_and_no_step_runs() {
        let model = FakeChat::replying(REPLY, Finish::Complete);
        let pipeline = correction(refine(&model));

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

        let output = resolve(Pipeline::new(Vec::new()).run(Generation(1), transcript, &context()));

        assert_eq!(final_text(&output), RAW);
        assert_eq!(final_text(&output).as_ptr(), address);
        assert!(output.report.is_empty());
    }

    #[test]
    fn each_step_gets_the_text_of_the_previous_step() {
        let model = FakeChat::replying(REPLY, Finish::Complete);

        let output = run(&correction(refine(&model)), RAW);

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
                "Failed(Rejected(Incomplete))",
            ),
            (
                FakeChat::replying(REPLY, Finish::Other),
                "Failed(Rejected(Incomplete))",
            ),
            (
                FakeChat::replying("\n \n", Finish::Complete),
                "Failed(Rejected(Empty))",
            ),
            (
                FakeChat::failing(ProviderError::Timeout),
                "Failed(Provider(Timeout))",
            ),
            (
                FakeChat::failing(ProviderError::Unauthorized),
                "Failed(Provider(Unauthorized))",
            ),
        ];
        for (model, expected) in failures {
            let output = run(&correction(refine(&model)), RAW);

            assert_eq!(final_text(&output), RAW_FINAL, "{expected}");
            assert_eq!(
                results(&output),
                [
                    ("llm_refine", expected.to_owned()),
                    ("typography", "Applied".to_owned()),
                    ("trim", "Applied".to_owned()),
                ]
            );
        }
    }

    #[test]
    fn a_rejected_ratio_keeps_the_raw_transcript() {
        for (reply, expected) in [
            ("Oui.", "Failed(Rejected(TooShort))"),
            (&*RAW.repeat(2), "Failed(Rejected(TooLong))"),
        ] {
            let model = FakeChat::replying(reply, Finish::Complete);

            let output = run(&correction(refine(&model).with_ratio(50..=160)), RAW);

            assert_eq!(final_text(&output), RAW_FINAL, "{expected}");
            assert_eq!(results(&output)[0], ("llm_refine", expected.to_owned()));
        }
    }

    #[test]
    fn a_skipped_correction_keeps_the_raw_transcript_and_the_next_steps_run() {
        let model = FakeChat::replying(REPLY, Finish::Complete);
        let pipeline = correction(refine(&model).with_min_words(4));

        let output = run(&pipeline, "Oui\u{a0}!\n");

        assert_eq!(final_text(&output), "Oui !");
        assert_eq!(model.sent(), [] as [Sent; 0]);
        assert_matches!(
            output.report.as_slice(),
            [
                StepReport {
                    step: "llm_refine",
                    result: StepResult::Skipped(SkipReason::TooFewWords)
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
    fn a_step_after_a_failure_gets_the_text_from_before_the_failure() {
        let first = FakeChat::failing(ProviderError::Network);
        let second = FakeChat::replying("Deuxième version.", Finish::Complete);
        let pipeline = Pipeline::new(vec![
            Arc::new(refine(&first)) as Arc<dyn TextStep>,
            Arc::new(refine(&second)),
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
                ("llm_refine", "Failed(Provider(Network))".to_owned()),
                ("llm_refine", "Applied".to_owned()),
            ]
        );
    }

    #[test]
    fn the_output_debug_shows_no_text() {
        let model = FakeChat::replying(REPLY, Finish::Complete);

        let output = run(&correction(refine(&model)), RAW);

        let debug = format!("{output:?}");
        assert!(!debug.contains("heure"), "{debug}");
        assert!(debug.contains("FinalText { chars: 33 }"), "{debug}");
    }
}
