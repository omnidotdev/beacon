//! Claude Code CLI harness adapter

use async_trait::async_trait;
use tokio::process::Command;

use super::types::{HarnessAdapter, HarnessConfig, HarnessOutput, HarnessUsage};

/// Adapter for Claude Code CLI (`claude` command)
#[derive(Debug)]
pub struct ClaudeCliAdapter;

impl ClaudeCliAdapter {
    /// Parse usage from Claude CLI's stream-json result event
    fn parse_result_event(raw: &str) -> Option<(String, Option<String>, Option<HarnessUsage>)> {
        // Claude CLI with --output-format stream-json emits newline-delimited JSON
        // The final event has type "result" with cost and usage data
        for line in raw.lines().rev() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if event.get("type").and_then(|t| t.as_str()) != Some("result") {
                continue;
            }

            let response = event
                .get("result")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_owned();

            let session_id = event
                .get("session_id")
                .and_then(|s| s.as_str())
                .map(String::from);

            let usage = Self::extract_usage(&event);

            return Some((response, session_id, usage));
        }
        None
    }

    /// Extract usage metrics from a result event
    fn extract_usage(event: &serde_json::Value) -> Option<HarnessUsage> {
        let cost_usd = event.get("total_cost_usd")?.as_f64()?;
        let usage_obj = event.get("usage")?;

        Some(HarnessUsage {
            input_tokens: usage_obj
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            cached_input_tokens: usage_obj
                .get("cache_read_input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage_obj
                .get("output_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            cost_usd,
            model: event
                .get("model")
                .and_then(|m| m.as_str())
                .map(String::from),
        })
    }
}

#[async_trait]
impl HarnessAdapter for ClaudeCliAdapter {
    #[allow(clippy::unnecessary_literal_bound)]
    fn id(&self) -> &str {
        "claude_cli"
    }

    async fn is_available(&self) -> bool {
        which::which("claude").is_ok()
    }

    async fn execute(
        &self,
        prompt: &str,
        config: &HarnessConfig,
        session_id: Option<&str>,
    ) -> crate::Result<HarnessOutput> {
        let mut cmd = Command::new("claude");
        cmd.arg("--print")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");

        // Session resumption
        if let Some(sid) = session_id {
            cmd.arg("--resume").arg(sid);
        }

        // Permission skipping
        if config.skip_permissions {
            cmd.arg("--dangerously-skip-permissions");
        }

        // MCP server configs (JSON strings or file paths)
        for mcp_config in &config.mcp_servers {
            cmd.arg("--mcp-config").arg(mcp_config);
        }

        // Model override
        if let Some(ref model) = config.model {
            cmd.arg("--model").arg(model);
        }

        // System prompt
        if let Some(ref system_prompt) = config.system_prompt {
            cmd.arg("--system-prompt").arg(system_prompt);
        }

        // Environment variables
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        // Prompt as positional argument
        cmd.arg(prompt);

        tracing::info!(adapter = "claude_cli", "spawning harness");

        let output = cmd
            .output()
            .await
            .map_err(|e| crate::Error::Agent(format!("failed to spawn claude CLI: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::Error::Agent(format!(
                "claude CLI exited with {}: {stderr}",
                output.status
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        match Self::parse_result_event(&stdout) {
            Some((response, sid, usage)) => Ok(HarnessOutput {
                response,
                session_id: sid,
                usage,
            }),
            None => {
                // Fallback: treat entire stdout as response
                Ok(HarnessOutput {
                    response: stdout.into_owned(),
                    session_id: None,
                    usage: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_result_event_extracts_data() {
        let raw = r#"{"type":"assistant","message":"thinking..."}
{"type":"result","result":"Hello!","session_id":"sess_abc123","total_cost_usd":0.003,"usage":{"input_tokens":100,"cache_read_input_tokens":50,"output_tokens":200},"model":"claude-sonnet-4-6"}"#;

        let (response, session_id, usage) = ClaudeCliAdapter::parse_result_event(raw).unwrap();
        assert_eq!(response, "Hello!");
        assert_eq!(session_id.unwrap(), "sess_abc123");
        let usage = usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cached_input_tokens, 50);
        assert_eq!(usage.output_tokens, 200);
        assert!((usage.cost_usd - 0.003).abs() < f64::EPSILON);
        assert_eq!(usage.model.unwrap(), "claude-sonnet-4-6");
    }

    #[test]
    fn parse_result_event_handles_missing_usage() {
        let raw = r#"{"type":"result","result":"Done","session_id":"sess_xyz"}"#;
        let (response, session_id, usage) = ClaudeCliAdapter::parse_result_event(raw).unwrap();
        assert_eq!(response, "Done");
        assert_eq!(session_id.unwrap(), "sess_xyz");
        assert!(usage.is_none());
    }

    #[test]
    fn parse_result_event_returns_none_for_no_result() {
        let raw = r#"{"type":"assistant","message":"just text"}"#;
        assert!(ClaudeCliAdapter::parse_result_event(raw).is_none());
    }
}
