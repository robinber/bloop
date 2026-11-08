//! One sample through the correction steps, with the duration and the finish reason of its chat request.

use std::mem;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bloop_core::domain::{FinalText, Transcript};
use bloop_core::error::ProviderError;
use bloop_core::pipeline::{Pipeline, StepReport, StepResult};
use bloop_core::ports::ai::{BoxFuture, ChatModel, ChatReply, ChatRequest, Finish, StepContext};
use tokio::time::Instant;

/// One chat request: its duration and how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) elapsed: Duration,
    pub(crate) result: Result<Finish, ProviderError>,
}

/// A chat model that forwards each request to another model and records the request.
pub(crate) struct Recording {
    inner: Arc<dyn ChatModel>,
    requests: Mutex<Vec<Request>>,
}

impl Recording {
    pub(crate) fn new(inner: Arc<dyn ChatModel>) -> Self {
        Self {
            inner,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Returns the requests since the previous call, in order.
    fn take(&self) -> Vec<Request> {
        mem::take(&mut *self.requests())
    }

    /// The recorded requests. A push cannot leave the list half changed, so a poisoned lock keeps the list.
    fn requests(&self) -> MutexGuard<'_, Vec<Request>> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ChatModel for Recording {
    fn complete<'a>(
        &'a self,
        request: ChatRequest<'a>,
    ) -> BoxFuture<'a, Result<ChatReply, ProviderError>> {
        Box::pin(async move {
            let started = Instant::now();
            let reply = self.inner.complete(request).await;
            let result = match &reply {
                Ok(reply) => Ok(reply.finish),
                Err(error) => Err(*error),
            };
            self.requests().push(Request {
                elapsed: started.elapsed(),
                result,
            });
            reply
        })
    }
}

/// The result of one sample.
pub(crate) struct Outcome {
    /// The final text, or `None` when the transcript contains only whitespace.
    pub(crate) text: Option<FinalText>,
    pub(crate) report: Vec<StepReport>,
    pub(crate) requests: Vec<Request>,
}

impl Outcome {
    /// Returns the steps that failed, with their result, or `None` when each step applied.
    /// When the correction failed, the final text is the raw transcript after the other steps.
    pub(crate) fn fallback(&self) -> Option<String> {
        let reasons: Vec<String> = self
            .report
            .iter()
            .filter(|report| matches!(report.result, StepResult::Failed(_)))
            .map(|report| format!("{} {:?}", report.step, report.result))
            .collect();
        (!reasons.is_empty()).then(|| reasons.join(", "))
    }
}

/// Runs `raw` through `pipeline`, whose chat model is `chat`, and returns the outcome with the requests of the run.
pub(crate) async fn sample(
    pipeline: &Pipeline,
    chat: &Recording,
    raw: &str,
    context: &StepContext,
) -> Outcome {
    let processed = pipeline
        .process(Transcript::new(raw.to_owned()), context)
        .await;
    Outcome {
        text: processed.text,
        report: processed.report,
        requests: chat.take(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bloop_core::error::ProviderError;
    use bloop_core::pipeline::Pipeline;
    use bloop_core::ports::ai::{
        BoxFuture, ChatModel, ChatReply, ChatRequest, Finish, StepContext,
    };
    use bloop_core::steps;
    use tokio::time::sleep;

    use super::{Outcome, Recording, Request, sample};

    const RAW: &str = "euh tu viens ce soir\u{a0}?";
    const RAW_FINAL: &str = "euh tu viens ce soir ?";

    struct FakeChat {
        delay: Duration,
        reply: Result<(&'static str, Finish), ProviderError>,
    }

    impl ChatModel for FakeChat {
        fn complete<'a>(
            &'a self,
            _request: ChatRequest<'a>,
        ) -> BoxFuture<'a, Result<ChatReply, ProviderError>> {
            Box::pin(async move {
                sleep(self.delay).await;
                self.reply.map(|(text, finish)| ChatReply {
                    text: text.to_owned(),
                    finish,
                })
            })
        }
    }

    async fn run(delay: Duration, reply: Result<(&'static str, Finish), ProviderError>) -> Outcome {
        let chat = Arc::new(Recording::new(Arc::new(FakeChat { delay, reply })));
        let pipeline = Pipeline::new(steps::correction(
            Arc::clone(&chat) as Arc<dyn ChatModel>,
            "Corrige le texte.".to_owned(),
        ));
        let context = StepContext {
            language: "fr".to_owned(),
        };
        sample(&pipeline, &chat, RAW, &context).await
    }

    fn final_text(outcome: &Outcome) -> &str {
        outcome.text.as_ref().unwrap().as_str()
    }

    #[tokio::test(start_paused = true)]
    async fn a_complete_reply_gives_the_corrected_text_and_records_the_request() {
        let outcome = run(
            Duration::from_millis(640),
            Ok(("\nTu viens ce soir\u{a0}?\n", Finish::Complete)),
        )
        .await;

        assert_eq!(final_text(&outcome), "Tu viens ce soir ?");
        assert_eq!(outcome.fallback(), None);
        assert_eq!(
            outcome.requests,
            [Request {
                elapsed: Duration::from_millis(640),
                result: Ok(Finish::Complete),
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_truncated_reply_keeps_the_raw_transcript_and_records_the_finish() {
        let outcome = run(Duration::from_secs(2), Ok(("Tu viens", Finish::Truncated))).await;

        assert_eq!(final_text(&outcome), RAW_FINAL);
        assert_eq!(
            outcome.fallback().as_deref(),
            Some("llm_refine Failed(Rejected(Incomplete))")
        );
        assert_eq!(
            outcome.requests,
            [Request {
                elapsed: Duration::from_secs(2),
                result: Ok(Finish::Truncated),
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_provider_failure_keeps_the_raw_transcript_and_records_the_error() {
        let outcome = run(Duration::from_secs(30), Err(ProviderError::Timeout)).await;

        assert_eq!(final_text(&outcome), RAW_FINAL);
        assert_eq!(
            outcome.fallback().as_deref(),
            Some("llm_refine Failed(Provider(Timeout))")
        );
        assert_eq!(
            outcome.requests,
            [Request {
                elapsed: Duration::from_secs(30),
                result: Err(ProviderError::Timeout),
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn each_sample_gets_only_its_own_requests() {
        let chat = Arc::new(Recording::new(Arc::new(FakeChat {
            delay: Duration::ZERO,
            reply: Ok(("Oui.", Finish::Complete)),
        })));
        let pipeline = Pipeline::new(steps::correction(
            Arc::clone(&chat) as Arc<dyn ChatModel>,
            "Corrige le texte.".to_owned(),
        ));
        let context = StepContext {
            language: "fr".to_owned(),
        };

        let first = sample(&pipeline, &chat, "oui", &context).await;
        let second = sample(&pipeline, &chat, "oui", &context).await;

        assert_eq!(first.requests.len(), 1);
        assert_eq!(second.requests.len(), 1);
    }
}
