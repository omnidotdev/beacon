//! Individual diagnostic checks for `beacon doctor`

use crate::Config;

use super::{CheckResult, CheckStatus};

/// Check that config.toml is parseable
#[must_use]
pub fn config_parseable() -> CheckResult {
    let file_config = crate::config::file::load_config_file();
    let path = crate::config::file::config_file_path()
        .map_or_else(|| "unknown".to_string(), |p| p.display().to_string());

    if file_config.persona.is_some() || file_config.llm.model.is_some() {
        CheckResult {
            name: "ConfigParseable",
            status: CheckStatus::Ok,
            message: format!("config loaded from {path}"),
            suggestion: None,
        }
    } else {
        CheckResult {
            name: "ConfigParseable",
            status: CheckStatus::Ok,
            message: format!("config at {path} (using defaults)"),
            suggestion: None,
        }
    }
}

/// Check Synapse connectivity
#[must_use]
pub async fn synapse_connectivity(config: &Config) -> CheckResult {
    let url = &config.synapse_url;

    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    else {
        return CheckResult {
            name: "SynapseConnectivity",
            status: CheckStatus::Error,
            message: "failed to create HTTP client".to_string(),
            suggestion: None,
        };
    };

    let health_url = format!("{url}/health");
    match client.get(&health_url).send().await {
        Ok(resp) if resp.status().is_success() => CheckResult {
            name: "SynapseConnectivity",
            status: CheckStatus::Ok,
            message: format!("Synapse reachable at {url}"),
            suggestion: None,
        },
        Ok(resp) => CheckResult {
            name: "SynapseConnectivity",
            status: CheckStatus::Warning,
            message: format!("Synapse returned {} at {url}", resp.status()),
            suggestion: Some("check Synapse logs".to_string()),
        },
        Err(e) if config.cloud_mode => CheckResult {
            name: "SynapseConnectivity",
            status: CheckStatus::Error,
            message: format!("cannot reach Synapse at {url}: {e}"),
            suggestion: Some("verify SYNAPSE_URL or network connectivity".to_string()),
        },
        Err(_) => CheckResult {
            name: "SynapseConnectivity",
            status: CheckStatus::Warning,
            message: format!("Synapse not reachable at {url} (embedded mode may start it)"),
            suggestion: None,
        },
    }
}

/// Check `SQLite` database integrity
#[must_use]
pub fn sqlite_health(config: &Config) -> CheckResult {
    let db_path = config.data_dir.join("beacon.db");

    if !db_path.exists() {
        return CheckResult {
            name: "SqliteHealth",
            status: CheckStatus::Warning,
            message: "database file not found (will be created on first run)".to_string(),
            suggestion: None,
        };
    }

    match rusqlite::Connection::open(&db_path) {
        Ok(conn) => {
            match conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0)) {
                Ok(ref result) if result == "ok" => CheckResult {
                    name: "SqliteHealth",
                    status: CheckStatus::Ok,
                    message: format!("database at {} is healthy", db_path.display()),
                    suggestion: None,
                },
                Ok(result) => CheckResult {
                    name: "SqliteHealth",
                    status: CheckStatus::Error,
                    message: format!("integrity check failed: {result}"),
                    suggestion: Some(
                        "consider running `beacon backup create` then restoring".to_string(),
                    ),
                },
                Err(e) => CheckResult {
                    name: "SqliteHealth",
                    status: CheckStatus::Error,
                    message: format!("integrity check error: {e}"),
                    suggestion: None,
                },
            }
        }
        Err(e) => CheckResult {
            name: "SqliteHealth",
            status: CheckStatus::Error,
            message: format!("cannot open database: {e}"),
            suggestion: None,
        },
    }
}

/// Check if embeddings API key is available
#[must_use]
pub fn embeddings_available() -> CheckResult {
    if std::env::var("OPENAI_API_KEY").is_ok() {
        CheckResult {
            name: "EmbeddingsAvailable",
            status: CheckStatus::Ok,
            message: "OPENAI_API_KEY set (embeddings enabled)".to_string(),
            suggestion: None,
        }
    } else {
        CheckResult {
            name: "EmbeddingsAvailable",
            status: CheckStatus::Warning,
            message: "OPENAI_API_KEY not set (semantic memory disabled)".to_string(),
            suggestion: Some("set OPENAI_API_KEY for memory and knowledge features".to_string()),
        }
    }
}

/// Check channel authentication
#[must_use]
pub fn channel_auth(config: &Config) -> CheckResult {
    let mut issues = Vec::new();

    if config.telegram.is_some() && std::env::var("TELEGRAM_BOT_TOKEN").is_err() {
        issues.push("Telegram configured but TELEGRAM_BOT_TOKEN not set");
    }

    if issues.is_empty() {
        CheckResult {
            name: "ChannelAuth",
            status: CheckStatus::Ok,
            message: "channel credentials look good".to_string(),
            suggestion: None,
        }
    } else {
        CheckResult {
            name: "ChannelAuth",
            status: CheckStatus::Warning,
            message: issues.join("; "),
            suggestion: Some("set missing env vars for channels you want to use".to_string()),
        }
    }
}

/// Check MCP server availability
#[must_use]
pub fn mcp_servers(config: &Config) -> CheckResult {
    if config.mcp_servers.is_empty() {
        return CheckResult {
            name: "McpServers",
            status: CheckStatus::Ok,
            message: "no MCP servers configured".to_string(),
            suggestion: None,
        };
    }

    let count = config.mcp_servers.len();
    CheckResult {
        name: "McpServers",
        status: CheckStatus::Ok,
        message: format!("{count} MCP server(s) configured"),
        suggestion: None,
    }
}

/// Check sandbox runtime availability
#[must_use]
pub fn sandbox_runtime(config: &Config) -> CheckResult {
    if config.sandbox.is_none() {
        return CheckResult {
            name: "SandboxRuntime",
            status: CheckStatus::Ok,
            message: "sandbox not configured (shell runs uncontained)".to_string(),
            suggestion: None,
        };
    }

    let has_docker = which::which("docker").is_ok();
    let has_podman = which::which("podman").is_ok();

    if has_docker || has_podman {
        let engine = if has_docker { "Docker" } else { "Podman" };
        CheckResult {
            name: "SandboxRuntime",
            status: CheckStatus::Ok,
            message: format!("{engine} available for sandbox execution"),
            suggestion: None,
        }
    } else {
        CheckResult {
            name: "SandboxRuntime",
            status: CheckStatus::Error,
            message: "sandbox configured but neither Docker nor Podman found".to_string(),
            suggestion: Some("install Docker or Podman, or disable sandbox in config".to_string()),
        }
    }
}

/// Check available disk space in data directory
#[must_use]
pub fn disk_space(config: &Config) -> CheckResult {
    let data_dir = &config.data_dir;

    #[cfg(unix)]
    {
        let path = if data_dir.exists() {
            data_dir.clone()
        } else {
            data_dir.parent().map_or_else(
                || std::path::PathBuf::from("/"),
                std::path::Path::to_path_buf,
            )
        };

        match tempfile::tempfile_in(&path) {
            Ok(_) => CheckResult {
                name: "DiskSpace",
                status: CheckStatus::Ok,
                message: format!("data directory writable: {}", data_dir.display()),
                suggestion: None,
            },
            Err(_) => CheckResult {
                name: "DiskSpace",
                status: CheckStatus::Warning,
                message: format!("data directory may not be writable: {}", data_dir.display()),
                suggestion: Some("check permissions on data directory".to_string()),
            },
        }
    }

    #[cfg(not(unix))]
    {
        CheckResult {
            name: "DiskSpace",
            status: CheckStatus::Ok,
            message: format!("data directory: {}", data_dir.display()),
            suggestion: None,
        }
    }
}

/// Check system service status
#[must_use]
pub fn service_status() -> CheckResult {
    match crate::lifecycle::service_status() {
        Ok(crate::lifecycle::ServiceStatus::Running) => CheckResult {
            name: "ServiceStatus",
            status: CheckStatus::Ok,
            message: "service: running".to_string(),
            suggestion: None,
        },
        Ok(crate::lifecycle::ServiceStatus::Stopped) => CheckResult {
            name: "ServiceStatus",
            status: CheckStatus::Warning,
            message: "service: stopped".to_string(),
            suggestion: Some("start the service or run `beacon install`".to_string()),
        },
        Ok(crate::lifecycle::ServiceStatus::NotInstalled) => CheckResult {
            name: "ServiceStatus",
            status: CheckStatus::Warning,
            message: "service not installed".to_string(),
            suggestion: Some("run `beacon install` to set up as a system service".to_string()),
        },
        Ok(crate::lifecycle::ServiceStatus::Unknown(ref msg)) => CheckResult {
            name: "ServiceStatus",
            status: CheckStatus::Warning,
            message: format!("service: {msg}"),
            suggestion: None,
        },
        Err(_) => CheckResult {
            name: "ServiceStatus",
            status: CheckStatus::Warning,
            message: "could not determine service status".to_string(),
            suggestion: Some("run `beacon install` to set up as a system service".to_string()),
        },
    }
}

/// Check for security-related warnings
#[must_use]
pub fn security_warnings(config: &Config) -> CheckResult {
    let mut warnings = Vec::new();

    if matches!(config.dm_policy, crate::security::DmPolicy::Open) {
        warnings.push("DM policy is 'open' (anyone can message the bot)");
    }

    if config.api_server.api_key.is_none() {
        warnings.push("no API key set for admin endpoints");
    }

    if warnings.is_empty() {
        CheckResult {
            name: "SecurityWarnings",
            status: CheckStatus::Ok,
            message: "no security warnings".to_string(),
            suggestion: None,
        }
    } else {
        CheckResult {
            name: "SecurityWarnings",
            status: CheckStatus::Warning,
            message: warnings.join("; "),
            suggestion: Some("review security settings in config.toml".to_string()),
        }
    }
}
