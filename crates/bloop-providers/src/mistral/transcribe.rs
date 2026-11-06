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
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// The largest part of a provider string that a log contains, in bytes.
const LOG_EXCERPT: usize = 200;

/// The largest number of provider messages that one `send_audio` reads before it sends the audio.
/// Thus a flood of provider events cannot keep the send synchronous and block its time limit.
const DRAIN_LIMIT: usize = 64;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The Voxtral realtime transcriber. Each session opens its own connection.
#[derive(Debug)]
pub(crate) struct Transcriber {
    url: String,
    key: SecretString,
    config: TranscribeConfig,
}

impl Transcriber {
    /// Creates a transcriber for the Mistral API.
    #[must_use]
    pub(crate) fn new(key: SecretString, config: TranscribeConfig) -> Self {
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
        let update = ClientEvent::SessionUpdate {
            session: SessionSettings {
                audio_format: AudioFormat {
                    encoding: "pcm_s16le",
                    sample_rate: options.sample_rate.get(),
                },
                target_streaming_delay_ms: self.config.target_streaming_delay_ms,
                language: options.language,
                context_bias: options.vocabulary,
            },
        };
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
    fn count(&mut self, delta: Option<&Text>) {
        self.deltas += 1;
        self.delta_chars += delta.map_or(0, |delta| delta.0.chars().count());
    }

    /// Reads the provider events that already arrived, without waiting for more.
    /// It reads at most `DRAIN_LIMIT` messages. The next call or `finish` reads the others.
    fn drain(&mut self) -> Result<(), ProviderError> {
        for _ in 0..DRAIN_LIMIT {
            let Some(item) = self.ws.next().now_or_never() else {
                break;
            };
            match decode(item)? {
                None => {}
                Some(ServerEvent::Delta { text }) => self.count(text.as_ref()),
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
            let append = ClientEvent::AudioAppend {
                audio: STANDARD.encode(chunk.as_bytes()),
            };
            send(&mut self.ws, &append).await
        })
    }

    fn finish(mut self: Box<Self>) -> BoxFuture<'static, Result<Transcript, ProviderError>> {
        Box::pin(async move {
            send(&mut self.ws, &ClientEvent::AudioEnd).await?;
            let text = loop {
                match read(&mut self.ws).await? {
                    ServerEvent::Done { text } => break text,
                    ServerEvent::Delta { text } => self.count(text.as_ref()),
                    event => return Err(unexpected(&event)),
                }
            };
            tracing::info!(
                deltas = self.deltas,
                delta_chars = self.delta_chars,
                "the provider sent the transcript"
            );
            // The transcript never waits for the close: the close frame gets one try, and the
            // drop of the stream closes the socket.
            if !matches!(self.ws.close(None).now_or_never(), Some(Ok(()))) {
                tracing::debug!("the provider connection did not close cleanly");
            }
            Ok(Transcript::new(text.0))
        })
    }
}

/// An event that the adapter sends to the provider.
#[derive(Serialize)]
#[serde(tag = "type")]
enum ClientEvent<'a> {
    #[serde(rename = "session.update")]
    SessionUpdate { session: SessionSettings<'a> },
    #[serde(rename = "input_audio.append")]
    AudioAppend { audio: String },
    #[serde(rename = "input_audio.end")]
    AudioEnd,
}

/// The settings of a session. The provider fixes them at the first audio chunk.
#[derive(Serialize)]
struct SessionSettings<'a> {
    audio_format: AudioFormat,
    target_streaming_delay_ms: u32,
    language: &'a str,
    context_bias: &'a [String],
}

#[derive(Serialize)]
struct AudioFormat {
    encoding: &'static str,
    sample_rate: u32,
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
        text: Option<Text>,
    },
    #[serde(rename = "transcription.done")]
    Done { text: Text },
    #[serde(rename = "error")]
    Error { error: ErrorBody },
    #[serde(other)]
    Other,
}

/// Text from the provider. `Debug` shows only the number of characters.
#[derive(Deserialize)]
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

async fn send(ws: &mut Socket, event: &ClientEvent<'_>) -> Result<(), ProviderError> {
    let text = serde_json::to_string(event).map_err(|error| {
        tracing::error!(category = ?error.classify(), "the adapter cannot encode its event");
        ProviderError::InvalidResponse
    })?;
    ws.send(Message::text(text)).await.map_err(transport_error)
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
                    kind = ?excerpt(kind),
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
        let code = error.code.to_string();
        let message = error.message.to_string();
        tracing::warn!(
            code = %excerpt(&code),
            message = %excerpt(&message),
            "the provider sent an error"
        );
    } else {
        tracing::warn!(?event, "the provider sent an unexpected event");
    }
    ProviderError::InvalidResponse
}

/// Returns the start of `text`: at most `LOG_EXCERPT` bytes, cut at a char boundary.
fn excerpt(text: &str) -> &str {
    &text[..text.floor_char_boundary(LOG_EXCERPT)]
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
mod tests;
