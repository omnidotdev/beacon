//! AI response moderation via Say Less.
//!
//! Beacon screens assembled AI responses through the Say Less `/check` endpoint
//! before delivery. Checks fail OPEN: when moderation is unconfigured or the
//! provider errors, content is allowed so a Say Less outage never blocks replies.

use serde_json::{Value, json};

/// Screen `text` through Say Less. Returns `true` when the content is flagged.
///
/// Fails open (returns `false`) on empty input, non-success responses, or
/// transport errors.
pub async fn is_flagged(say_less_url: &str, text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{say_less_url}/check"))
        .json(&json!({ "text": text }))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => resp
            .json::<Value>()
            .await
            .ok()
            .and_then(|body| {
                body.get("result")
                    .and_then(|result| result.get("flagged"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false),
        Ok(resp) => {
            tracing::warn!(status = %resp.status(), "Say Less returned non-success; allowing");
            false
        }
        Err(error) => {
            tracing::warn!(error = %error, "Say Less check failed; allowing");
            false
        }
    }
}
