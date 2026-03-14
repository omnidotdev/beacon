//! Container-sandboxed shell execution
//!
//! Wraps shell commands in Docker/Podman containers for isolation.
//! Falls back to host execution with a warning if no container runtime is found.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Container sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Container engine: "docker" or "podman" (auto-detected if empty)
    pub engine: String,
    /// Container image to use
    pub image: String,
    /// Memory limit (e.g. "512m")
    pub memory_limit: String,
    /// CPU limit (e.g. "1")
    pub cpu_limit: String,
    /// Enable container networking
    pub network: bool,
    /// Timeout in seconds
    pub timeout_secs: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            engine: String::new(),
            image: "alpine:latest".to_string(),
            memory_limit: "512m".to_string(),
            cpu_limit: "1".to_string(),
            network: false,
            timeout_secs: 120,
        }
    }
}

/// Detect an available container runtime on the system
fn detect_engine() -> Option<String> {
    for candidate in ["docker", "podman"] {
        if which::which(candidate).is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Resolve the container engine from config or auto-detection
fn resolve_engine(config: &SandboxConfig) -> Option<String> {
    if config.engine.is_empty() {
        detect_engine()
    } else if which::which(&config.engine).is_ok() {
        Some(config.engine.clone())
    } else {
        tracing::warn!(engine = %config.engine, "configured sandbox engine not found");
        None
    }
}

/// Execute a shell command inside a container sandbox
///
/// # Errors
///
/// Returns error if container execution fails
pub async fn execute_sandboxed(
    command: &str,
    working_dir: &PathBuf,
    config: &SandboxConfig,
    timeout: Option<u64>,
) -> Result<SandboxOutput> {
    let Some(engine) = resolve_engine(config) else {
        tracing::warn!("no container runtime found, falling back to host execution");
        return execute_host_fallback(command, working_dir, timeout.unwrap_or(config.timeout_secs))
            .await;
    };

    let timeout_secs = timeout.unwrap_or(config.timeout_secs).min(600);

    let mut cmd = tokio::process::Command::new(&engine);
    cmd.args(["run", "--rm"]);

    // Resource limits
    cmd.args(["--memory", &config.memory_limit]);
    cmd.args(["--cpus", &config.cpu_limit]);

    // Network isolation
    if !config.network {
        cmd.args(["--network", "none"]);
    }

    // Mount working directory
    let workdir_str = working_dir.display().to_string();
    cmd.args(["-v", &format!("{workdir_str}:/workspace")]);
    cmd.args(["-w", "/workspace"]);

    // Image and command
    cmd.arg(&config.image);
    cmd.args(["/bin/sh", "-c", command]);

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = cmd
        .spawn()
        .map_err(|e| Error::Tool(format!("failed to spawn {engine}: {e}")))?;

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| Error::Tool(format!("sandboxed command timed out ({timeout_secs}s)")))?
        .map_err(|e| Error::Tool(format!("sandbox process error: {e}")))?;

    Ok(SandboxOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        sandboxed: true,
    })
}

/// Fallback to host execution when no container runtime is available
async fn execute_host_fallback(
    command: &str,
    working_dir: &PathBuf,
    timeout_secs: u64,
) -> Result<SandboxOutput> {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.args(["-c", command]);
    cmd.current_dir(working_dir);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = cmd
        .spawn()
        .map_err(|e| Error::Tool(format!("failed to spawn shell: {e}")))?;

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| Error::Tool(format!("command timed out ({timeout_secs}s)")))?
        .map_err(|e| Error::Tool(format!("process error: {e}")))?;

    Ok(SandboxOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        sandboxed: false,
    })
}

/// Output from a sandboxed command execution
#[derive(Debug, Clone)]
pub struct SandboxOutput {
    /// Process exit code
    pub exit_code: i32,
    /// Captured stdout
    pub stdout: String,
    /// Captured stderr
    pub stderr: String,
    /// Whether execution was sandboxed (false = host fallback)
    pub sandboxed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sane() {
        let config = SandboxConfig::default();
        assert_eq!(config.image, "alpine:latest");
        assert_eq!(config.memory_limit, "512m");
        assert!(!config.network);
    }

    #[test]
    fn detect_engine_finds_something_or_none() {
        // Just verify it doesn't panic
        let _ = detect_engine();
    }

    #[tokio::test]
    async fn host_fallback_runs_command() {
        let output = execute_host_fallback("echo hello", &PathBuf::from("/tmp"), 10)
            .await
            .unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("hello"));
        assert!(!output.sandboxed);
    }
}
