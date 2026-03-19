//! Harness turn runner — orchestrates adapter lookup, CLI execution, and cost recording

use std::collections::HashMap;
use std::sync::Arc;

use super::claude_cli::ClaudeCliAdapter;
use super::session::HarnessSessionRepo;
use super::types::{HarnessAdapter, HarnessConfig, SessionMode};
use crate::agent::registry::AgentId;
use crate::db::SessionRepo;

/// Registry of available harness adapters
#[derive(Debug)]
pub struct AdapterRegistry {
    adapters: HashMap<String, Arc<dyn HarnessAdapter>>,
}

impl AdapterRegistry {
    /// Create a registry with all built-in adapters
    #[must_use]
    pub fn new() -> Self {
        let mut adapters: HashMap<String, Arc<dyn HarnessAdapter>> = HashMap::new();
        adapters.insert("claude_cli".to_owned(), Arc::new(ClaudeCliAdapter));
        Self { adapters }
    }

    /// Get an adapter by ID
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<dyn HarnessAdapter>> {
        self.adapters.get(id).cloned()
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Dependencies for running a harness turn
pub struct HarnessTurnDeps<'a> {
    pub adapter_registry: &'a AdapterRegistry,
    pub harness_session_repo: &'a HarnessSessionRepo,
    pub session_repo: &'a SessionRepo,
    pub usage_recorder: Option<&'a synapse_billing::UsageRecorder>,
    pub usage_repo: Option<&'a crate::db::UsageRepo>,
}

/// Run a prompt through an external harness adapter
///
/// Handles: adapter lookup, session resumption, CLI execution,
/// cost recording, session persistence
///
/// # Errors
///
/// Returns error if adapter not found, CLI unavailable, or harness fails
#[allow(clippy::too_many_arguments)]
pub async fn run_harness_turn(
    deps: &HarnessTurnDeps<'_>,
    agent_id: &AgentId,
    config: &HarnessConfig,
    prompt: &str,
    channel: &str,
    channel_id: &str,
    session_id: &str,
    user_id: &str,
) -> crate::Result<String> {
    // Resolve adapter
    let adapter = deps.adapter_registry.get(&config.adapter).ok_or_else(|| {
        crate::Error::Harness(format!("unknown harness adapter: {}", config.adapter))
    })?;

    // Check adapter availability
    if !adapter.is_available().await {
        return Err(crate::Error::Harness(format!(
            "harness CLI not found for adapter: {}",
            config.adapter
        )));
    }

    // Resolve session ID for resumption
    let stored_session_id = match config.session_mode {
        SessionMode::Resume => {
            deps.harness_session_repo
                .get_session_id(agent_id.as_str(), channel, channel_id)?
        }
        SessionMode::Fresh => None,
    };

    tracing::info!(
        agent = %agent_id,
        adapter = %config.adapter,
        resume_session = ?stored_session_id,
        "running harness turn"
    );

    // Execute via adapter
    let output = adapter
        .execute(prompt, config, stored_session_id.as_deref())
        .await;

    // Handle stale session: retry fresh if resumption failed
    let output = match output {
        Err(ref _e) if stored_session_id.is_some() => {
            tracing::warn!(
                agent = %agent_id,
                "harness session may be stale, retrying fresh"
            );
            deps.harness_session_repo
                .delete_session(agent_id.as_str(), channel, channel_id)?;
            adapter.execute(prompt, config, None).await?
        }
        other => other?,
    };

    // Persist session ID
    if let Some(ref sid) = output.session_id {
        deps.harness_session_repo.upsert_session_id(
            agent_id.as_str(),
            channel,
            channel_id,
            sid,
            &config.adapter,
        )?;
    }

    // Record usage
    if let Some(ref usage) = output.usage {
        let model = usage.model.as_deref().unwrap_or("unknown");
        let provider = "harness";

        #[allow(clippy::cast_possible_truncation)]
        let (input_tokens, output_tokens) = (usage.input_tokens as u32, usage.output_tokens as u32);

        if let Some(recorder) = deps.usage_recorder {
            let idempotency_key = uuid::Uuid::new_v4().to_string();
            recorder.record(synapse_billing::UsageEvent {
                entity_type: "user".to_owned(),
                entity_id: user_id.to_owned(),
                model: model.to_owned(),
                provider: provider.to_owned(),
                input_tokens,
                output_tokens,
                estimated_cost_usd: usage.cost_usd,
                idempotency_key,
            });
        }

        if let Some(usage_repo) = deps.usage_repo
            && let Err(e) = usage_repo.record(
                session_id,
                agent_id.as_str(),
                model,
                provider,
                input_tokens,
                output_tokens,
                usage.cost_usd,
            )
        {
            tracing::warn!(error = %e, "failed to record harness usage");
        }
    }

    // Store messages in session history
    deps.session_repo
        .add_message(session_id, crate::db::MessageRole::User, prompt)?;
    deps.session_repo.add_message(
        session_id,
        crate::db::MessageRole::Assistant,
        &output.response,
    )?;

    Ok(output.response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_registry_finds_claude_cli() {
        let registry = AdapterRegistry::new();
        assert!(registry.get("claude_cli").is_some());
        assert!(registry.get("nonexistent").is_none());
    }
}
