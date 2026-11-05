//! Voxtral realtime transcription through a WebSocket.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bloop_core::domain::{AudioChunk, Transcript};
use bloop_core::error::ProviderError;
use bloop_core::ports::ai::{self, BoxFuture, TranscriptionOptions, TranscriptionSession};
use futures_util::{FutureExt as _, SinkExt as _, StreamExt as _};
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};

use super::TranscribeConfig;

const URL: &str = "wss://api.mistral.ai/v1/audio/transcriptions/realtime";

/// The largest provider frame and message that the adapter accepts. It bounds the receive buffer.
const MAX_MESSAGE_SIZE: usize = 1 << 20;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The Voxtral realtime transcriber. Each session opens its own connection.
#[derive(Debug)]
pub struct Transcriber {
    url: String,
    key: SecretString,
    config: TranscribeConfig,
}

impl Transcriber {
    /// Creates a transcriber for the Mistral API.
    #[must_use]
    pub fn new(key: SecretString, config: TranscribeConfig) -> Self {
        Self {
            url: URL.to_owned(),
            key,
            config,
        }
    }

    async fn open(&self, options: TranscriptionOptions<'_>) -> Result<Session, ProviderError> {
        let config = WebSocketConfig::default()
            .max_frame_size(Some(MAX_MESSAGE_SIZE))
            .max_message_size(Some(MAX_MESSAGE_SIZE));
        let (mut ws, _response) = connect_async_with_config(self.request()?, Some(config), true)
            .await
            .map_err(transport_error)?;
        match read(&mut ws).await? {
            ServerEvent::SessionCreated => {}
            event => return Err(unexpected(&event)),
        }
        let update = json!({
            "type": "session.update",
            "session": {
                "audio_format": {
                    "encoding": "pcm_s16le",
                    "sample_rate": options.sample_rate.get(),
                },
                "target_streaming_delay_ms": self.config.target_streaming_delay_ms,
                "language": options.language,
                "context_bias": options.vocabulary,
            },
        });
        send(&mut ws, &update).await?;
        match read(&mut ws).await? {
            ServerEvent::SessionUpdated => {
                tracing::info!(
                    sample_rate = options.sample_rate.get(),
                    "the session is configured"
                );
                Ok(Session {
                    ws,
                    deltas: 0,
                    delta_chars: 0,
                })
            }
            event => Err(unexpected(&event)),
        }
    }

    fn request(&self) -> Result<Request, ProviderError> {
        let mut request = format!("{}?model={}", self.url, self.config.model)
            .into_client_request()
            .map_err(transport_error)?;
        let bearer = Zeroizing::new(format!("Bearer {}", self.key.expose_secret()));
        let mut authorization = HeaderValue::from_str(&bearer).map_err(|_| {
            tracing::warn!("the API key is not a valid HTTP header value");
            ProviderError::Unauthorized
        })?;
        authorization.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, authorization);
        Ok(request)
    }
}

impl ai::Transcriber for Transcriber {
    fn start<'a>(
        &'a self,
        options: TranscriptionOptions<'a>,
    ) -> BoxFuture<'a, Result<Box<dyn TranscriptionSession>, ProviderError>> {
        Box::pin(async move {
            let session = self.open(options).await?;
            Ok(Box::new(session) as Box<dyn TranscriptionSession>)
        })
    }
}

/// One transcription. Dropping it closes the connection, which cancels the transcription.
struct Session {
    ws: Socket,
    deltas: usize,
    delta_chars: usize,
}

impl Session {
    fn count(&mut self, delta: &Text) {
        self.deltas += 1;
        self.delta_chars += delta.0.chars().count();
    }

    /// Reads the provider events that already arrived, without waiting for more.
    fn drain(&mut self) -> Result<(), ProviderError> {
        while let Some(item) = self.ws.next().now_or_never() {
            match decode(item)? {
                None => {}
                Some(ServerEvent::Delta { text }) => self.count(&text),
                Some(event) => return Err(unexpected(&event)),
            }
        }
        Ok(())
    }
}

impl TranscriptionSession for Session {
    fn send_audio<'a>(
        &'a mut self,
        chunk: &'a AudioChunk,
    ) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move {
            self.drain()?;
            let append = json!({
                "type": "input_audio.append",
                "audio": STANDARD.encode(chunk.as_bytes()),
            });
            send(&mut self.ws, &append).await
        })
    }

    fn finish(mut self: Box<Self>) -> BoxFuture<'static, Result<Transcript, ProviderError>> {
        Box::pin(async move {
            send(&mut self.ws, &json!({ "type": "input_audio.end" })).await?;
            let text = loop {
                match read(&mut self.ws).await? {
                    ServerEvent::Done { text } => break text,
                    ServerEvent::Delta { text } => self.count(&text),
                    event => return Err(unexpected(&event)),
                }
            };
            tracing::info!(
                deltas = self.deltas,
                delta_chars = self.delta_chars,
                "the provider sent the transcript"
            );
            // The transcript is complete, and the drop of the stream closes the socket.
            // Thus a failed close has no effect on the result.
            if self.ws.close(None).await.is_err() {
                tracing::debug!("the provider connection did not close cleanly");
            }
            Ok(Transcript::new(text.0))
        })
    }
}

/// A provider event. The adapter ignores the event types that it does not use.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ServerEvent {
    #[serde(rename = "session.created")]
    SessionCreated,
    #[serde(rename = "session.updated")]
    SessionUpdated,
    #[serde(rename = "transcription.text.delta")]
    Delta {
        #[serde(default)]
        text: Text,
    },
    #[serde(rename = "transcription.done")]
    Done { text: Text },
    #[serde(rename = "error")]
    Error { error: ErrorBody },
    #[serde(other)]
    Other,
}

/// Text from the provider. `Debug` shows only the number of characters.
#[derive(Default, Deserialize)]
struct Text(String);

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Text")
            .field("chars", &self.0.chars().count())
            .finish()
    }
}

/// The Mistral documentation does not give the types of these fields.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: Value,
    #[serde(default)]
    code: Value,
}

async fn send(ws: &mut Socket, message: &Value) -> Result<(), ProviderError> {
    ws.send(Message::text(message.to_string()))
        .await
        .map_err(transport_error)
}

/// Waits for the next provider event.
async fn read(ws: &mut Socket) -> Result<ServerEvent, ProviderError> {
    loop {
        if let Some(event) = decode(ws.next().await)? {
            return Ok(event);
        }
    }
}

/// Decodes one item of the stream. A control frame or an event that the adapter does not use gives `None`.
fn decode(item: Option<Result<Message, WsError>>) -> Result<Option<ServerEvent>, ProviderError> {
    match item {
        Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
            Ok(ServerEvent::Other) => {
                let event = serde_json::from_str::<Value>(&text);
                let kind = event
                    .as_ref()
                    .map_or("?", |event| event["type"].as_str().unwrap_or("?"));
                tracing::info!(
                    ?kind,
                    "the provider sent an event that the adapter does not use"
                );
                Ok(None)
            }
            Ok(event) => Ok(Some(event)),
            Err(error) => {
                tracing::warn!(
                    category = ?error.classify(),
                    "the adapter cannot decode the provider event"
                );
                Err(ProviderError::InvalidResponse)
            }
        },
        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => Ok(None),
        Some(Ok(Message::Binary(_))) => {
            tracing::warn!("the provider sent a binary message");
            Err(ProviderError::InvalidResponse)
        }
        Some(Ok(Message::Close(frame))) => {
            tracing::warn!(
                code = ?frame.map(|frame| frame.code),
                "the provider closed the connection"
            );
            Err(ProviderError::Network)
        }
        None => {
            tracing::warn!("the provider connection ended");
            Err(ProviderError::Network)
        }
        Some(Err(error)) => Err(transport_error(error)),
    }
}

/// Logs the event that the current operation does not accept.
fn unexpected(event: &ServerEvent) -> ProviderError {
    if let ServerEvent::Error { error } = event {
        tracing::warn!(code = %error.code, message = %error.message, "the provider sent an error");
    } else {
        tracing::warn!(?event, "the provider sent an unexpected event");
    }
    ProviderError::InvalidResponse
}

/// Maps a WebSocket error without a log of its content: an HTTP refusal contains the response headers and body.
fn transport_error(error: WsError) -> ProviderError {
    match error {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            tracing::warn!(status, "the provider refused the connection");
            match status {
                401 | 403 => ProviderError::Unauthorized,
                _ => ProviderError::Http(status),
            }
        }
        WsError::Io(_)
        | WsError::Tls(_)
        | WsError::ConnectionClosed
        | WsError::AlreadyClosed
        | WsError::WriteBufferFull(_)
        | WsError::Url(_)
        | WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake) => {
            tracing::warn!("the provider connection failed");
            ProviderError::Network
        }
        WsError::Protocol(_)
        | WsError::Capacity(_)
        | WsError::Utf8(_)
        | WsError::AttackAttempt
        | WsError::HttpFormat(_) => {
            tracing::warn!("the provider broke the WebSocket protocol");
            ProviderError::InvalidResponse
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::pin::pin;
    use std::sync::LazyLock;

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use bloop_core::domain::AudioChunk;
    use bloop_core::error::ProviderError;
    use bloop_core::ports::ai::{self, TranscriptionOptions, TranscriptionSession};
    use futures_util::{SinkExt as _, StreamExt as _};
    use serde_json::{Value, json};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    use tokio_tungstenite::tungstenite::http::StatusCode;
    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, accept_hdr_async};

    use super::{Session, TranscribeConfig, Transcriber};

    const KEY: &str = "test-key";
    const MODEL: &str = "voxtral-test";
    const DELAY_MS: u32 = 480;
    const TEXT: &str = "Bonjour à tous.";
    const RATE: NonZeroU32 = NonZeroU32::new(24_000).unwrap();

    static VOCABULARY: LazyLock<Vec<String>> =
        LazyLock::new(|| vec!["Bloop".to_owned(), "Voxtral".to_owned()]);

    /// The server side of a test connection.
    type Provider = WebSocketStream<TcpStream>;

    /// The parts of the handshake request that the fake provider records.
    struct Head {
        authorization: Vec<String>,
        query: Option<String>,
    }

    fn options() -> TranscriptionOptions<'static> {
        TranscriptionOptions {
            language: "fr",
            vocabulary: &VOCABULARY,
            sample_rate: RATE,
        }
    }

    /// Three chunks of audio, in capture order.
    fn chunks() -> Vec<AudioChunk> {
        let samples: Vec<f32> = (0..5_000_u16)
            .map(|index| f32::from(index) / 4_096.0)
            .collect();
        AudioChunk::split(&samples, RATE).collect()
    }

    /// Binds a fake provider and returns a transcriber that connects to it.
    async fn listen() -> (Transcriber, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let transcriber = Transcriber {
            url: format!("ws://{address}/v1/audio/transcriptions/realtime"),
            key: KEY.into(),
            config: TranscribeConfig {
                model: MODEL.to_owned(),
                target_streaming_delay_ms: DELAY_MS,
            },
        };
        (transcriber, listener)
    }

    /// Accepts one connection and runs `script` on it.
    /// The task returns the recorded handshake request and the result of `script`.
    #[expect(
        clippy::result_large_err,
        reason = "the tungstenite handshake callback sets the error type"
    )]
    async fn serve<T, F>(
        script: impl FnOnce(Provider) -> F + Send + 'static,
    ) -> (Transcriber, JoinHandle<(Head, T)>)
    where
        F: Future<Output = T> + Send,
        T: Send + 'static,
    {
        let (transcriber, listener) = listen().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut head = None;
            let provider = accept_hdr_async(
                stream,
                |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
                    head = Some(Head {
                        authorization: request
                            .headers()
                            .get_all(AUTHORIZATION)
                            .iter()
                            .map(|value| value.to_str().unwrap().to_owned())
                            .collect(),
                        query: request.uri().query().map(str::to_owned),
                    });
                    Ok(response)
                },
            )
            .await
            .unwrap();
            (head.unwrap(), script(provider).await)
        });
        (transcriber, server)
    }

    /// Refuses one connection with `status`.
    #[expect(
        clippy::result_large_err,
        reason = "the tungstenite handshake callback sets the error type"
    )]
    async fn refuse(status: StatusCode) -> Transcriber {
        let (transcriber, listener) = listen().await;
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept_hdr_async(
                stream,
                |_: &Request, _: Response| -> Result<Response, ErrorResponse> {
                    let mut refusal = ErrorResponse::new(None);
                    *refusal.status_mut() = status;
                    Err(refusal)
                },
            )
            .await
            .unwrap_err();
        });
        transcriber
    }

    async fn emit(provider: &mut Provider, event: &Value) {
        provider
            .send(Message::text(event.to_string()))
            .await
            .unwrap();
    }

    async fn receive(provider: &mut Provider) -> Value {
        let message = provider.next().await.unwrap().unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    /// Receives audio until `input_audio.end` and returns the decoded chunks.
    async fn receive_audio(provider: &mut Provider) -> Vec<Vec<u8>> {
        let mut audio = Vec::new();
        loop {
            let message = receive(provider).await;
            match message["type"].as_str() {
                Some("input_audio.append") => {
                    audio.push(STANDARD.decode(message["audio"].as_str().unwrap()).unwrap());
                }
                Some("input_audio.end") => return audio,
                other => panic!("unexpected message type {other:?}"),
            }
        }
    }

    /// Returns true when the client closed the connection without another message.
    async fn ended(provider: &mut Provider) -> bool {
        matches!(provider.next().await, None | Some(Err(_)))
    }

    /// Opens the session as the provider does.
    async fn setup(provider: &mut Provider) {
        emit(provider, &created()).await;
        assert_eq!(receive(provider).await["type"], "session.update");
        emit(provider, &json!({ "type": "session.notice" })).await;
        emit(provider, &updated()).await;
    }

    /// Waits until a provider message reaches the client, so that the next `send_audio` reads it.
    async fn arrived(session: &Session) {
        let MaybeTlsStream::Plain(socket) = session.ws.get_ref() else {
            panic!("the test connection does not use TLS");
        };
        socket.readable().await.unwrap();
    }

    fn created() -> Value {
        json!({ "type": "session.created", "session": { "language": null } })
    }

    fn updated() -> Value {
        json!({ "type": "session.updated", "session": { "language": "fr" } })
    }

    fn delta() -> Value {
        json!({ "type": "transcription.text.delta", "text": "Bonsoir" })
    }

    fn done() -> Value {
        json!({ "type": "transcription.done", "text": TEXT, "language": "fr" })
    }

    fn error() -> Value {
        json!({ "type": "error", "error": { "message": "Invalid session", "code": 4000 } })
    }

    #[tokio::test]
    async fn start_applies_the_options_once_and_waits_for_session_updated() {
        let (transcriber, server) = serve(|mut provider| async move {
            emit(&mut provider, &created()).await;
            let update = receive(&mut provider).await;
            (update, provider)
        })
        .await;

        let mut open = pin!(transcriber.open(options()));
        let (head, (update, mut provider)) = tokio::select! {
            biased;
            _ = &mut open => panic!("start ended before session.updated"),
            server = server => server.unwrap(),
        };
        emit(&mut provider, &updated()).await;
        drop(open.await.unwrap());

        assert_eq!(head.authorization, [format!("Bearer {KEY}")]);
        assert_eq!(head.query, Some(format!("model={MODEL}")));
        assert_eq!(
            update,
            json!({
                "type": "session.update",
                "session": {
                    "audio_format": {
                        "encoding": "pcm_s16le",
                        "sample_rate": 24_000,
                    },
                    "target_streaming_delay_ms": DELAY_MS,
                    "language": "fr",
                    "context_bias": ["Bloop", "Voxtral"],
                },
            })
        );
        assert!(ended(&mut provider).await);
    }

    #[tokio::test]
    async fn audio_arrives_in_order_and_finish_returns_the_done_text() {
        let chunks = chunks();
        let (transcriber, server) = serve(|mut provider| async move {
            setup(&mut provider).await;
            let audio = receive_audio(&mut provider).await;
            emit(&mut provider, &done()).await;
            audio
        })
        .await;

        let mut session = transcriber.open(options()).await.unwrap();
        for chunk in &chunks {
            session.send_audio(chunk).await.unwrap();
        }
        let transcript = Box::new(session).finish().await.unwrap();

        let (_, audio) = server.await.unwrap();
        let sent: Vec<&[u8]> = chunks.iter().map(AudioChunk::as_bytes).collect();
        assert_eq!(audio, sent);
        assert_eq!(transcript.as_str(), TEXT);
    }

    #[tokio::test]
    async fn deltas_during_audio_and_before_done_are_ignored() {
        let chunks = chunks();
        let (transcriber, server) = serve(|mut provider| async move {
            setup(&mut provider).await;
            receive(&mut provider).await;
            emit(&mut provider, &delta()).await;
            receive_audio(&mut provider).await;
            emit(&mut provider, &delta()).await;
            emit(&mut provider, &done()).await;
        })
        .await;

        let mut session = transcriber.open(options()).await.unwrap();
        session.send_audio(&chunks[0]).await.unwrap();
        arrived(&session).await;
        session.send_audio(&chunks[1]).await.unwrap();
        assert_eq!((session.deltas, session.delta_chars), (1, 7));
        let transcript = Box::new(session).finish().await.unwrap();

        server.await.unwrap();
        assert_eq!(transcript.as_str(), TEXT);
    }

    #[tokio::test]
    async fn a_refused_handshake_maps_the_status() {
        for (status, expected) in [
            (StatusCode::UNAUTHORIZED, ProviderError::Unauthorized),
            (StatusCode::FORBIDDEN, ProviderError::Unauthorized),
            (StatusCode::INTERNAL_SERVER_ERROR, ProviderError::Http(500)),
        ] {
            let transcriber = refuse(status).await;

            let result = ai::Transcriber::start(&transcriber, options()).await;

            assert_eq!(result.err(), Some(expected), "{status}");
        }
    }

    #[tokio::test]
    async fn an_error_event_instead_of_session_updated_fails_start() {
        let (transcriber, server) = serve(|mut provider| async move {
            emit(&mut provider, &created()).await;
            receive(&mut provider).await;
            emit(&mut provider, &error()).await;
        })
        .await;

        let result = transcriber.open(options()).await;

        server.await.unwrap();
        assert_eq!(result.err(), Some(ProviderError::InvalidResponse));
    }

    #[tokio::test]
    async fn an_error_event_during_audio_fails_the_next_send_audio() {
        let chunks = chunks();
        let (transcriber, server) = serve(|mut provider| async move {
            setup(&mut provider).await;
            receive(&mut provider).await;
            emit(&mut provider, &error()).await;
            ended(&mut provider).await
        })
        .await;

        let mut session = transcriber.open(options()).await.unwrap();
        session.send_audio(&chunks[0]).await.unwrap();
        arrived(&session).await;
        let result = session.send_audio(&chunks[1]).await;
        drop(session);

        assert_eq!(result, Err(ProviderError::InvalidResponse));
        assert!(server.await.unwrap().1, "the failed chunk was not sent");
    }

    #[tokio::test]
    async fn an_error_event_instead_of_done_fails_finish() {
        let (transcriber, server) = serve(|mut provider| async move {
            setup(&mut provider).await;
            receive_audio(&mut provider).await;
            emit(&mut provider, &error()).await;
        })
        .await;

        let session = transcriber.open(options()).await.unwrap();
        let result = Box::new(session).finish().await;

        server.await.unwrap();
        assert_eq!(result.err(), Some(ProviderError::InvalidResponse));
    }

    #[tokio::test]
    async fn a_lost_connection_fails_send_audio_with_network() {
        let (transcriber, server) = serve(|mut provider| async move {
            setup(&mut provider).await;
        })
        .await;

        let mut session = transcriber.open(options()).await.unwrap();
        server.await.unwrap();
        arrived(&session).await;
        let result = session.send_audio(&chunks()[0]).await;

        assert_eq!(result, Err(ProviderError::Network));
    }

    #[tokio::test]
    async fn a_closed_connection_fails_finish_with_network() {
        let (transcriber, server) = serve(|mut provider| async move {
            setup(&mut provider).await;
            provider.close(None).await.unwrap();
        })
        .await;

        let session = transcriber.open(options()).await.unwrap();
        server.await.unwrap();
        let result = Box::new(session).finish().await;

        assert_eq!(result.err(), Some(ProviderError::Network));
    }

    #[tokio::test]
    async fn an_unexpected_first_message_fails_start() {
        for first in [
            Message::text(updated().to_string()),
            Message::binary(vec![1, 2, 3]),
            Message::text("not JSON"),
        ] {
            let (transcriber, server) = serve(|mut provider| async move {
                provider.send(first).await.unwrap();
            })
            .await;

            let result = transcriber.open(options()).await;

            server.await.unwrap();
            assert_eq!(result.err(), Some(ProviderError::InvalidResponse));
        }
    }

    #[tokio::test]
    async fn dropping_the_session_during_audio_closes_the_connection() {
        let (transcriber, server) = serve(|mut provider| async move {
            setup(&mut provider).await;
            receive(&mut provider).await;
            ended(&mut provider).await
        })
        .await;

        let mut session = transcriber.open(options()).await.unwrap();
        session.send_audio(&chunks()[0]).await.unwrap();
        drop(session);

        assert!(server.await.unwrap().1);
    }
}
