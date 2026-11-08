//! Fake chat model and a future driver for the tests of the text steps and the pipeline.

use std::future;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::error::ProviderError;
use crate::ports::ai::{BoxFuture, ChatModel, ChatReply, ChatRequest, Finish};

/// Polls `future` one time and returns its output. The fakes do not wait, so the future is ready.
pub(crate) fn resolve<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future waits"),
    }
}

/// A request that the fake chat model received.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Sent {
    pub(crate) system: String,
    pub(crate) user: String,
    pub(crate) max_tokens: u32,
}

/// A chat model that records each request and gives the same reply to each.
pub(crate) struct FakeChat {
    reply: Result<(String, Finish), ProviderError>,
    sent: Mutex<Vec<Sent>>,
}

impl FakeChat {
    pub(crate) fn replying(text: &str, finish: Finish) -> Arc<Self> {
        Self::new(Ok((text.to_owned(), finish)))
    }

    pub(crate) fn failing(error: ProviderError) -> Arc<Self> {
        Self::new(Err(error))
    }

    fn new(reply: Result<(String, Finish), ProviderError>) -> Arc<Self> {
        Arc::new(Self {
            reply,
            sent: Mutex::new(Vec::new()),
        })
    }

    /// Returns the requests since the previous call, in order.
    pub(crate) fn sent(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

impl ChatModel for FakeChat {
    fn complete<'a>(
        &'a self,
        request: ChatRequest<'a>,
    ) -> BoxFuture<'a, Result<ChatReply, ProviderError>> {
        self.sent.lock().unwrap().push(Sent {
            system: request.system.to_owned(),
            user: request.user.to_owned(),
            max_tokens: request.max_tokens.get(),
        });
        let reply = self
            .reply
            .clone()
            .map(|(text, finish)| ChatReply { text, finish });
        Box::pin(future::ready(reply))
    }
}
