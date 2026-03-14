//! Health diagnostics for `beacon doctor`

pub mod checks;

use crate::Config;

/// Status of a diagnostic check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    /// Check passed
    Ok,
    /// Non-critical issue
    Warning,
    /// Critical issue
    Error,
}

impl std::fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => write!(f, "ok"),
            Self::Warning => write!(f, "warning"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// Result of a single diagnostic check
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Check name
    pub name: &'static str,
    /// Pass/warn/fail status
    pub status: CheckStatus,
    /// Human-readable message
    pub message: String,
    /// Suggested fix (for warnings and errors)
    pub suggestion: Option<String>,
}

/// Run all diagnostic checks
pub async fn run_diagnostics(config: &Config) -> Vec<CheckResult> {
    let mut results = Vec::new();

    results.push(checks::config_parseable());
    results.push(checks::synapse_connectivity(config).await);
    results.push(checks::sqlite_health(config));
    results.push(checks::embeddings_available());
    results.push(checks::channel_auth(config));
    results.push(checks::mcp_servers(config));
    results.push(checks::sandbox_runtime(config));
    results.push(checks::disk_space(config));
    results.push(checks::service_status());
    results.push(checks::security_warnings(config));

    results
}

/// Print diagnostic results to stdout
pub fn print_results(results: &[CheckResult]) {
    let total = results.len();
    let ok_count = results
        .iter()
        .filter(|r| r.status == CheckStatus::Ok)
        .count();
    let warn_count = results
        .iter()
        .filter(|r| r.status == CheckStatus::Warning)
        .count();
    let err_count = results
        .iter()
        .filter(|r| r.status == CheckStatus::Error)
        .count();

    println!("Beacon Doctor - Health Diagnostics\n");

    for result in results {
        let icon = match result.status {
            CheckStatus::Ok => "[pass]",
            CheckStatus::Warning => "[warn]",
            CheckStatus::Error => "[FAIL]",
        };

        println!("{icon} {}: {}", result.name, result.message);

        if let Some(ref suggestion) = result.suggestion {
            println!("       -> {suggestion}");
        }
    }

    println!("\n---");
    println!("{total} checks: {ok_count} passed, {warn_count} warnings, {err_count} errors");
}
