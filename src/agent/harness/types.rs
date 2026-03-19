//! Harness adapter types

use std::collections::HashMap;

use async_trait::async_trait;
use serde::Deserialize;

/// How an agent executes: built-in Synapse loop or external CLI harness
#[derive(Debug, Clone, Default)]
pub enum Execution {
    /// Run via Synapse agentic loop (current behavior)
    #[default]
    Builtin,
    /// Delegate to an external CLI harness
    Harness(HarnessConfig),
}

/// Configuration for a harness-backed agent
#[derive(Debug, Clone, Deserialize)]
pub struct HarnessConfig {
    /// Adapter identifier (e.g. `claude_cli`, `codex_cli`)
    pub adapter: String,
    /// Session persistence mode
    #[serde(default)]
    pub session_mode: SessionMode,
    /// Environment variables to inject into the harness process
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// MCP servers to expose to the harness
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    /// Whether to skip permission prompts (e.g. --dangerously-skip-permissions)
    #[serde(default)]
    pub skip_permissions: bool,
}

/// Session persistence mode for harness invocations
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Resume previous session when possible (default)
    #[default]
    Resume,
    /// Fresh session each invocation
    Fresh,
}

/// Structured output from a harness invocation
#[derive(Debug, Clone)]
pub struct HarnessOutput {
    /// Final text response
    pub response: String,
    /// Session ID for potential resumption
    pub session_id: Option<String>,
    /// Token usage and cost data
    pub usage: Option<HarnessUsage>,
}

/// Token usage and cost from a harness run
#[derive(Debug, Clone)]
pub struct HarnessUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub model: Option<String>,
}

/// Trait that all harness adapters implement
#[async_trait]
pub trait HarnessAdapter: Send + Sync + std::fmt::Debug {
    /// Unique adapter identifier (e.g. `claude_cli`)
    fn id(&self) -> &str;

    /// Check if the harness CLI is available on this system
    async fn is_available(&self) -> bool;

    /// Execute a prompt and return structured output
    ///
    /// # Errors
    ///
    /// Returns error if the harness process fails to spawn or exits with error
    async fn execute(
        &self,
        prompt: &str,
        config: &HarnessConfig,
        session_id: Option<&str>,
    ) -> crate::Result<HarnessOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_defaults_to_builtin() {
        let exec = Execution::default();
        assert!(matches!(exec, Execution::Builtin));
    }

    #[test]
    fn session_mode_defaults_to_resume() {
        let mode = SessionMode::default();
        assert!(matches!(mode, SessionMode::Resume));
    }

    #[test]
    fn harness_config_deserializes_minimal() {
        let toml = r#"
            adapter = "claude_cli"
        "#;
        let config: HarnessConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.adapter, "claude_cli");
        assert!(matches!(config.session_mode, SessionMode::Resume));
        assert!(config.env.is_empty());
        assert!(config.mcp_servers.is_empty());
        assert!(!config.skip_permissions);
    }

    #[test]
    fn harness_config_deserializes_full() {
        let toml = r#"
            adapter = "claude_cli"
            session_mode = "fresh"
            skip_permissions = true
            mcp_servers = ["beacon-memory", "beacon-canvas"]

            [env]
            ANTHROPIC_API_KEY = "sk-test"
        "#;
        let config: HarnessConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.adapter, "claude_cli");
        assert!(matches!(config.session_mode, SessionMode::Fresh));
        assert!(config.skip_permissions);
        assert_eq!(config.env.get("ANTHROPIC_API_KEY").unwrap(), "sk-test");
        assert_eq!(config.mcp_servers.len(), 2);
    }
}
