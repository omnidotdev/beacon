//! Map Synapse gateway failures onto user-facing outcomes
//!
//! Managed AI runs through a product-scoped Synapse key, and the Synapse
//! gateway is the only place credits are checked and charged. It refuses a
//! request with 402 before any spend when the pooled Omni balance is empty,
//! and with 503 when billing is unavailable. Beacon never retries a refused
//! request on another model; it surfaces the outcome to the user instead.
//!
//! The Synapse client flattens the HTTP status into its error text
//! (`"402 insufficient_credits_error: ..."`), and agent-core wraps that again,
//! so classification works on the rendered error text.

/// WebSocket / HTTP error code for an empty credit balance
pub const CODE_OUT_OF_CREDITS: &str = "out_of_credits";

/// WebSocket / HTTP error code for managed AI being temporarily unavailable
pub const CODE_AI_UNAVAILABLE: &str = "ai_unavailable";

/// Where users buy credits or upgrade when the env does not override it
pub const DEFAULT_BILLING_URL: &str = "https://beacon.omni.dev/settings";

/// User-facing copy when managed AI is temporarily unavailable
pub const UNAVAILABLE_MESSAGE: &str =
    "Beacon is temporarily unavailable. Please try again in a moment.";

/// Generic channel reply for failures that are not billing related
pub const GENERIC_ERROR_MESSAGE: &str = "Sorry, I encountered an error processing your message.";

/// A gateway refusal that needs a dedicated user-facing outcome
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayFailure {
    /// 402: the pooled credit balance cannot cover the request
    OutOfCredits,
    /// 503: billing or the gateway is unavailable, nothing was charged
    Unavailable,
    /// 401: the managed key was rejected (rotated or revoked)
    Unauthorized,
}

impl GatewayFailure {
    /// Stable error code sent to clients
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::OutOfCredits => CODE_OUT_OF_CREDITS,
            Self::Unavailable | Self::Unauthorized => CODE_AI_UNAVAILABLE,
        }
    }

    /// User-facing message for this failure
    #[must_use]
    pub fn user_message(self) -> String {
        match self {
            Self::OutOfCredits => out_of_credits_message(&billing_url()),
            Self::Unavailable | Self::Unauthorized => UNAVAILABLE_MESSAGE.to_string(),
        }
    }
}

/// Classify a rendered Synapse / agent error, or `None` when it is not a
/// billing-relevant gateway refusal
#[must_use]
pub fn classify(error_text: &str) -> Option<GatewayFailure> {
    // Typed error names from the gateway's JSON error body win over status
    // parsing, since they survive any wrapping
    if error_text.contains("insufficient_credits_error") {
        return Some(GatewayFailure::OutOfCredits);
    }
    if error_text.contains("billing_unavailable_error") {
        return Some(GatewayFailure::Unavailable);
    }

    first_status_token(error_text).and_then(|status| match status {
        402 => Some(GatewayFailure::OutOfCredits),
        503 => Some(GatewayFailure::Unavailable),
        401 => Some(GatewayFailure::Unauthorized),
        _ => None,
    })
}

/// Find the first standalone 401 / 402 / 503 token, the shape the Synapse
/// client renders (`"402 type: msg"`, `"API error (402): ..."`)
fn first_status_token(text: &str) -> Option<u16> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|token| token.len() == 3)
        .find_map(|token| match token {
            "401" => Some(401),
            "402" => Some(402),
            "503" => Some(503),
            _ => None,
        })
}

/// Billing page users are sent to for credits or an upgrade
#[must_use]
pub fn billing_url() -> String {
    std::env::var("BEACON_BILLING_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BILLING_URL.to_string())
}

/// Out-of-credits copy with a link to buy credits or upgrade
#[must_use]
pub fn out_of_credits_message(billing_url: &str) -> String {
    format!(
        "You're out of Omni credits (your balance is shared across Omni). \
         Buy credits or upgrade your plan to keep going: {billing_url}"
    )
}

/// Reply text for a messaging channel (Telegram, Slack, Teams, etc.) after a
/// failed model call
#[must_use]
pub fn channel_reply(error_text: &str) -> String {
    classify(error_text).map_or_else(
        || GENERIC_ERROR_MESSAGE.to_string(),
        GatewayFailure::user_message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_synapse_client_402() {
        let text = "402 insufficient_credits_error: insufficient credits: balance too low";
        assert_eq!(classify(text), Some(GatewayFailure::OutOfCredits));
    }

    #[test]
    fn classifies_402_wrapped_by_agent_core() {
        // agent-core drops the status to 0 and keeps the client text
        let text = "agent error: API error (0): 402 unknown: payment required";
        assert_eq!(classify(text), Some(GatewayFailure::OutOfCredits));
    }

    #[test]
    fn classifies_typed_insufficient_credits_without_status() {
        let text = "stream error: insufficient_credits_error";
        assert_eq!(classify(text), Some(GatewayFailure::OutOfCredits));
    }

    #[test]
    fn classifies_503_and_billing_unavailable() {
        assert_eq!(
            classify("503 billing_unavailable_error: billing service unavailable"),
            Some(GatewayFailure::Unavailable)
        );
        assert_eq!(
            classify("503 unknown: upstream down"),
            Some(GatewayFailure::Unavailable)
        );
    }

    #[test]
    fn classifies_401_as_rejected_key() {
        assert_eq!(
            classify("401 authentication_error: authentication required"),
            Some(GatewayFailure::Unauthorized)
        );
    }

    #[test]
    fn ignores_other_failures_and_embedded_digits() {
        assert_eq!(
            classify("500 internal_error: an internal error occurred"),
            None
        );
        assert_eq!(classify("429 rate_limit_error: rate limit exceeded"), None);
        // A longer number containing 402 is not a status token
        assert_eq!(classify("upstream error: request 14021 failed"), None);
    }

    #[test]
    fn out_of_credits_copy_links_to_billing() {
        let msg = out_of_credits_message("https://beacon.omni.dev/settings");
        assert!(msg.contains("https://beacon.omni.dev/settings"));
        assert!(msg.contains("shared across Omni"));
    }

    #[test]
    fn channel_reply_maps_each_outcome() {
        assert!(channel_reply("402 insufficient_credits_error: x").contains("out of Omni credits"));
        assert_eq!(channel_reply("503 unknown: x"), UNAVAILABLE_MESSAGE);
        assert_eq!(
            channel_reply("500 internal_error: x"),
            GENERIC_ERROR_MESSAGE
        );
    }

    #[test]
    fn codes_are_stable() {
        assert_eq!(GatewayFailure::OutOfCredits.code(), "out_of_credits");
        assert_eq!(GatewayFailure::Unavailable.code(), "ai_unavailable");
        assert_eq!(GatewayFailure::Unauthorized.code(), "ai_unavailable");
    }
}
