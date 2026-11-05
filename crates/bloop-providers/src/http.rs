//! The HTTP client of the chat adapters, and the conversion of HTTP failures to `ProviderError`.
//!
//! The logs contain the HTTP status, the failure category, and the `type` and `code` fields of a provider error body.
//! They never contain a header, a body, or another field of an error body.

use std::error::Error;
use std::io;
use std::time::Duration;

use bloop_core::error::ProviderError;
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, StatusCode, Version};
use hyper_tls::HttpsConnector;
use hyper_util::client::legacy::{self, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use native_tls::TlsConnector;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::time::{Instant, timeout_at};

/// The time limit of the TCP connection. With several addresses, each address gets an equal part.
const CONNECT_LIMIT: Duration = Duration::from_secs(5);

/// The size limit of a response body. A chat reply of 2,048 tokens is much smaller.
pub(crate) const BODY_LIMIT: usize = 1024 * 1024;

/// The size limit of an error body. The client reads only its `type` and `code` fields.
const ERROR_BODY_LIMIT: usize = 16 * 1024;

/// The length limit of a `type` or `code` value in the log.
const FIELD_LIMIT: usize = 64;

/// The protocols that the TLS handshake offers through ALPN, in the order of preference.
const PROTOCOLS: [&str; 2] = ["h2", "http/1.1"];

/// A client with native TLS. It uses HTTP/2 when the server selects it through ALPN, and HTTP/1.1 otherwise.
/// Its requests share the connection pool.
#[derive(Debug)]
pub(crate) struct Client {
    inner: legacy::Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    limit: Duration,
}

/// Builds a client. `limit` applies to each request, from the connection start to the end of the response body.
pub(crate) fn client(limit: Duration) -> Result<Client, ProviderError> {
    let tls = TlsConnector::builder()
        .request_alpns(&PROTOCOLS)
        .build()
        .map_err(|error| {
            tracing::warn!(?error, "the HTTP client did not start");
            ProviderError::Network
        })?;
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(CONNECT_LIMIT));
    http.set_nodelay(true);
    let inner = legacy::Client::builder(TokioExecutor::new())
        .build(HttpsConnector::from((http, tls.into())));
    Ok(Client { inner, limit })
}

/// Sends `request` with `client` and decodes the JSON body of a successful response.
/// It also returns the HTTP version of the response.
pub(crate) async fn send<T: DeserializeOwned>(
    client: &Client,
    request: Request<Full<Bytes>>,
) -> Result<(Version, T), ProviderError> {
    let deadline = Instant::now() + client.limit;
    let (version, body) = exchange(&client.inner, request, deadline).await?;
    let value = serde_json::from_slice(&body).map_err(|error| {
        tracing::warn!(
            category = ?error.classify(),
            "the adapter cannot decode the provider response"
        );
        ProviderError::InvalidResponse
    })?;
    Ok((version, value))
}

/// Sends `request` and reads the body of a successful response before `deadline`, with the HTTP version of the response.
/// An HTTP error gives its status error, also when its body does not arrive before `deadline`.
async fn exchange(
    client: &legacy::Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    request: Request<Full<Bytes>>,
    deadline: Instant,
) -> Result<(Version, Bytes), ProviderError> {
    let response = timeout_at(deadline, client.request(request))
        .await
        .map_err(|_| expired())?
        .map_err(|error| request_error(&error))?;
    let version = response.version();
    let status = response.status();
    if !status.is_success() {
        let detail = timeout_at(deadline, read_error(response.into_body()))
            .await
            .unwrap_or_default();
        return Err(status_error(status, &detail));
    }
    let body = timeout_at(
        deadline,
        Limited::new(response.into_body(), BODY_LIMIT).collect(),
    )
    .await
    .map_err(|_| expired())?
    .map_err(|error| body_error(&*error))?;
    Ok((version, body.to_bytes()))
}

fn expired() -> ProviderError {
    tracing::warn!("the provider did not answer within the time limit");
    ProviderError::Timeout
}

fn status_error(status: StatusCode, detail: &ErrorDetail) -> ProviderError {
    let status = status.as_u16();
    tracing::warn!(
        status,
        "type" = detail.kind.as_deref(),
        code = detail.code.as_deref(),
        "the provider answered with an HTTP error"
    );
    match status {
        401 | 403 => ProviderError::Unauthorized,
        _ => ProviderError::Http(status),
    }
}

/// The fields of a provider error body that the log can contain.
#[derive(Debug, Default, PartialEq, Eq)]
struct ErrorDetail {
    kind: Option<String>,
    code: Option<String>,
}

/// Reads an error body. A failed read, or a body larger than `ERROR_BODY_LIMIT`, gives no field.
async fn read_error(body: Incoming) -> ErrorDetail {
    match Limited::new(body, ERROR_BODY_LIMIT).collect().await {
        Ok(body) => error_detail(&body.to_bytes()),
        Err(_) => ErrorDetail::default(),
    }
}

/// Gets the string `type` and the string or number `code` of a JSON object.
/// It ignores all other fields, because a `message` can quote the request.
/// A body that is not a JSON object gives no field. A value that is not a short token gives no value.
fn error_detail(body: &[u8]) -> ErrorDetail {
    let Ok(object) = serde_json::from_slice::<Map<String, Value>>(body) else {
        return ErrorDetail::default();
    };
    let kind = match object.get("type") {
        Some(Value::String(kind)) => token(kind),
        _ => None,
    };
    let code = match object.get("code") {
        Some(Value::String(code)) => token(code),
        Some(Value::Number(code)) => token(&code.to_string()),
        _ => None,
    };
    ErrorDetail { kind, code }
}

/// Returns `value` when it has 1 to `FIELD_LIMIT` ASCII letters, digits, `_`, `-`, or `.`.
fn token(value: &str) -> Option<String> {
    let valid = (1..=FIELD_LIMIT).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    valid.then(|| value.to_owned())
}

/// Maps a request failure without a log of its content.
fn request_error(error: &legacy::Error) -> ProviderError {
    if timed_out(error) {
        tracing::warn!("the connection to the provider did not open within the time limit");
        ProviderError::Timeout
    } else if error.is_connect() {
        tracing::warn!("the connection to the provider failed");
        ProviderError::Network
    } else {
        tracing::warn!("the provider request failed");
        ProviderError::Network
    }
}

/// Maps a failure of the body read without a log of its content.
fn body_error(error: &(dyn Error + Send + Sync + 'static)) -> ProviderError {
    if error.is::<LengthLimitError>() {
        tracing::warn!(
            limit = BODY_LIMIT,
            "the provider response is larger than the limit"
        );
        ProviderError::InvalidResponse
    } else {
        tracing::warn!("the provider request failed");
        ProviderError::Network
    }
}

/// Returns true when `error` or one of its sources is an I/O time-out, for example the expired TCP connection limit.
fn timed_out(error: &(dyn Error + 'static)) -> bool {
    std::iter::successors(Some(error), |&error| error.source()).any(|error| {
        error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::TimedOut)
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;
    use std::io;

    use super::{ErrorDetail, error_detail, timed_out};

    const MISTRAL_429: &str = r#"{"object":"error","message":"Rate limit exceeded","type":"rate_limited","param":null,"code":"1300","raw_status_code":429}"#;

    fn detail(kind: Option<&str>, code: Option<&str>) -> ErrorDetail {
        ErrorDetail {
            kind: kind.map(str::to_owned),
            code: code.map(str::to_owned),
        }
    }

    /// An error with an I/O source, as the connector returns it.
    #[derive(Debug)]
    struct Connect(io::Error);

    impl fmt::Display for Connect {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("tcp connect error")
        }
    }

    impl Error for Connect {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn only_a_timed_out_source_is_a_time_out() {
        let expired = Connect(io::Error::from(io::ErrorKind::TimedOut));
        let refused = Connect(io::Error::from(io::ErrorKind::ConnectionRefused));

        assert!(timed_out(&expired));
        assert!(!timed_out(&refused));
    }

    #[test]
    fn a_mistral_error_gives_the_type_and_the_string_code_and_no_other_field() {
        let result = error_detail(MISTRAL_429.as_bytes());

        assert_eq!(result, detail(Some("rate_limited"), Some("1300")));
    }

    #[test]
    fn a_number_code_gives_its_decimal_text() {
        let result = error_detail(br#"{"type":"invalid_request","code":3051}"#);

        assert_eq!(result, detail(Some("invalid_request"), Some("3051")));
    }

    #[test]
    fn missing_or_null_fields_give_no_value() {
        for body in [
            "{}",
            r#"{"message":"Rate limit exceeded"}"#,
            r#"{"type":null,"code":null}"#,
        ] {
            assert_eq!(
                error_detail(body.as_bytes()),
                ErrorDetail::default(),
                "{body}"
            );
        }
        assert_eq!(
            error_detail(br#"{"code":"1300"}"#),
            detail(None, Some("1300"))
        );
    }

    #[test]
    fn a_body_that_is_not_a_json_object_gives_no_field() {
        for body in [
            "",
            "not JSON",
            "Too Many Requests",
            r#"{"type":"rate_limited""#,
            r#"["rate_limited","1300"]"#,
            r#""rate_limited""#,
        ] {
            assert_eq!(
                error_detail(body.as_bytes()),
                ErrorDetail::default(),
                "{body}"
            );
        }
    }

    #[test]
    fn a_value_that_is_not_a_short_token_gives_no_value() {
        let long = "a".repeat(65);
        for body in [
            r#"{"type":"bonjour a tous","code":"1300 x"}"#.to_owned(),
            r#"{"type":"line\nbreak","code":""}"#.to_owned(),
            r#"{"type":1300,"code":true}"#.to_owned(),
            r#"{"type":{"text":"bonjour"},"code":["1300"]}"#.to_owned(),
            format!(r#"{{"type":"{long}","code":"{long}"}}"#),
        ] {
            assert_eq!(
                error_detail(body.as_bytes()),
                ErrorDetail::default(),
                "{body}"
            );
        }
        let longest = "a".repeat(64);
        assert_eq!(
            error_detail(format!(r#"{{"type":"{longest}","code":"a.b-c_1"}}"#).as_bytes()),
            detail(Some(&longest), Some("a.b-c_1"))
        );
    }
}
