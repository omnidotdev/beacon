//! Multi-agent registry for isolated agent configurations

use std::collections::HashMap;
use std::path::PathBuf;

use crate::security::DmPolicy;

use super::harness::Execution;

/// Unique agent identifier, lowercase-normalized
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(pub String);

impl AgentId {
    /// Create a new agent ID, normalizing to lowercase
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into().to_lowercase())
    }

    /// Default agent ID used when no agents are configured
    #[must_use]
    pub fn default_id() -> Self {
        Self("default".to_string())
    }

    /// Get the string value
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Configuration for a single agent instance
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Unique agent identifier
    pub id: AgentId,
    /// Optional persona override
    pub persona_id: Option<String>,
    /// Optional model override
    pub model_override: Option<String>,
    /// Optional skill filter (allowlist)
    pub skill_filter: Option<Vec<String>>,
    /// Per-agent workspace directory
    pub workspace_dir: PathBuf,
    /// Optional DM policy override
    pub dm_policy_override: Option<DmPolicy>,
    /// Channels this agent is enabled for (None = all)
    pub enabled_channels: Option<Vec<String>>,
    /// Execution mode: built-in Synapse loop or external harness
    pub execution: Execution,
}

/// Registry of all configured agents
#[derive(Debug)]
pub struct AgentRegistry {
    agents: HashMap<String, AgentConfig>,
    default_id: String,
}

impl AgentRegistry {
    /// Create a registry from a list of agent configs
    #[must_use]
    pub fn new(agents: Vec<AgentConfig>, default_id: Option<String>) -> Self {
        let default_id = default_id.unwrap_or_else(|| "default".to_string());
        let mut map = HashMap::new();
        for agent in agents {
            map.insert(agent.id.0.clone(), agent);
        }
        Self {
            agents: map,
            default_id,
        }
    }

    /// Create a single-agent registry from existing config (backward-compat)
    #[must_use]
    pub fn single(data_dir: &std::path::Path) -> Self {
        let default_config = AgentConfig {
            id: AgentId::default_id(),
            persona_id: None,
            model_override: None,
            skill_filter: None,
            workspace_dir: data_dir.join("agents").join("default"),
            dm_policy_override: None,
            enabled_channels: None,
            execution: Execution::default(),
        };

        let mut agents = HashMap::new();
        agents.insert("default".to_string(), default_config);

        Self {
            agents,
            default_id: "default".to_string(),
        }
    }

    /// Get agent config by ID
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&AgentConfig> {
        self.agents.get(id)
    }

    /// Get the default agent config
    #[must_use]
    pub fn default_agent(&self) -> Option<&AgentConfig> {
        self.agents.get(&self.default_id)
    }

    /// Get the default agent ID
    #[must_use]
    pub fn default_id(&self) -> &str {
        &self.default_id
    }

    /// List all agent configs
    #[must_use]
    pub fn list(&self) -> Vec<&AgentConfig> {
        self.agents.values().collect()
    }

    /// Number of configured agents
    #[must_use]
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Check if registry is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_normalizes_to_lowercase() {
        let id = AgentId::new("MyAgent");
        assert_eq!(id.as_str(), "myagent");
    }

    #[test]
    fn single_registry_has_default() {
        let registry = AgentRegistry::single(std::path::Path::new("/tmp"));
        assert_eq!(registry.len(), 1);
        assert!(registry.default_agent().is_some());
        assert_eq!(registry.default_id(), "default");
    }

    #[test]
    fn multi_agent_registry() {
        let agents = vec![
            AgentConfig {
                id: AgentId::new("alpha"),
                persona_id: Some("orin".to_string()),
                model_override: None,
                skill_filter: None,
                workspace_dir: PathBuf::from("/tmp/alpha"),
                dm_policy_override: None,
                enabled_channels: None,
                execution: Execution::default(),
            },
            AgentConfig {
                id: AgentId::new("beta"),
                persona_id: Some("assistant".to_string()),
                model_override: Some("gpt-4o".to_string()),
                skill_filter: None,
                workspace_dir: PathBuf::from("/tmp/beta"),
                dm_policy_override: None,
                enabled_channels: Some(vec!["discord".to_string()]),
                execution: Execution::default(),
            },
        ];

        let registry = AgentRegistry::new(agents, Some("alpha".to_string()));
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.default_id(), "alpha");
        assert!(registry.get("beta").is_some());
    }
}
