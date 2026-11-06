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

use super::{LOG_EXCERPT, ServerEvent, Session, Text, TranscribeConfig, Transcriber, excerpt};

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
async fn deltas_with_a_null_text_are_counted_and_do_not_fail_the_session() {
    let chunks = chunks();
    let null_delta = json!({ "type": "transcription.text.delta", "text": null });
    let (transcriber, server) = serve(|mut provider| async move {
        setup(&mut provider).await;
        receive(&mut provider).await;
        emit(&mut provider, &null_delta).await;
        receive_audio(&mut provider).await;
        emit(&mut provider, &null_delta).await;
        emit(&mut provider, &done()).await;
    })
    .await;

    let mut session = transcriber.open(options()).await.unwrap();
    session.send_audio(&chunks[0]).await.unwrap();
    arrived(&session).await;
    session.send_audio(&chunks[1]).await.unwrap();
    assert_eq!((session.deltas, session.delta_chars), (1, 0));
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

#[test]
fn the_debug_of_text_shows_the_number_of_characters_and_not_the_text() {
    let input = "Été à Noël";

    let output = format!("{:?}", Text(input.to_owned()));

    assert_eq!(output, "Text { chars: 10 }");
    assert!(!output.contains(input));
}

#[test]
fn the_debug_of_a_delta_event_shows_no_text() {
    let event: ServerEvent = serde_json::from_value(delta()).unwrap();

    let output = format!("{event:?}");

    assert!(output.starts_with("Delta"), "{output}");
    assert!(!output.contains("Bonsoir"), "{output}");
}

#[test]
fn an_excerpt_ends_at_the_last_char_boundary_before_the_limit() {
    let long = format!("a{}", "é".repeat(150));
    assert!(!long.is_char_boundary(LOG_EXCERPT));

    assert_eq!(excerpt(&long), format!("a{}", "é".repeat(99)));
    assert_eq!(excerpt("Invalid session"), "Invalid session");
}
