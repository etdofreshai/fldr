//! HTTP client for the fldr server, used by the `fldr` binary.
//!
//! The client is deliberately thin: it speaks the JSON wire types in
//! [`fldr_core::api`] and turns every failure mode (unreachable host, timeout,
//! a proxy's HTML error page, a structured [`ApiError`]) into a typed
//! [`Error`] so the binary can choose an exit code and a helpful message.

use std::fmt;
use std::time::Duration;

use fldr_core::api::{ApiError, HealthResponse, ServiceInfo};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

/// Host used when neither `--host` nor `FLDR_HOST` is given.
pub const DEFAULT_HOST: &str = "https://fldr.etdofresh.com";

/// Sent as `User-Agent` on every request.
pub const USER_AGENT: &str = concat!("fldr-cli/", env!("CARGO_PKG_VERSION"));

/// Default end-to-end request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Upper bound on connection establishment (TCP + TLS), whatever the total.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How much of an unexpected body to quote in error messages.
const SNIPPET_LEN: usize = 160;

/// Everything that can go wrong talking to a server.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The host URL was rejected before any request was made.
    #[error("invalid host {host:?}: {reason}")]
    InvalidHost { host: String, reason: String },
    /// Building the HTTP client failed (TLS backend initialisation).
    #[error("could not initialise HTTP client: {0}")]
    Client(#[source] reqwest::Error),
    /// The async runtime could not be started.
    #[error("could not start async runtime: {0}")]
    Runtime(#[source] std::io::Error),
    /// The server could not be reached at all.
    #[error("could not connect to {url}")]
    Connect {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// The request did not complete within the timeout.
    #[error("request to {url} timed out after {}s", timeout.as_secs_f64())]
    Timeout { url: String, timeout: Duration },
    /// Any other transport-level failure (reset connection, bad TLS, ...).
    #[error("request to {url} failed")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// The server answered with a structured [`ApiError`].
    #[error("server returned {status}: {} ({})", error.message, error.code)]
    Api { status: StatusCode, error: ApiError },
    /// The response was not the JSON we expected, typically an HTML error
    /// page from a proxy or something other than fldr listening on the host.
    #[error(
        "unexpected response from {url}: HTTP {status}, {content_type}, {problem}\n  body starts: {snippet}"
    )]
    Unexpected {
        url: String,
        status: StatusCode,
        content_type: String,
        problem: String,
        snippet: String,
    },
}

/// A decoded response together with its HTTP status and raw JSON body, so
/// callers can print the server's exact bytes with `--json`.
#[derive(Debug, Clone)]
pub struct Fetched<T> {
    pub status: StatusCode,
    pub value: T,
    pub raw: String,
}

/// Normalise a user-supplied host to `scheme://authority[/path]` with no
/// trailing slash. Only `http` and `https` are accepted; query strings and
/// fragments are rejected since every API path is appended to the host.
pub fn normalize_host(input: &str) -> Result<String, Error> {
    let invalid = |reason: &str| Error::InvalidHost {
        host: input.to_owned(),
        reason: reason.to_owned(),
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(invalid("host is empty"));
    }
    if !trimmed.contains("://") {
        return Err(invalid("missing scheme; use e.g. https://fldr.example.com"));
    }
    let url = reqwest::Url::parse(trimmed).map_err(|e| invalid(&e.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(invalid(&format!(
                "unsupported scheme {other:?}; only http:// and https:// are allowed"
            )));
        }
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid("missing host name"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("must not contain a query string or fragment"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("must not contain credentials"));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// A connection to one fldr server.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    host: String,
    timeout: Duration,
}

impl Client {
    /// Client for `host` (normalised with [`normalize_host`]) with the default
    /// timeout.
    pub fn new(host: &str) -> Result<Self, Error> {
        Self::with_timeout(host, DEFAULT_TIMEOUT)
    }

    /// Client for `host` with an end-to-end per-request `timeout`.
    pub fn with_timeout(host: &str, timeout: Duration) -> Result<Self, Error> {
        let host = normalize_host(host)?;
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .connect_timeout(CONNECT_TIMEOUT.min(timeout))
            .build()
            .map_err(Error::Client)?;
        Ok(Self {
            http,
            host,
            timeout,
        })
    }

    /// The normalised host, without a trailing slash.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// `GET /health`. A 503 that carries a [`HealthResponse`] is returned as
    /// `Ok` with `value.ok == false`, not as an error.
    pub async fn health(&self) -> Result<Fetched<HealthResponse>, Error> {
        self.get_json("/health", &[StatusCode::SERVICE_UNAVAILABLE])
            .await
    }

    /// `GET /v1`: service discovery.
    pub async fn info(&self) -> Result<Fetched<ServiceInfo>, Error> {
        self.get_json("/v1", &[]).await
    }

    /// GET `path` and decode a `T` from any 2xx response or from one of the
    /// `also_ok` statuses. Other statuses decode as [`ApiError`].
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        also_ok: &[StatusCode],
    ) -> Result<Fetched<T>, Error> {
        let url = format!("{}{path}", self.host);
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| self.transport_error(&url, e))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("no content-type")
            .to_owned();
        let raw = response
            .text()
            .await
            .map_err(|e| self.transport_error(&url, e))?;

        let unexpected = |problem: String| Error::Unexpected {
            url: url.clone(),
            status,
            content_type: content_type.clone(),
            problem,
            snippet: snippet(&raw),
        };

        if status.is_success() || also_ok.contains(&status) {
            match serde_json::from_str::<T>(&raw) {
                Ok(value) => return Ok(Fetched { status, value, raw }),
                // A non-2xx we tolerate may still be a plain ApiError.
                Err(_) if !status.is_success() => {}
                Err(e) => return Err(unexpected(format!("invalid JSON body: {e}"))),
            }
        }
        match serde_json::from_str::<ApiError>(&raw) {
            Ok(error) => Err(Error::Api { status, error }),
            Err(_) => Err(unexpected("body is not a fldr API response".to_owned())),
        }
    }

    fn transport_error(&self, url: &str, e: reqwest::Error) -> Error {
        let url = url.to_owned();
        if e.is_timeout() {
            Error::Timeout {
                url,
                timeout: self.timeout,
            }
        } else if e.is_connect() {
            Error::Connect { url, source: e }
        } else {
            Error::Transport { url, source: e }
        }
    }
}

/// First [`SNIPPET_LEN`] characters of `body` on one line, for messages.
fn snippet(body: &str) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return "(empty)".to_owned();
    }
    let mut out: String = flat.chars().take(SNIPPET_LEN).collect();
    if flat.chars().count() > SNIPPET_LEN {
        out.push('…');
    }
    out
}

/// Format a duration in seconds as e.g. `3d 4h 5m 6s`, dropping leading
/// zero units.
pub fn format_uptime(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3_600 % 24, secs / 60 % 60, secs % 60);
    let parts = [(d, "d"), (h, "h"), (m, "m"), (s, "s")];
    let first = parts.iter().position(|(n, _)| *n > 0).unwrap_or(3);
    parts[first..]
        .iter()
        .map(|(n, unit)| format!("{n}{unit}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Display adapter for an optional value: the value or `-`.
pub struct OrDash<'a, T>(pub &'a Option<T>);

impl<T: fmt::Display> fmt::Display for OrDash<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(v) => v.fmt(f),
            None => f.write_str("-"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> String {
        normalize_host(input).unwrap()
    }

    fn err(input: &str) -> String {
        normalize_host(input).unwrap_err().to_string()
    }

    #[test]
    fn strips_trailing_slashes() {
        assert_eq!(
            ok("https://fldr.etdofresh.com"),
            "https://fldr.etdofresh.com"
        );
        assert_eq!(
            ok("https://fldr.etdofresh.com/"),
            "https://fldr.etdofresh.com"
        );
        assert_eq!(ok("http://127.0.0.1:8080///"), "http://127.0.0.1:8080");
        assert_eq!(ok("  https://h.example/api/ "), "https://h.example/api");
    }

    #[test]
    fn canonicalises_case_and_default_port() {
        assert_eq!(
            ok("HTTPS://Fldr.Example.COM:443/"),
            "https://fldr.example.com"
        );
        assert_eq!(ok("http://[::1]:9000"), "http://[::1]:9000");
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(err("ftp://fldr.example.com").contains("only http:// and https://"));
        assert!(err("file:///tmp/x").contains("unsupported scheme"));
    }

    #[test]
    fn rejects_missing_scheme_and_empty() {
        assert!(err("fldr.example.com").contains("missing scheme"));
        assert!(err("   ").contains("empty"));
    }

    #[test]
    fn rejects_query_fragment_and_credentials() {
        assert!(err("https://h.example/?x=1").contains("query"));
        assert!(err("https://h.example/#top").contains("fragment"));
        assert!(err("https://user:pw@h.example").contains("credentials"));
    }

    #[test]
    fn uptime_formatting() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(59), "59s");
        assert_eq!(format_uptime(3_600), "1h 0m 0s");
        assert_eq!(format_uptime(90_061), "1d 1h 1m 1s");
    }

    #[test]
    fn snippet_is_flat_and_bounded() {
        assert_eq!(snippet(""), "(empty)");
        assert_eq!(
            snippet("<html>\n  <b>502</b>\n</html>"),
            "<html> <b>502</b> </html>"
        );
        let long = "x".repeat(500);
        assert_eq!(snippet(&long).chars().count(), SNIPPET_LEN + 1);
    }
}
