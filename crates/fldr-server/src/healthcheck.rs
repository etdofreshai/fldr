//! `fldr-server --healthcheck`: a dependency-free probe for Docker's
//! `HEALTHCHECK`, since the runtime image has no curl.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// GET `http://127.0.0.1:{port}/health` and return whether it answered 200.
///
/// Speaks just enough HTTP/1.1 to read the status line; the connection is
/// closed by the server after the response (`Connection: close`).
pub async fn probe(port: u16, timeout: Duration) -> std::io::Result<bool> {
    let run = async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(
                b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nUser-Agent: fldr-healthcheck\r\n\r\n",
            )
            .await?;
        // The status line fits comfortably in the first read's worth of
        // bytes; keep reading until we have it or the peer closes.
        let mut buf = Vec::with_capacity(256);
        let mut chunk = [0u8; 256];
        while !buf.windows(2).any(|w| w == b"\r\n") {
            let n = stream.read(&mut chunk).await?;
            if n == 0 || buf.len() > 8192 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Ok(status_is_200(&buf))
    };
    tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "healthcheck timed out"))?
}

fn status_is_200(response: &[u8]) -> bool {
    let line = response.split(|&b| b == b'\r').next().unwrap_or_default();
    let mut parts = line.split(|&b| b == b' ');
    matches!(
        (parts.next(), parts.next()),
        (Some(v), Some(b"200")) if v.starts_with(b"HTTP/1.")
    )
}

#[cfg(test)]
mod tests {
    use super::status_is_200;

    #[test]
    fn parses_status_line() {
        assert!(status_is_200(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n"));
        assert!(!status_is_200(b"HTTP/1.1 503 Service Unavailable\r\n"));
        assert!(!status_is_200(b""));
        assert!(!status_is_200(b"garbage 200"));
    }
}
