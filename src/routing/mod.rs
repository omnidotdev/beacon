//! Message routing and gating system
//!
//! Evaluate rule-based filters against incoming messages to decide whether
//! to allow, deny, redirect, or rate-limit them before agent dispatch.

pub mod rules;
pub use rules::*;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Result of evaluating routing rules against a message
#[derive(Debug)]
pub enum RoutingDecision {
    /// Allow the message through
    Allow,
    /// Silently deny the message
    Deny,
    /// Deny and reply with the given message
    DenyWithReply(String),
    /// Redirect message to a specific agent
    RedirectToAgent(String),
    /// Message was rate-limited
    RateLimited,
}

/// Engine that evaluates routing rules against incoming messages
pub struct RoutingEngine {
    rules: Vec<RoutingRule>,
    rate_counters: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RoutingEngine {
    /// Create a new routing engine with the given rules, sorted by priority (lower = higher priority)
    #[must_use]
    pub fn new(mut rules: Vec<RoutingRule>) -> Self {
        rules.sort_by_key(|r| r.priority);
        Self {
            rules,
            rate_counters: Mutex::new(HashMap::new()),
        }
    }

    /// Evaluate all rules against the given message parameters
    ///
    /// Returns the decision from the first matching rule, or `Allow` if no rules match.
    #[must_use]
    pub fn evaluate(
        &self,
        channel: &str,
        sender_id: &str,
        content: &str,
        is_dm: bool,
    ) -> RoutingDecision {
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }

            if !rule.match_on.matches(channel, sender_id, content, is_dm) {
                continue;
            }

            return match &rule.action {
                RoutingAction::Allow => RoutingDecision::Allow,
                RoutingAction::Deny => RoutingDecision::Deny,
                RoutingAction::DenyWithReply { message } => {
                    RoutingDecision::DenyWithReply(message.clone())
                }
                RoutingAction::RedirectToAgent { agent_id } => {
                    RoutingDecision::RedirectToAgent(agent_id.clone())
                }
                RoutingAction::RateLimit { max_per_minute } => {
                    self.check_rate_limit(sender_id, *max_per_minute)
                }
            };
        }

        RoutingDecision::Allow
    }

    /// Check rate limit for a sender, pruning stale entries
    #[allow(clippy::significant_drop_tightening)]
    fn check_rate_limit(&self, sender_id: &str, max_per_minute: u32) -> RoutingDecision {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        let mut counters = self
            .rate_counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let timestamps = counters.entry(sender_id.to_string()).or_default();

        // Prune timestamps older than 1 minute
        timestamps.retain(|t| now.duration_since(*t) < window);

        if timestamps.len() >= max_per_minute as usize {
            RoutingDecision::RateLimited
        } else {
            timestamps.push(now);
            RoutingDecision::Allow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(name: &str, match_on: MatchCondition, action: RoutingAction) -> RoutingRule {
        RoutingRule {
            name: name.to_string(),
            enabled: true,
            match_on,
            action,
            priority: 100,
        }
    }

    #[test]
    fn empty_rules_allows_all() {
        let engine = RoutingEngine::new(vec![]);
        let decision = engine.evaluate("discord", "user1", "hello", false);
        assert!(matches!(decision, RoutingDecision::Allow));
    }

    #[test]
    fn deny_rule_blocks() {
        let rule = make_rule("block-all", MatchCondition::default(), RoutingAction::Deny);
        let engine = RoutingEngine::new(vec![rule]);
        let decision = engine.evaluate("discord", "user1", "hello", false);
        assert!(matches!(decision, RoutingDecision::Deny));
    }

    #[test]
    fn channel_filter_matches() {
        let rule = make_rule(
            "discord-only",
            MatchCondition {
                channels: vec!["discord".to_string()],
                ..Default::default()
            },
            RoutingAction::Deny,
        );
        let engine = RoutingEngine::new(vec![rule]);

        // Should match discord
        let decision = engine.evaluate("discord", "user1", "hello", false);
        assert!(matches!(decision, RoutingDecision::Deny));

        // Should not match slack
        let decision = engine.evaluate("slack", "user1", "hello", false);
        assert!(matches!(decision, RoutingDecision::Allow));
    }

    #[test]
    fn content_pattern_matches() {
        let rule = make_rule(
            "block-spam",
            MatchCondition {
                content_pattern: Some(r"(?i)buy\s+now".to_string()),
                ..Default::default()
            },
            RoutingAction::Deny,
        );
        let engine = RoutingEngine::new(vec![rule]);

        let decision = engine.evaluate("discord", "user1", "BUY NOW cheap stuff", false);
        assert!(matches!(decision, RoutingDecision::Deny));

        let decision = engine.evaluate("discord", "user1", "hello world", false);
        assert!(matches!(decision, RoutingDecision::Allow));
    }

    #[test]
    fn priority_ordering() {
        let allow_rule = RoutingRule {
            name: "allow-all".to_string(),
            enabled: true,
            match_on: MatchCondition::default(),
            action: RoutingAction::Allow,
            priority: 10, // Higher priority (lower number)
        };
        let deny_rule = RoutingRule {
            name: "deny-all".to_string(),
            enabled: true,
            match_on: MatchCondition::default(),
            action: RoutingAction::Deny,
            priority: 50,
        };
        // Insert in wrong order to verify sorting
        let engine = RoutingEngine::new(vec![deny_rule, allow_rule]);
        let decision = engine.evaluate("discord", "user1", "hello", false);
        assert!(matches!(decision, RoutingDecision::Allow));
    }

    #[test]
    fn dm_only_filter() {
        let rule = make_rule(
            "dm-only",
            MatchCondition {
                dm_only: Some(true),
                ..Default::default()
            },
            RoutingAction::Deny,
        );
        let engine = RoutingEngine::new(vec![rule]);

        // Should match DMs
        let decision = engine.evaluate("discord", "user1", "hello", true);
        assert!(matches!(decision, RoutingDecision::Deny));

        // Should not match group messages
        let decision = engine.evaluate("discord", "user1", "hello", false);
        assert!(matches!(decision, RoutingDecision::Allow));
    }

    #[test]
    fn redirect_overrides_agent() {
        let rule = make_rule(
            "redirect",
            MatchCondition::default(),
            RoutingAction::RedirectToAgent {
                agent_id: "support-agent".to_string(),
            },
        );
        let engine = RoutingEngine::new(vec![rule]);
        let decision = engine.evaluate("discord", "user1", "help", false);
        match decision {
            RoutingDecision::RedirectToAgent(id) => assert_eq!(id, "support-agent"),
            other => panic!("expected RedirectToAgent, got {other:?}"),
        }
    }

    #[test]
    fn rate_limit_enforced() {
        let rule = make_rule(
            "rate-limit",
            MatchCondition::default(),
            RoutingAction::RateLimit { max_per_minute: 2 },
        );
        let engine = RoutingEngine::new(vec![rule]);

        // First two should be allowed
        let d1 = engine.evaluate("discord", "user1", "msg1", false);
        assert!(matches!(d1, RoutingDecision::Allow));
        let d2 = engine.evaluate("discord", "user1", "msg2", false);
        assert!(matches!(d2, RoutingDecision::Allow));

        // Third should be rate limited
        let d3 = engine.evaluate("discord", "user1", "msg3", false);
        assert!(matches!(d3, RoutingDecision::RateLimited));

        // Different sender should still be allowed
        let d4 = engine.evaluate("discord", "user2", "msg1", false);
        assert!(matches!(d4, RoutingDecision::Allow));
    }

    #[test]
    fn disabled_rule_skipped() {
        let rule = RoutingRule {
            name: "disabled".to_string(),
            enabled: false,
            match_on: MatchCondition::default(),
            action: RoutingAction::Deny,
            priority: 1,
        };
        let engine = RoutingEngine::new(vec![rule]);
        let decision = engine.evaluate("discord", "user1", "hello", false);
        assert!(matches!(decision, RoutingDecision::Allow));
    }

    #[test]
    fn deny_with_reply_returns_message() {
        let rule = make_rule(
            "deny-reply",
            MatchCondition::default(),
            RoutingAction::DenyWithReply {
                message: "not allowed".to_string(),
            },
        );
        let engine = RoutingEngine::new(vec![rule]);
        let decision = engine.evaluate("discord", "user1", "hello", false);
        match decision {
            RoutingDecision::DenyWithReply(msg) => assert_eq!(msg, "not allowed"),
            other => panic!("expected DenyWithReply, got {other:?}"),
        }
    }
}
