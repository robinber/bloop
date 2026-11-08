//! Mistral chat completions through HTTP.

use std::time::Instant;

use bloop_core::error::ProviderError;
use bloop_core::ports::ai::{BoxFuture, ChatModel, ChatReply, ChatRequest, Finish};
use http_body_util::Full;
use hyper::Request;
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};

use super::ChatConfig;
use crate::http;

const URL: &str = "https://api.mistral.ai/v1/chat/completions";

/// A Mistral chat model. Its requests share the connections of one HTTP client.
#[derive(Debug)]
pub(crate) struct Chat {
    url: String,
    client: http::Client,
    key: SecretString,
    config: ChatConfig,
}

impl Chat {
    /// Creates a chat model for the Mistral API. The time limit of the requests is the limit of `client`.
    pub(crate) fn new(client: http::Client, key: SecretString, config: ChatConfig) -> Self {
        Self {
            url: URL.to_owned(),
            client,
            key,
            config,
        }
    }

    async fn send(&self, request: ChatRequest<'_>) -> Result<ChatReply, ProviderError> {
        let body = Body {
            model: &self.config.model,
            messages: [
                Message {
                    role: "system",
                    content: request.system,
                },
                Message {
                    role: "user",
                    content: request.user,
                },
            ],
            max_tokens: request.max_tokens.get(),
            temperature: self.config.temperature,
            reasoning_effort: self.config.reasoning_effort.as_deref(),
            stream: false,
            prediction: request.prediction.map(|content| Prediction {
                kind: "content",
                content,
            }),
        };
        let started = Instant::now();
        let (version, completion): (_, Completion) =
            http::send(&self.client, self.request(&body)?).await?;
        let Some(choice) = completion.choices.into_iter().next() else {
            tracing::warn!("the provider sent no choice");
            return Err(ProviderError::InvalidResponse);
        };
        let Some(content) = choice.message.content else {
            tracing::warn!("the provider sent no content");
            return Err(ProviderError::InvalidResponse);
        };
        let finish = match choice.finish_reason.as_deref() {
            Some("stop") => Finish::Complete,
            Some("length" | "model_length") => Finish::Truncated,
            _ => Finish::Other,
        };
        let usage = completion.usage.unwrap_or_default();
        tracing::info!(
            model = %self.config.model,
            http_version = ?version,
            millis = started.elapsed().as_millis(),
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            ?finish,
            "the chat model replied"
        );
        Ok(ChatReply {
            text: content.into_text(),
            finish,
        })
    }

    /// Builds the POST request with the JSON body and the API key.
    fn request(&self, body: &Body<'_>) -> Result<Request<Full<Bytes>>, ProviderError> {
        let json = serde_json::to_vec(body).map_err(|_| {
            tracing::warn!("the adapter cannot encode the request");
            ProviderError::Network
        })?;
        Request::post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, self.authorization()?)
            .body(Full::from(json))
            .map_err(|_| {
                tracing::warn!("the adapter cannot build the request");
                ProviderError::Network
            })
    }

    fn authorization(&self) -> Result<HeaderValue, ProviderError> {
        let bearer = Zeroizing::new(format!("Bearer {}", self.key.expose_secret()));
        let mut authorization = HeaderValue::from_str(&bearer).map_err(|_| {
            tracing::warn!("the API key is not a valid HTTP header value");
            ProviderError::Unauthorized
        })?;
        authorization.set_sensitive(true);
        Ok(authorization)
    }
}

impl ChatModel for Chat {
    fn complete<'a>(
        &'a self,
        request: ChatRequest<'a>,
    ) -> BoxFuture<'a, Result<ChatReply, ProviderError>> {
        Box::pin(self.send(request))
    }
}

/// The request body. It borrows the texts of the request.
#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    messages: [Message<'a>; 2],
    max_tokens: u32,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    prediction: Option<Prediction<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
struct Prediction<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    content: &'a str,
}

/// The parts of the response that the adapter uses.
#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Reply,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    content: Option<Content>,
}

/// The content of the reply: one string, or a list of chunks.
#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Chunks(Vec<Chunk>),
}

impl Content {
    /// Returns the text. For a list of chunks, it joins the text chunks and ignores the other chunks.
    fn into_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Chunks(chunks) => chunks
                .into_iter()
                .filter_map(|chunk| match chunk {
                    Chunk::Text { text } => Some(text),
                    Chunk::Other => None,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Chunk {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Default, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::num::NonZeroU32;
    use std::time::Duration;

    use bloop_core::error::ProviderError;
    use bloop_core::ports::ai::{ChatModel as _, ChatReply, ChatRequest, Finish};
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tracing::instrument::WithSubscriber as _;

    use super::{Chat, ChatConfig};
    use crate::http;
    use crate::test_log::Log;

    const KEY: &str = "test-key";
    const MODEL: &str = "mistral-test";
    const SYSTEM: &str = "Corrige le texte.";
    const USER: &str = "<transcript>\nbonjour a tous\n</transcript>";
    const PREDICTION: &str = "bonjour a tous";
    const TEXT: &str = "Bonjour à tous.";
    const LIMIT: Duration = Duration::from_secs(5);
    const MAX_TOKENS: NonZeroU32 = NonZeroU32::new(64).unwrap();

    /// The request that the fake provider received.
    struct Received {
        line: String,
        headers: Vec<(String, String)>,
        body: Value,
    }

    impl Received {
        /// Returns the values of the header `name`, in lowercase.
        fn header(&self, name: &str) -> Vec<&str> {
            self.headers
                .iter()
                .filter(|(header, _)| header == name)
                .map(|(_, value)| value.as_str())
                .collect()
        }
    }

    fn config(reasoning_effort: Option<&str>) -> ChatConfig {
        ChatConfig {
            model: MODEL.to_owned(),
            models: Vec::new(),
            reasoning_effort: reasoning_effort.map(str::to_owned),
            temperature: 0.0,
        }
    }

    fn request(prediction: Option<&str>) -> ChatRequest<'_> {
        ChatRequest {
            system: SYSTEM,
            user: USER,
            prediction,
            max_tokens: MAX_TOKENS,
        }
    }

    /// A response body with one choice.
    fn completion(content: &Value, finish_reason: &Value) -> String {
        json!({
            "id": "test",
            "object": "chat.completion",
            "model": MODEL,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": finish_reason,
            }],
            "usage": { "prompt_tokens": 20, "completion_tokens": 5, "total_tokens": 25 },
        })
        .to_string()
    }

    /// Binds a fake provider and returns a chat model that sends to it.
    async fn listen(config: ChatConfig, limit: Duration) -> (Chat, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let chat = Chat {
            url: format!("http://{address}/v1/chat/completions"),
            client: http::client(limit).unwrap(),
            key: KEY.into(),
            config,
        };
        (chat, listener)
    }

    /// Accepts one connection and reads one HTTP/1.1 request with a `content-length` body.
    async fn accept(listener: &TcpListener) -> (TcpStream, Received) {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut data = Vec::new();
        let mut buffer = [0; 4_096];
        let end = loop {
            if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0, "the client closed the connection");
            data.extend_from_slice(&buffer[..count]);
        };
        let head = std::str::from_utf8(&data[..end]).unwrap().to_owned();
        let mut lines = head.split("\r\n").filter(|line| !line.is_empty());
        let line = lines.next().unwrap().to_owned();
        let headers: Vec<(String, String)> = lines
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                (name.to_ascii_lowercase(), value.trim().to_owned())
            })
            .collect();
        let length: usize = headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .unwrap()
            .1
            .parse()
            .unwrap();
        while data.len() < end + length {
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0, "the client closed the connection");
            data.extend_from_slice(&buffer[..count]);
        }
        let body = serde_json::from_slice(&data[end..end + length]).unwrap();
        (
            stream,
            Received {
                line,
                headers,
                body,
            },
        )
    }

    /// Answers one request with `status` and `body`. The task returns the request.
    async fn serve(config: ChatConfig, status: u16, body: String) -> (Chat, JoinHandle<Received>) {
        let (chat, listener) = listen(config, LIMIT).await;
        let server = tokio::spawn(async move {
            let (mut stream, received) = accept(&listener).await;
            let response = format!(
                "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
            received
        });
        (chat, server)
    }

    /// Sends one request to a fake provider that answers with `status` and `body`.
    async fn complete(status: u16, body: String) -> Result<ChatReply, ProviderError> {
        let (chat, server) = serve(config(Some("none")), status, body).await;
        let result = chat.complete(request(None)).await;
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn a_complete_reply_gives_the_text_and_complete() {
        let reply = complete(200, completion(&json!(TEXT), &json!("stop")))
            .await
            .unwrap();

        assert_eq!(reply.text, TEXT);
        assert_eq!(reply.finish, Finish::Complete);
    }

    #[tokio::test]
    async fn the_request_contains_the_messages_and_the_options() {
        let (chat, server) = serve(
            config(Some("none")),
            200,
            completion(&json!(TEXT), &json!("stop")),
        )
        .await;

        chat.complete(request(None)).await.unwrap();

        let received = server.await.unwrap();
        assert_eq!(received.line, "POST /v1/chat/completions HTTP/1.1");
        assert_eq!(received.header("content-type"), ["application/json"]);
        assert_eq!(
            received.body,
            json!({
                "model": MODEL,
                "messages": [
                    { "role": "system", "content": SYSTEM },
                    { "role": "user", "content": USER },
                ],
                "max_tokens": 64,
                "temperature": 0.0,
                "reasoning_effort": "none",
                "stream": false,
            })
        );
    }

    #[tokio::test]
    async fn the_request_contains_the_prediction_only_when_the_request_has_one() {
        let (chat, server) = serve(
            config(Some("none")),
            200,
            completion(&json!(TEXT), &json!("stop")),
        )
        .await;

        chat.complete(request(Some(PREDICTION))).await.unwrap();

        let body = server.await.unwrap().body;
        assert_eq!(
            body["prediction"],
            json!({ "type": "content", "content": PREDICTION })
        );
    }

    #[tokio::test]
    async fn the_request_omits_reasoning_effort_when_the_config_has_none() {
        let (chat, server) =
            serve(config(None), 200, completion(&json!(TEXT), &json!("stop"))).await;

        chat.complete(request(None)).await.unwrap();

        let body = server.await.unwrap().body;
        assert_eq!(body.get("reasoning_effort"), None);
        assert_eq!(body.get("prediction"), None);
        assert_eq!(body["model"], MODEL);
    }

    #[tokio::test]
    async fn the_authorization_header_carries_the_key() {
        let (chat, server) = serve(
            config(Some("none")),
            200,
            completion(&json!(TEXT), &json!("stop")),
        )
        .await;

        chat.complete(request(None)).await.unwrap();

        let received = server.await.unwrap();
        assert_eq!(received.header("authorization"), [format!("Bearer {KEY}")]);
        assert!(chat.authorization().unwrap().is_sensitive());
    }

    #[tokio::test]
    async fn a_reply_at_the_token_limit_is_truncated() {
        for reason in ["length", "model_length"] {
            let reply = complete(200, completion(&json!(TEXT), &json!(reason)))
                .await
                .unwrap();

            assert_eq!(reply.finish, Finish::Truncated, "{reason}");
            assert_eq!(reply.text, TEXT);
        }
    }

    #[tokio::test]
    async fn another_finish_reason_gives_other() {
        for reason in [json!("tool_calls"), json!("error"), json!(null)] {
            let reply = complete(200, completion(&json!(TEXT), &reason))
                .await
                .unwrap();

            assert_eq!(reply.finish, Finish::Other, "{reason}");
        }
    }

    #[tokio::test]
    async fn an_unknown_finish_reason_is_logged_only_as_a_local_category() {
        const PRIVATE: &str = "private-dictation-and-key-sentinel";
        let log = Log::default();

        let reply = complete(200, completion(&json!(TEXT), &json!(PRIVATE)))
            .with_subscriber(log.clone())
            .await
            .unwrap();

        assert_eq!(reply.finish, Finish::Other);
        let output = log.text();
        assert!(output.contains("finish=Other"), "{output}");
        assert!(!output.contains(PRIVATE), "{output}");
        assert!(!output.contains(TEXT), "{output}");
    }

    #[tokio::test]
    async fn chunk_content_joins_the_text_chunks() {
        let content = json!([
            { "type": "thinking", "thinking": [{ "type": "text", "text": "Hmm." }] },
            { "type": "text", "text": "Bonjour " },
            { "type": "reference", "reference_ids": [1] },
            { "type": "text", "text": "à tous." },
        ]);

        let reply = complete(200, completion(&content, &json!("stop")))
            .await
            .unwrap();

        assert_eq!(reply.text, TEXT);
        assert_eq!(reply.finish, Finish::Complete);
    }

    #[tokio::test]
    async fn an_http_error_maps_the_status() {
        for (status, expected) in [
            (401, ProviderError::Unauthorized),
            (403, ProviderError::Unauthorized),
            (429, ProviderError::Http(429)),
            (500, ProviderError::Http(500)),
        ] {
            let body = json!({ "message": "error", "type": "test" }).to_string();

            let result = complete(status, body).await;

            assert_eq!(result.err(), Some(expected), "{status}");
        }
    }

    #[tokio::test]
    async fn a_mistral_rate_limit_error_gives_http_429() {
        let body = json!({
            "object": "error",
            "message": "Rate limit exceeded",
            "type": "rate_limited",
            "param": null,
            "code": "1300",
            "raw_status_code": 429,
        })
        .to_string();

        let result = complete(429, body).await;

        assert_eq!(result.err(), Some(ProviderError::Http(429)));
    }

    #[tokio::test]
    async fn an_http_error_body_never_enters_logs() {
        const PRIVATE: &str = "private-dictation-and-key-sentinel";
        let log = Log::default();
        let body = json!({ "message": PRIVATE, "type": PRIVATE, "code": PRIVATE }).to_string();

        let result = complete(429, body).with_subscriber(log.clone()).await;

        assert_eq!(result.err(), Some(ProviderError::Http(429)));
        let output = log.text();
        assert!(output.contains("status=429"), "{output}");
        assert!(!output.contains(PRIVATE), "{output}");
    }

    #[tokio::test]
    async fn an_http_error_with_a_stalled_body_gives_the_status() {
        let (chat, listener) = listen(config(Some("none")), Duration::from_millis(200)).await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = accept(&listener).await;
            let head = "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"type\": ";
            stream.write_all(head.as_bytes()).await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });

        let result = tokio::time::timeout(LIMIT, chat.complete(request(None)))
            .await
            .expect("the request limit ends a stalled error body");

        server.abort();
        assert_eq!(result.err(), Some(ProviderError::Http(429)));
    }

    #[tokio::test]
    async fn an_http_error_with_a_large_body_gives_the_status() {
        let (chat, listener) = listen(config(Some("none")), LIMIT).await;
        let body = json!({ "type": "rate_limited", "code": "1300" }).to_string()
            + &" ".repeat(http::BODY_LIMIT);
        let server = tokio::spawn(async move {
            let (mut stream, _) = accept(&listener).await;
            let response = format!(
                "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()).await {
                assert!(
                    matches!(
                        error.kind(),
                        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                    ),
                    "{error}"
                );
            }
        });

        let result = chat.complete(request(None)).await;

        server.await.unwrap();
        assert_eq!(result.err(), Some(ProviderError::Http(429)));
    }

    #[tokio::test]
    async fn a_provider_slower_than_the_limit_gives_timeout() {
        let (chat, listener) = listen(config(Some("none")), Duration::from_millis(200)).await;
        let server = tokio::spawn(async move {
            let (stream, _) = accept(&listener).await;
            std::future::pending::<()>().await;
            drop(stream);
        });

        let result = chat.complete(request(None)).await;

        server.abort();
        assert_eq!(result.err(), Some(ProviderError::Timeout));
    }

    #[tokio::test]
    async fn a_body_that_stalls_after_the_headers_gives_timeout() {
        let (chat, listener) = listen(config(Some("none")), Duration::from_millis(200)).await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = accept(&listener).await;
            let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"choices\": [";
            stream.write_all(head.as_bytes()).await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });

        let result = tokio::time::timeout(Duration::from_secs(5), chat.complete(request(None)))
            .await
            .expect("the request limit ends a stalled body");

        server.abort();
        assert_eq!(result.err(), Some(ProviderError::Timeout));
    }

    #[tokio::test]
    async fn dropping_the_request_after_the_provider_received_it_closes_the_connection() {
        let (chat, listener) = listen(config(Some("none")), LIMIT).await;

        let mut stream = tokio::select! {
            result = chat.complete(request(None)) => panic!("the fake provider never answers: {result:?}"),
            (stream, _) = accept(&listener) => stream,
        };

        let mut buffer = [0; 64];
        let count = tokio::time::timeout(LIMIT, stream.read(&mut buffer))
            .await
            .expect("the connection closes before the limit")
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn a_closed_port_gives_network() {
        let (chat, listener) = listen(config(Some("none")), LIMIT).await;
        drop(listener);

        let result = chat.complete(request(None)).await;

        assert_eq!(result.err(), Some(ProviderError::Network));
    }

    #[tokio::test]
    async fn an_unusable_body_gives_invalid_response() {
        for body in [
            "not JSON".to_owned(),
            "{\"choices\": [{\"message\": ".to_owned(),
            json!({ "id": "test" }).to_string(),
            json!({ "choices": [] }).to_string(),
            json!({ "choices": [{ "finish_reason": "stop" }] }).to_string(),
            completion(&json!(null), &json!("stop")),
            json!({ "choices": [{ "message": { "role": "assistant" }, "finish_reason": "stop" }] })
                .to_string(),
            completion(&json!([{ "text": "no type" }]), &json!("stop")),
        ] {
            let result = complete(200, body.clone()).await;

            assert_eq!(result.err(), Some(ProviderError::InvalidResponse), "{body}");
        }
    }

    #[tokio::test]
    async fn a_body_larger_than_the_limit_gives_invalid_response() {
        let (chat, listener) = listen(config(Some("none")), LIMIT).await;
        let body = completion(&json!(TEXT), &json!("stop")) + &" ".repeat(http::BODY_LIMIT);
        let server = tokio::spawn(async move {
            let (mut stream, _) = accept(&listener).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()).await {
                assert!(
                    matches!(
                        error.kind(),
                        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                    ),
                    "{error}"
                );
            }
        });

        let result = chat.complete(request(None)).await;

        server.await.unwrap();
        assert_eq!(result.err(), Some(ProviderError::InvalidResponse));
    }

    #[tokio::test]
    async fn the_debug_of_the_adapter_shows_no_key() {
        let (chat, _listener) = listen(config(Some("none")), LIMIT).await;

        let debug = format!("{chat:?}");

        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains(KEY));
    }
}
