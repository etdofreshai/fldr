//! Server configuration from environment variables.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

/// Log output format, from `FLDR_LOG_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable lines; the default.
    Pretty,
    /// One JSON object per line, for log collectors (Docker sets this).
    Json,
}

/// Runtime configuration. See [`Config::from_env`] for the variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    /// Postgres URL. `None` selects the in-memory metadata store.
    pub database_url: Option<String>,
    /// Root of persistent local state; blobs live in `blobs/` below it.
    pub data_dir: PathBuf,
    pub log_format: LogFormat,
}

/// Default `PORT`.
pub const DEFAULT_PORT: u16 = 8080;
/// Default `DATA_DIR` for local runs.
pub const DEFAULT_DATA_DIR: &str = "./.fldr-data";

/// An environment variable has an unusable value.
#[derive(Debug, thiserror::Error)]
#[error("invalid {var}={value:?}: {reason}")]
pub struct ConfigError {
    pub var: &'static str,
    pub value: String,
    pub reason: String,
}

impl Config {
    /// Read `PORT`, `BIND`, `DATABASE_URL`, `DATA_DIR` and `FLDR_LOG_FORMAT`.
    /// Unset or empty variables take their defaults. `RUST_LOG` is read by
    /// the tracing subscriber, not here.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Like [`Config::from_env`] but reading variables through `get`.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let invalid = |var, value: &str, reason: &dyn std::fmt::Display| ConfigError {
            var,
            value: value.to_owned(),
            reason: reason.to_string(),
        };

        let port = match get("PORT") {
            None => DEFAULT_PORT,
            Some(v) => v.parse().map_err(|e| invalid("PORT", &v, &e))?,
        };
        let bind = match get("BIND") {
            None => IpAddr::from([0, 0, 0, 0]),
            Some(v) => v.parse().map_err(|e| invalid("BIND", &v, &e))?,
        };
        let log_format = match get("FLDR_LOG_FORMAT").as_deref() {
            None | Some("pretty") => LogFormat::Pretty,
            Some("json") => LogFormat::Json,
            Some(v) => return Err(invalid("FLDR_LOG_FORMAT", v, &"expected json or pretty")),
        };

        Ok(Self {
            bind,
            port,
            database_url: get("DATABASE_URL"),
            data_dir: PathBuf::from(get("DATA_DIR").unwrap_or_else(|| DEFAULT_DATA_DIR.into())),
            log_format,
        })
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.socket_addr().to_string(), "0.0.0.0:8080");
        assert_eq!(c.database_url, None);
        assert_eq!(c.data_dir, PathBuf::from(DEFAULT_DATA_DIR));
        assert_eq!(c.log_format, LogFormat::Pretty);
    }

    #[test]
    fn overrides_and_empty_values() {
        let c = cfg(&[
            ("PORT", "9000"),
            ("BIND", "127.0.0.1"),
            ("DATABASE_URL", ""),
            ("DATA_DIR", "/data"),
            ("FLDR_LOG_FORMAT", "json"),
        ])
        .unwrap();
        assert_eq!(c.socket_addr().to_string(), "127.0.0.1:9000");
        assert_eq!(c.database_url, None);
        assert_eq!(c.data_dir, PathBuf::from("/data"));
        assert_eq!(c.log_format, LogFormat::Json);
    }

    #[test]
    fn rejects_bad_values() {
        assert_eq!(cfg(&[("PORT", "http")]).unwrap_err().var, "PORT");
        assert_eq!(cfg(&[("BIND", "localhost")]).unwrap_err().var, "BIND");
        assert_eq!(
            cfg(&[("FLDR_LOG_FORMAT", "xml")]).unwrap_err().var,
            "FLDR_LOG_FORMAT"
        );
    }
}
