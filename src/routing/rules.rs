//! Routing rule definitions for message filtering

use serde::Deserialize;

/// A routing rule evaluated against incoming messages
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingRule {
    /// Human-readable rule name
    pub name: String,
    /// Whether the rule is active
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Conditions that must all match for the rule to fire
    pub match_on: MatchCondition,
    /// Action to take when the rule matches
    pub action: RoutingAction,
    /// Lower number = higher priority (evaluated first)
    #[serde(default = "default_priority")]
    pub priority: u32,
}

const fn default_enabled() -> bool {
    true
}

const fn default_priority() -> u32 {
    100
}

/// All conditions must match for the rule to fire
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchCondition {
    /// Channel names (empty = all)
    #[serde(default)]
    pub channels: Vec<String>,
    /// Sender IDs (empty = all)
    #[serde(default)]
    pub senders: Vec<String>,
    /// Regex on content
    pub content_pattern: Option<String>,
    /// Only match DMs
    pub dm_only: Option<bool>,
    /// Only match group messages
    pub group_only: Option<bool>,
}

impl MatchCondition {
    /// Check whether this condition matches the given message parameters
    #[must_use]
    pub fn matches(&self, channel: &str, sender_id: &str, content: &str, is_dm: bool) -> bool {
        // Channel filter: empty = match all
        if !self.channels.is_empty() && !self.channels.iter().any(|c| c == channel) {
            return false;
        }

        // Sender filter: empty = match all
        if !self.senders.is_empty() && !self.senders.iter().any(|s| s == sender_id) {
            return false;
        }

        // Content pattern: regex match
        if let Some(ref pattern) = self.content_pattern {
            match regex::Regex::new(pattern) {
                Ok(re) => {
                    if !re.is_match(content) {
                        return false;
                    }
                }
                Err(e) => {
                    tracing::warn!(pattern = %pattern, error = %e, "invalid routing rule regex");
                    return false;
                }
            }
        }

        // DM filter
        if self.dm_only == Some(true) && !is_dm {
            return false;
        }

        // Group filter
        if self.group_only == Some(true) && is_dm {
            return false;
        }

        true
    }
}

/// Action to take when a routing rule matches
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutingAction {
    /// Allow the message through (no-op)
    Allow,
    /// Silently deny the message
    Deny,
    /// Deny and send a reply to the sender
    DenyWithReply {
        /// Reply message to send
        message: String,
    },
    /// Redirect to a specific agent
    RedirectToAgent {
        /// Agent identifier
        agent_id: String,
    },
    /// Rate limit the sender
    RateLimit {
        /// Maximum messages per minute
        max_per_minute: u32,
    },
}
