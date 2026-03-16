//! Line messaging channel adapter
//!
//! Receives messages via a webhook (Line Messaging API) and sends replies via the push API

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::mpsc;

use super::{Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

type HmacSha256 = Hmac<Sha256>;

/// Line connection configuration
#[derive(Debug, Clone)]
pub struct LineConfig {
    /// Line channel access token
    pub channel_access_token: String,
    /// Line channel secret (for webhook signature verification)
    pub channel_secret: String,
    /// Port for the inbound webhook HTTP server
    pub webhook_port: u16,
}

/// Line messaging channel adapter
pub struct LineChannel {
    config: LineConfig,
    client: reqwest::Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: Arc<AtomicBool>,
}

impl LineChannel {
    /// Create a new Line channel adapter
    #[must_use]
    pub fn new(config: LineConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            message_tx: None,
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(config: LineConfig) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            config,
            client: reqwest::Client::new(),
            message_tx: Some(tx),
            connected: Arc::new(AtomicBool::new(false)),
        };
        (channel, rx)
    }
}

/// Verify Line webhook signature
///
/// Computes HMAC-SHA256 of the raw body using the channel secret and compares
/// against the base64-encoded `x-line-signature` header value.
#[must_use]
pub fn verify_signature(channel_secret: &str, body: &[u8], signature: &str) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(channel_secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let expected = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    expected == signature
}

/// Top-level webhook request body
#[derive(Debug, serde::Deserialize)]
pub struct LineWebhookBody {
    /// Array of webhook events
    pub events: Vec<LineEvent>,
}

/// A single Line webhook event
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineEvent {
    /// Event type (e.g. "message", "follow", "unfollow")
    #[serde(rename = "type")]
    pub event_type: String,
    /// Reply token for responding to this event
    pub reply_token: Option<String>,
    /// Source of the event
    pub source: Option<LineSource>,
    /// Message payload (present when `event_type` is "message")
    pub message: Option<LineMessage>,
}

/// Source of a Line event
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSource {
    /// Source type ("user", "group", "room")
    #[serde(rename = "type")]
    pub source_type: String,
    /// User ID
    pub user_id: Option<String>,
}

/// Line message payload
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMessage {
    /// Message type ("text", "image", "video", etc.)
    #[serde(rename = "type")]
    pub message_type: String,
    /// Message ID
    pub id: String,
    /// Text content (present when `message_type` is "text")
    pub text: Option<String>,
}

/// Shared state for the webhook axum server
#[derive(Clone)]
struct WebhookState {
    tx: mpsc::Sender<IncomingMessage>,
    channel_secret: String,
}

/// Handle incoming Line webhook POST
async fn handle_webhook(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Verify signature
    let Some(signature) = headers
        .get("x-line-signature")
        .and_then(|v| v.to_str().ok())
    else {
        tracing::warn!("Line webhook: missing x-line-signature header");
        return StatusCode::UNAUTHORIZED;
    };

    if !verify_signature(&state.channel_secret, &body, signature) {
        tracing::warn!("Line webhook: invalid signature");
        return StatusCode::UNAUTHORIZED;
    }

    // Parse body
    let webhook_body: LineWebhookBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "Line webhook: failed to parse body");
            return StatusCode::BAD_REQUEST;
        }
    };

    for event in webhook_body.events {
        if event.event_type != "message" {
            continue;
        }

        let Some(message) = event.message else {
            continue;
        };

        if message.message_type != "text" {
            continue;
        }

        let Some(text) = message.text else {
            continue;
        };

        let sender_id = event
            .source
            .as_ref()
            .and_then(|s| s.user_id.clone())
            .unwrap_or_default();

        let incoming = IncomingMessage {
            id: message.id,
            channel_id: sender_id.clone(),
            sender_id: sender_id.clone(),
            sender_name: sender_id,
            content: text,
            is_dm: true,
            reply_to: event.reply_token,
            attachments: Vec::new(),
            thread_id: None,
            callback_data: None,
        };

        if let Err(e) = state.tx.send(incoming).await {
            tracing::warn!(error = %e, "failed to forward Line message");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }

    StatusCode::OK
}

#[async_trait]
impl Channel for LineChannel {
    fn name(&self) -> &'static str {
        "line"
    }

    fn capabilities(&self) -> &'static [super::ChannelCapability] {
        &[]
    }

    async fn connect(&mut self) -> Result<()> {
        // Start webhook server for inbound messages
        if let Some(tx) = &self.message_tx {
            let state = WebhookState {
                tx: tx.clone(),
                channel_secret: self.config.channel_secret.clone(),
            };
            let app = axum::Router::new()
                .route("/line/webhook", post(handle_webhook))
                .with_state(state);

            let port = self.config.webhook_port;
            let connected = Arc::clone(&self.connected);
            tokio::spawn(async move {
                let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::error!(error = %e, port, "failed to bind Line webhook server");
                        return;
                    }
                };

                tracing::info!(port, "Line webhook server listening");

                if let Err(e) = axum::serve(listener, app).await
                    && connected.load(Ordering::Relaxed)
                {
                    tracing::error!(error = %e, "Line webhook server error");
                }
            });
        }

        self.connected.store(true, Ordering::Relaxed);
        tracing::info!(
            webhook_port = self.config.webhook_port,
            "Line channel connected"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected.store(false, Ordering::Relaxed);
        tracing::info!("Line channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let body = serde_json::json!({
            "to": message.channel_id,
            "messages": [{"type": "text", "text": message.content}],
        });

        let resp = self
            .client
            .post("https://api.line.me/v2/bot/message/push")
            .bearer_auth(&self.config.channel_access_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Line push message failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Line push message returned {status}: {resp_body}"
            )));
        }

        tracing::debug!(to = %message.channel_id, "Line message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_channel_name() {
        let config = LineConfig {
            channel_access_token: "token_test".to_string(),
            channel_secret: "secret_test".to_string(),
            webhook_port: 8092,
        };

        let channel = LineChannel::new(config);
        assert_eq!(channel.name(), "line");
        assert!(!channel.is_connected());
    }

    #[test]
    fn verify_line_signature_valid() {
        let secret = "test_channel_secret";
        let body = b"test body content";

        // Compute the expected signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let signature =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        assert!(verify_signature(secret, body, &signature));
    }

    #[test]
    fn verify_line_signature_invalid() {
        let secret = "test_channel_secret";
        let body = b"test body content";
        let bad_signature = "aW52YWxpZF9zaWduYXR1cmU=";

        assert!(!verify_signature(secret, body, bad_signature));
    }

    #[test]
    fn parse_line_message_event() {
        let json = r#"{
            "events": [{
                "type": "message",
                "replyToken": "reply_tok_123",
                "source": {
                    "type": "user",
                    "userId": "U1234567890abcdef"
                },
                "message": {
                    "type": "text",
                    "id": "msg_001",
                    "text": "Hello from Line"
                }
            }]
        }"#;

        let body: LineWebhookBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.events.len(), 1);

        let event = &body.events[0];
        assert_eq!(event.event_type, "message");
        assert_eq!(event.reply_token.as_deref(), Some("reply_tok_123"));

        let source = event.source.as_ref().unwrap();
        assert_eq!(source.user_id.as_deref(), Some("U1234567890abcdef"));

        let message = event.message.as_ref().unwrap();
        assert_eq!(message.message_type, "text");
        assert_eq!(message.id, "msg_001");
        assert_eq!(message.text.as_deref(), Some("Hello from Line"));
    }
}
