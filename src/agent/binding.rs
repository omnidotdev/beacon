//! Agent binding router for channel-to-agent mapping

use super::registry::AgentId;

/// Binding of a channel (and optionally an account) to a specific agent
#[derive(Debug, Clone)]
pub struct AgentBinding {
    /// Channel name (e.g. "discord", "telegram", "web")
    pub channel: String,
    /// Optional channel-specific account ID
    pub account_id: Option<String>,
    /// Agent to route to
    pub agent_id: String,
}

/// Router that resolves which agent handles a given channel/account
#[derive(Debug)]
pub struct BindingRouter {
    bindings: Vec<AgentBinding>,
    default_agent_id: String,
}

impl BindingRouter {
    /// Create a new binding router
    #[must_use]
    pub const fn new(bindings: Vec<AgentBinding>, default_agent_id: String) -> Self {
        Self {
            bindings,
            default_agent_id,
        }
    }

    /// Resolve channel + account to an agent ID
    ///
    /// Matching priority:
    /// 1. Exact channel + `account_id` match
    /// 2. Channel-only match (no `account_id` filter)
    /// 3. Default agent
    #[must_use]
    pub fn resolve(&self, channel: &str, account_id: Option<&str>) -> AgentId {
        // Try exact match (channel + account_id)
        if let Some(acct) = account_id
            && let Some(binding) = self
                .bindings
                .iter()
                .find(|b| b.channel == channel && b.account_id.as_deref() == Some(acct))
        {
            return AgentId::new(&binding.agent_id);
        }

        // Try channel-only match
        if let Some(binding) = self
            .bindings
            .iter()
            .find(|b| b.channel == channel && b.account_id.is_none())
        {
            return AgentId::new(&binding.agent_id);
        }

        // Fall back to default
        AgentId::new(&self.default_agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_exact_match() {
        let bindings = vec![AgentBinding {
            channel: "discord".to_string(),
            account_id: Some("guild-123".to_string()),
            agent_id: "alpha".to_string(),
        }];
        let router = BindingRouter::new(bindings, "default".to_string());
        let result = router.resolve("discord", Some("guild-123"));
        assert_eq!(result.as_str(), "alpha");
    }

    #[test]
    fn resolve_channel_only() {
        let bindings = vec![AgentBinding {
            channel: "telegram".to_string(),
            account_id: None,
            agent_id: "beta".to_string(),
        }];
        let router = BindingRouter::new(bindings, "default".to_string());
        let result = router.resolve("telegram", Some("any-chat"));
        assert_eq!(result.as_str(), "beta");
    }

    #[test]
    fn resolve_falls_back_to_default() {
        let bindings = vec![];
        let router = BindingRouter::new(bindings, "default".to_string());
        let result = router.resolve("slack", None);
        assert_eq!(result.as_str(), "default");
    }
}
