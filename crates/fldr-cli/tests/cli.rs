//! End-to-end tests: run the real `fldr` binary against a mock axum server.

use std::net::{SocketAddr, TcpListener as StdListener};
use std::process::{Command, Output};

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use fldr_core::api::{ApiError, ComponentStatus, HealthResponse, ServiceInfo};

fn health_body(ok: bool) -> HealthResponse {
    HealthResponse {
        ok,
        service: "fldr-server".into(),
        version: "0.1.0".into(),
        git_sha: Some("abc1234".into()),
        db: if ok {
            ComponentStatus::Memory
        } else {
            ComponentStatus::Error
        },
        schema: Some(1),
        blobs: ComponentStatus::Ok,
        volume_id: Some("vol-42".into()),
        uptime_secs: 3_725,
    }
}

fn json(status: StatusCode, body: &impl serde::Serialize) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(body).unwrap(),
    )
        .into_response()
}

fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        &ApiError {
            code: "not_found".into(),
            message: "no such route".into(),
        },
    )
}

/// Which flavour of server the mock imitates.
#[derive(Clone, Copy)]
enum Mode {
    /// Healthy fldr server.
    Ok,
    /// `/health` reports not-ok with HTTP 503.
    Unhealthy,
    /// Every route is an unknown route (404 `ApiError`).
    NotFound,
    /// A proxy answering with an HTML error page.
    HtmlProxy,
}

/// Start a mock server on an ephemeral port in a background thread.
fn serve(mode: Mode) -> String {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let app = match mode {
        Mode::Ok => Router::new()
            .route(
                "/health",
                get(|| async { json(StatusCode::OK, &health_body(true)) }),
            )
            .route(
                "/v1",
                get(|| async {
                    json(
                        StatusCode::OK,
                        &ServiceInfo {
                            service: "fldr-server".into(),
                            version: "0.1.0".into(),
                            api_version: 1,
                            capabilities: vec!["health".into()],
                        },
                    )
                }),
            )
            .fallback(|| async { not_found() }),
        Mode::Unhealthy => Router::new().route(
            "/health",
            get(|| async { json(StatusCode::SERVICE_UNAVAILABLE, &health_body(false)) }),
        ),
        Mode::NotFound => Router::new().fallback(|| async { not_found() }),
        Mode::HtmlProxy => Router::new().fallback(|| async {
            (
                StatusCode::BAD_GATEWAY,
                [(header::CONTENT_TYPE, "text/html")],
                "<!DOCTYPE html>\n<html><title>502 Bad gateway</title></html>",
            )
        }),
    };
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    format!("http://{addr}/")
}

/// An address with nothing listening: bind, read the port, drop.
fn dead_host() -> String {
    let addr: SocketAddr = StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    format!("http://{addr}")
}

fn fldr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fldr"))
        .args(args)
        .env_remove("FLDR_HOST")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

#[test]
fn version() {
    let out = fldr(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout(&out).trim(),
        concat!("fldr ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn health_ok_human() {
    let host = serve(Mode::Ok);
    let out = fldr(&["--host", &host, "health"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    let base = host.trim_end_matches('/');
    assert!(
        s.starts_with(&format!("ok fldr-server 0.1.0 ({base})")),
        "{s}"
    );
    for needle in [
        "gitSha    abc1234",
        "db        memory",
        "schema    1",
        "blobs     ok",
        "volumeId  vol-42",
        "uptime    1h 2m 5s",
    ] {
        assert!(s.contains(needle), "missing {needle:?} in:\n{s}");
    }
}

#[test]
fn health_ok_json() {
    let host = serve(Mode::Ok);
    let out = fldr(&["health", "--json", "--host", &host]);
    assert_eq!(out.status.code(), Some(0));
    let parsed: HealthResponse = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(parsed, health_body(true));
    let raw: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(raw["volumeId"], "vol-42");
    assert_eq!(raw["uptimeSecs"], 3_725);
}

#[test]
fn health_unhealthy_exits_1() {
    let host = serve(Mode::Unhealthy);
    let out = fldr(&["--host", &host, "health"]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.starts_with("NOT OK fldr-server"), "{s}");
    assert!(s.contains("db        error"), "{s}");

    let out = fldr(&["--host", &host, "--json", "health"]);
    assert_eq!(out.status.code(), Some(1));
    let parsed: HealthResponse = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(!parsed.ok);
}

#[test]
fn health_unreachable_exits_2() {
    let out = fldr(&["--host", &dead_host(), "health"]);
    assert_eq!(out.status.code(), Some(2));
    let e = stderr(&out);
    assert!(e.contains("could not connect"), "{e}");
    assert!(e.contains("hint:"), "{e}");
    assert!(!e.contains("panicked"), "{e}");
}

#[test]
fn health_api_error_exits_2() {
    let host = serve(Mode::NotFound);
    let out = fldr(&["--host", &host, "health"]);
    assert_eq!(out.status.code(), Some(2));
    let e = stderr(&out);
    assert!(e.contains("404 Not Found"), "{e}");
    assert!(e.contains("no such route (not_found)"), "{e}");
}

#[test]
fn html_proxy_page_is_a_clear_error() {
    let host = serve(Mode::HtmlProxy);
    for cmd in ["health", "info"] {
        let out = fldr(&["--host", &host, cmd]);
        assert_eq!(out.status.code(), Some(2));
        let e = stderr(&out);
        assert!(e.contains("unexpected response"), "{e}");
        assert!(e.contains("HTTP 502 Bad Gateway, text/html"), "{e}");
        assert!(e.contains("body starts: <!DOCTYPE html>"), "{e}");
        assert!(!e.contains("panicked"), "{e}");
    }
}

#[test]
fn info_human_and_json() {
    let host = serve(Mode::Ok);
    let out = fldr(&["--host", &host, "info"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.starts_with("fldr-server 0.1.0 ("), "{s}");
    assert!(s.contains("apiVersion    1"), "{s}");
    assert!(s.contains("capabilities  health"), "{s}");

    let out = fldr(&["--host", &host, "--json", "info"]);
    assert_eq!(out.status.code(), Some(0));
    let raw: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(raw["apiVersion"], 1);
    assert_eq!(raw["capabilities"], serde_json::json!(["health"]));
}

#[test]
fn env_host_is_used() {
    let host = serve(Mode::Ok);
    let out = Command::new(env!("CARGO_BIN_EXE_fldr"))
        .arg("health")
        .env("FLDR_HOST", &host)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
}

#[test]
fn bad_host_scheme_is_rejected() {
    let out = fldr(&["--host", "ftp://fldr.example.com", "health"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("only http:// and https://"));
}

#[test]
fn planned_commands_exit_64() {
    for cmd in ["clone", "status", "commit", "log", "diff", "cat"] {
        let out = fldr(&[cmd, "--some-future-flag", "x"]);
        assert_eq!(out.status.code(), Some(64), "{cmd}");
        assert!(
            stderr(&out).contains("not implemented yet (planned for M4)"),
            "{cmd}"
        );
    }
}

#[test]
fn help_lists_planned_surface() {
    let s = stdout(&fldr(&["--help"]));
    for cmd in [
        "health", "info", "clone", "status", "commit", "log", "diff", "cat",
    ] {
        assert!(s.contains(&format!("  {cmd} ")), "missing {cmd} in:\n{s}");
    }
    assert!(s.contains("[planned]"));
}
