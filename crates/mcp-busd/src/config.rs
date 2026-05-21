//! Daemon configuration from env + CLI flags.

use clap::Parser;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

#[derive(Debug, Parser, Clone)]
#[command(name = "mcp-busd", about = "mcp-bus daemon")]
pub struct Config {
    /// Bind address. Refuses non-loopback values in v1.
    #[arg(long, env = "MCP_BUS_BIND", default_value = "127.0.0.1")]
    pub bind: IpAddr,

    #[arg(long, env = "MCP_BUS_PORT", default_value_t = 8765)]
    pub port: u16,

    /// Unix socket path for shim IPC.
    #[arg(long, env = "MCP_BUS_SOCKET")]
    pub socket: Option<PathBuf>,

    /// Event log path.
    #[arg(long, env = "MCP_BUS_LOG_PATH")]
    pub log_path: Option<PathBuf>,

    #[arg(long, env = "MCP_BUS_LOG_MAX_PAYLOAD", default_value_t = 65536)]
    pub max_payload: usize,

    #[arg(long, env = "MCP_BUS_INBOX_DIR")]
    pub inbox_dir: Option<PathBuf>,

    #[arg(long, env = "MCP_BUS_DEFAULT_TIMEOUT_MS", default_value_t = 30_000)]
    pub default_timeout_ms: u64,

    #[arg(long, env = "MCP_BUS_MAX_TIMEOUT_MS", default_value_t = 86_400_000)]
    pub max_timeout_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("bind must be a loopback address in v1; got {0}. Auth/TLS for non-loopback is future work.")]
    NonLoopbackBind(IpAddr),
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.bind {
            IpAddr::V4(v) if v == Ipv4Addr::LOCALHOST => Ok(()),
            IpAddr::V6(v) if v.is_loopback() => Ok(()),
            other => Err(ConfigError::NonLoopbackBind(other)),
        }
    }

    #[allow(dead_code)]
    pub fn resolved_socket(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(|| {
            let base = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            base.join("mcp-bus.sock")
        })
    }

    pub fn resolved_log_path(&self) -> PathBuf {
        self.log_path.clone().unwrap_or_else(|| {
            let base = std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let home = std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(std::env::temp_dir);
                    home.join(".local/state")
                });
            base.join("mcp-bus/events.jsonl")
        })
    }

    #[allow(dead_code)]
    pub fn resolved_inbox_dir(&self) -> PathBuf {
        self.inbox_dir.clone().unwrap_or_else(|| {
            let base = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            base.join("mcp-bus/inbox")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_v4_accepted() {
        let mut c = Config::parse_from(["mcp-busd"]);
        c.bind = "127.0.0.1".parse().unwrap();
        c.validate().unwrap();
    }

    #[test]
    fn non_loopback_rejected() {
        let mut c = Config::parse_from(["mcp-busd"]);
        c.bind = "0.0.0.0".parse().unwrap();
        let err = c.validate().unwrap_err();
        assert!(matches!(err, ConfigError::NonLoopbackBind(_)));
    }
}
