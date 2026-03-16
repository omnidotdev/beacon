//! Hook system for message lifecycle events
//!
//! Supports both built-in handlers (auto-reply) and external hooks loaded
//! from `~/.beacon/hooks/`

mod auto_reply;
mod executor;
mod loader;
mod types;

pub use auto_reply::AutoReplyRule;
pub use types::{HookAction, HookEvent, HookResult};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use loader::DiscoveredHook;

/// Hook configuration
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct HooksConfig {
    /// Enable hook system
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Custom hooks directory (default: ~/.beacon/hooks)
    pub path: Option<PathBuf>,
    /// Auto-reply rules
    #[serde(default)]
    pub auto_reply: Vec<AutoReplyRule>,
}

const fn default_true() -> bool {
    true
}

/// Check whether an event string matches a pattern
///
/// Supports exact matching, glob-style `*` (match all), and
/// namespace wildcards like `"message:*"`.
fn event_matches_pattern(event: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.ends_with(":*") {
        let prefix = &pattern[..pattern.len() - 1]; // e.g. "message:"
        return event.starts_with(prefix);
    }
    event == pattern
}

/// Hook manager
pub struct HookManager {
    enabled: bool,
    auto_reply: auto_reply::AutoReplyHandler,
    external_hooks: HashMap<HookAction, Vec<Arc<DiscoveredHook>>>,
    /// Hooks with wildcard event patterns (cannot be indexed by exact action)
    wildcard_hooks: Vec<Arc<DiscoveredHook>>,
}

impl HookManager {
    /// Create a new hook manager
    #[must_use]
    pub fn new(config: &HooksConfig, data_dir: &std::path::Path) -> Self {
        if !config.enabled {
            tracing::info!("hooks disabled");
            return Self {
                enabled: false,
                auto_reply: auto_reply::AutoReplyHandler::new(&[]),
                external_hooks: HashMap::new(),
                wildcard_hooks: Vec::new(),
            };
        }

        // Load auto-reply handler
        let auto_reply = auto_reply::AutoReplyHandler::new(&config.auto_reply);

        // Discover external hooks
        let hooks_dir = config
            .path
            .clone()
            .unwrap_or_else(|| data_dir.join("hooks"));

        let discovered = loader::discover_hooks(&hooks_dir);

        // Index hooks by event; hooks with wildcard patterns go into a separate list
        let mut external_hooks: HashMap<HookAction, Vec<Arc<DiscoveredHook>>> = HashMap::new();
        let mut wildcard_hooks: Vec<Arc<DiscoveredHook>> = Vec::new();

        for hook in discovered {
            let hook = Arc::new(hook);
            let has_wildcard = hook.event_patterns.iter().any(|p| p.contains('*'));

            if has_wildcard {
                wildcard_hooks.push(Arc::clone(&hook));
            }

            // Also index exact-match events for fast lookup
            for event in &hook.events {
                external_hooks
                    .entry(event.clone())
                    .or_default()
                    .push(Arc::clone(&hook));
            }
        }

        let total_external: usize =
            external_hooks.values().map(Vec::len).sum::<usize>() + wildcard_hooks.len();
        tracing::info!(
            auto_reply_rules = config.auto_reply.len(),
            external_hooks = total_external,
            "hook manager initialized"
        );

        Self {
            enabled: true,
            auto_reply,
            external_hooks,
            wildcard_hooks,
        }
    }

    /// Trigger hooks for an event
    ///
    /// Runs auto-reply first, then external hooks in discovery order
    pub async fn trigger(&self, event: &HookEvent) -> HookResult {
        if !self.enabled {
            return HookResult::default();
        }

        let mut result = HookResult::default();

        // Check auto-reply first
        if let Some(auto_result) = self.auto_reply.handle(event) {
            tracing::debug!(
                action = %event.action,
                has_reply = auto_result.reply.is_some(),
                skip_agent = auto_result.skip_agent,
                "auto-reply triggered"
            );
            result.merge(auto_result);

            // If skip_processing, don't run external hooks
            if result.skip_processing {
                return result;
            }
        }

        // Run external hooks — collect from exact-match index and wildcard list
        let action = HookAction::from_str(&event.action);

        // Deduplicate: track which hooks we have already executed (by name)
        let mut executed = std::collections::HashSet::new();

        // Exact-match hooks
        if let Some(action) = &action
            && let Some(hooks) = self.external_hooks.get(action)
        {
            for hook in hooks {
                executed.insert(hook.name.clone());
                if let Some(early) = Self::run_hook(hook, event, &mut result).await {
                    return early;
                }
            }
        }

        // Wildcard hooks
        for hook in &self.wildcard_hooks {
            if executed.contains(&hook.name) {
                continue;
            }
            let matches = hook
                .event_patterns
                .iter()
                .any(|p| event_matches_pattern(&event.action, p));
            if !matches {
                continue;
            }
            executed.insert(hook.name.clone());
            if let Some(early) = Self::run_hook(hook, event, &mut result).await {
                return early;
            }
        }

        result
    }

    /// Execute a single hook, merging results. Returns `Some(result)` for early exit
    async fn run_hook(
        hook: &DiscoveredHook,
        event: &HookEvent,
        result: &mut HookResult,
    ) -> Option<HookResult> {
        match executor::execute_hook(&hook.handler_path, event, None).await {
            Ok(hook_result) => {
                tracing::debug!(
                    hook = %hook.name,
                    action = %event.action,
                    "hook executed"
                );
                result.merge(hook_result);
                if result.skip_processing {
                    return Some(result.clone());
                }
            }
            Err(e) => {
                tracing::warn!(
                    hook = %hook.name,
                    error = %e,
                    "hook execution failed"
                );
                // Continue with other hooks
            }
        }
        None
    }

    /// Check if hooks are enabled
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl std::fmt::Debug for HookManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookManager")
            .field("enabled", &self.enabled)
            .field("external_hooks", &self.external_hooks.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_disabled_manager() {
        let config = HooksConfig {
            enabled: false,
            ..Default::default()
        };

        let manager = HookManager::new(&config, std::path::Path::new("/tmp"));
        assert!(!manager.is_enabled());

        let event = HookEvent {
            action: "message:received".to_string(),
            channel: "test".to_string(),
            channel_id: "ch1".to_string(),
            message_id: "msg1".to_string(),
            sender_id: "user1".to_string(),
            sender_name: "Test".to_string(),
            content: "/help".to_string(),
            thread_id: None,
            session_id: None,
            response: None,
            context: Default::default(),
        };

        let result = manager.trigger(&event).await;
        assert!(result.reply.is_none());
    }

    #[test]
    fn wildcard_matches_message_events() {
        assert!(super::event_matches_pattern(
            "message:received",
            "message:*"
        ));
        assert!(super::event_matches_pattern(
            "message:before_agent",
            "message:*"
        ));
        assert!(super::event_matches_pattern(
            "message:after_agent",
            "message:*"
        ));
    }

    #[test]
    fn wildcard_star_matches_all() {
        assert!(super::event_matches_pattern("message:received", "*"));
        assert!(super::event_matches_pattern("session:created", "*"));
        assert!(super::event_matches_pattern("custom:anything", "*"));
    }

    #[test]
    fn exact_match_works() {
        assert!(super::event_matches_pattern(
            "session:created",
            "session:created"
        ));
        assert!(!super::event_matches_pattern(
            "session:ended",
            "session:created"
        ));
    }

    #[test]
    fn wildcard_no_partial() {
        // "message:*" must NOT match "messagefoo" (no colon separator)
        assert!(!super::event_matches_pattern("messagefoo", "message:*"));
    }

    #[tokio::test]
    async fn test_auto_reply_integration() {
        let config = HooksConfig {
            enabled: true,
            path: None,
            auto_reply: vec![AutoReplyRule {
                pattern: "^/ping$".to_string(),
                reply: "pong".to_string(),
                channels: vec![],
                skip_agent: true,
                case_insensitive: true,
            }],
        };

        let temp_dir = tempfile::TempDir::new().unwrap();
        let manager = HookManager::new(&config, temp_dir.path());

        let event = HookEvent {
            action: "message:received".to_string(),
            channel: "test".to_string(),
            channel_id: "ch1".to_string(),
            message_id: "msg1".to_string(),
            sender_id: "user1".to_string(),
            sender_name: "Test".to_string(),
            content: "/ping".to_string(),
            thread_id: None,
            session_id: None,
            response: None,
            context: Default::default(),
        };

        let result = manager.trigger(&event).await;
        assert_eq!(result.reply, Some("pong".to_string()));
        assert!(result.skip_agent);
    }
}
