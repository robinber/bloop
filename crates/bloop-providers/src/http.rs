//! The HTTP client of the chat adapters, and the conversion of HTTP failures to `ProviderError`.
//!
//! The logs contain the HTTP status and the failure category. They never contain a header or a body.

use std::error::Error;
use std::io;
use std::time::Duration;

use bloop_core::error::ProviderError;
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use hyper::body::Bytes;
use hyper::{Request, StatusCode, Version};
use hyper_tls::HttpsConnector;
use hyper_util::client::legacy::{self, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use native_tls::TlsConnector;
use serde::de::DeserializeOwned;

/// The time limit of the TCP connection. With several addresses, each address gets an equal part.
const CONNECT_LIMIT: Duration = Duration::from_secs(5);

/// The size limit of a response body. A chat reply of 2,048 tokens is much smaller.
pub(crate) const BODY_LIMIT: usize = 1024 * 1024;

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
    let (version, body) = tokio::time::timeout(client.limit, exchange(&client.inner, request))
        .await
        .map_err(|_| {
            tracing::warn!("the provider did not answer within the time limit");
            ProviderError::Timeout
        })??;
    let value = serde_json::from_slice(&body).map_err(|error| {
        tracing::warn!(
            category = ?error.classify(),
            "the adapter cannot decode the provider response"
        );
        ProviderError::InvalidResponse
    })?;
    Ok((version, value))
}

/// Sends `request` and reads the body of a successful response, with the HTTP version of the response.
async fn exchange(
    client: &legacy::Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    request: Request<Full<Bytes>>,
) -> Result<(Version, Bytes), ProviderError> {
    let response = client
        .request(request)
        .await
        .map_err(|error| request_error(&error))?;
    let version = response.version();
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(status));
    }
    let body = Limited::new(response.into_body(), BODY_LIMIT)
        .collect()
        .await
        .map_err(|error| body_error(&*error))?;
    Ok((version, body.to_bytes()))
}

fn status_error(status: StatusCode) -> ProviderError {
    let status = status.as_u16();
    tracing::warn!(status, "the provider answered with an HTTP error");
    match status {
        401 | 403 => ProviderError::Unauthorized,
        _ => ProviderError::Http(status),
    }
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

    use super::timed_out;

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
}
