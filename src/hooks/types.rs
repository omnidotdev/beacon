//! Hook system types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Hook event actions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookAction {
    /// After pairing check, before user lookup
    MessageReceived,
    /// After context built, before agent call
    BeforeAgent,
    /// After agent response, before send
    AfterAgent,
    /// When a new session is created
    SessionCreated,
    /// When a session ends
    SessionEnded,
    /// When a channel successfully connects
    ChannelConnected,
    /// When a channel disconnects
    ChannelDisconnected,
}

impl HookAction {
    /// Parse from string like "message:received"
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "message:received" | "message" => Some(Self::MessageReceived),
            "message:before_agent" => Some(Self::BeforeAgent),
            "message:after_agent" => Some(Self::AfterAgent),
            "session:created" => Some(Self::SessionCreated),
            "session:ended" => Some(Self::SessionEnded),
            "channel:connected" => Some(Self::ChannelConnected),
            "channel:disconnected" => Some(Self::ChannelDisconnected),
            _ => None,
        }
    }

    /// Convert to string representation
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::MessageReceived => "message:received",
            Self::BeforeAgent => "message:before_agent",
            Self::AfterAgent => "message:after_agent",
            Self::SessionCreated => "session:created",
            Self::SessionEnded => "session:ended",
            Self::ChannelConnected => "channel:connected",
            Self::ChannelDisconnected => "channel:disconnected",
        }
    }
}

/// Event data passed to hooks
#[derive(Debug, Clone, Serialize)]
pub struct HookEvent {
    /// Event action type
    pub action: String,
    /// Channel name (discord, slack, etc)
    pub channel: String,
    /// Channel-specific ID
    pub channel_id: String,
    /// Message ID
    pub message_id: String,
    /// Sender identifier
    pub sender_id: String,
    /// Sender display name
    pub sender_name: String,
    /// Message content
    pub content: String,
    /// Thread ID if in a thread (`thread_id`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Session ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Agent response (only for `after_agent`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    /// Additional context
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub context: HashMap<String, String>,
}

impl HookEvent {
    /// Create a new hook event
    #[must_use]
    pub fn new(action: HookAction, channel: &str, msg: &crate::channels::IncomingMessage) -> Self {
        Self {
            action: action.as_str().to_string(),
            channel: channel.to_string(),
            channel_id: msg.channel_id.clone(),
            message_id: msg.id.clone(),
            sender_id: msg.sender_id.clone(),
            sender_name: msg.sender_name.clone(),
            content: msg.content.clone(),
            thread_id: msg.reply_to.clone(),
            session_id: None,
            response: None,
            context: HashMap::new(),
        }
    }

    /// Create a lifecycle event for channel connect/disconnect (no message required)
    #[must_use]
    pub fn channel_lifecycle(action: HookAction, channel_name: &str) -> Self {
        Self {
            action: action.as_str().to_string(),
            channel: channel_name.to_string(),
            channel_id: String::new(),
            message_id: String::new(),
            sender_id: String::new(),
            sender_name: String::new(),
            content: String::new(),
            thread_id: None,
            session_id: None,
            response: None,
            context: HashMap::new(),
        }
    }

    /// Set session ID
    #[must_use]
    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    /// Set agent response (for `after_agent` events)
    #[must_use]
    pub fn with_response(mut self, response: &str) -> Self {
        self.response = Some(response.to_string());
        self
    }

    /// Add context key-value pair
    #[must_use]
    pub fn with_context(mut self, key: &str, value: &str) -> Self {
        self.context.insert(key.to_string(), value.to_string());
        self
    }
}

/// Result from hook execution
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookResult {
    /// Skip further processing of this message
    #[serde(default)]
    pub skip_processing: bool,
    /// Skip agent call (but continue with reply if provided)
    #[serde(default)]
    pub skip_agent: bool,
    /// Direct reply to send (bypasses or replaces agent)
    #[serde(default)]
    pub reply: Option<String>,
    /// Modified response (for `after_agent` hooks)
    #[serde(default)]
    pub modified_response: Option<String>,
    /// Messages to log/display
    #[serde(default)]
    pub messages: Vec<String>,
}

impl HookResult {
    /// Merge another result into this one
    pub fn merge(&mut self, other: Self) {
        // Later hooks can override skip flags
        if other.skip_processing {
            self.skip_processing = true;
        }
        if other.skip_agent {
            self.skip_agent = true;
        }
        // Later replies override earlier ones
        if other.reply.is_some() {
            self.reply = other.reply;
        }
        if other.modified_response.is_some() {
            self.modified_response = other.modified_response;
        }
        // Accumulate messages
        self.messages.extend(other.messages);
    }
}

/// Hook handler metadata from HOOK.toml
#[derive(Debug, Clone, Deserialize)]
pub struct HookManifest {
    /// Hook name
    pub name: String,
    /// Description
    #[serde(default)]
    #[allow(dead_code)]
    pub description: String,
    /// Events this hook subscribes to
    #[serde(default)]
    pub events: Vec<String>,
    /// Whether hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Requirements
    #[serde(default)]
    pub requires: HookRequirements,
}

const fn default_true() -> bool {
    true
}

/// Hook requirements for eligibility checking
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookRequirements {
    /// Required binaries in PATH
    #[serde(default)]
    pub bins: Vec<String>,
    /// Required environment variables
    #[serde(default)]
    pub env: Vec<String>,
    /// Supported operating systems
    #[serde(default)]
    pub os: Vec<String>,
}

impl HookRequirements {
    /// Check if requirements are satisfied
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        // Check OS
        if !self.os.is_empty() {
            let current_os = std::env::consts::OS;
            if !self.os.iter().any(|os| os == current_os) {
                return false;
            }
        }

        // Check binaries
        for bin in &self.bins {
            if which::which(bin).is_err() {
                return false;
            }
        }

        // Check env vars
        for var in &self.env {
            if std::env::var(var).is_err() {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_created_action() {
        let action = HookAction::from_str("session:created");
        assert!(matches!(action, Some(HookAction::SessionCreated)));
    }

    #[test]
    fn parse_session_ended_action() {
        let action = HookAction::from_str("session:ended");
        assert!(matches!(action, Some(HookAction::SessionEnded)));
    }

    #[test]
    fn parse_channel_connected_action() {
        let action = HookAction::from_str("channel:connected");
        assert!(matches!(action, Some(HookAction::ChannelConnected)));
    }

    #[test]
    fn parse_channel_disconnected_action() {
        let action = HookAction::from_str("channel:disconnected");
        assert!(matches!(action, Some(HookAction::ChannelDisconnected)));
    }

    #[test]
    fn lifecycle_actions_roundtrip() {
        for (s, expected) in [
            ("session:created", HookAction::SessionCreated),
            ("session:ended", HookAction::SessionEnded),
            ("channel:connected", HookAction::ChannelConnected),
            ("channel:disconnected", HookAction::ChannelDisconnected),
        ] {
            let parsed = HookAction::from_str(s).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.as_str(), s);
        }
    }

    #[test]
    fn channel_lifecycle_event_fields() {
        let event = HookEvent::channel_lifecycle(HookAction::ChannelConnected, "discord");
        assert_eq!(event.action, "channel:connected");
        assert_eq!(event.channel, "discord");
        assert!(event.channel_id.is_empty());
        assert!(event.message_id.is_empty());
        assert!(event.sender_id.is_empty());
        assert!(event.session_id.is_none());
    }
}
