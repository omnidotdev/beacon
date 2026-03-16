//! Feishu (Lark) channel adapter
//!
//! Receives messages via HTTP webhook callbacks and sends replies via the Feishu REST API

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use axum::extract::State;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, mpsc};

use super::{Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Feishu bot connection configuration
#[derive(Debug, Clone)]
pub struct FeishuConfig {
    /// Feishu app ID
    pub app_id: String,
    /// Feishu app secret
    pub app_secret: String,
    /// Verification token for webhook validation
    pub verification_token: String,
    /// Port for the inbound webhook HTTP server
    pub webhook_port: u16,
}

/// Feishu channel adapter
pub struct FeishuChannel {
    config: FeishuConfig,
    client: reqwest::Client,
    access_token: Arc<RwLock<Option<String>>>,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: Arc<AtomicBool>,
}

impl FeishuChannel {
    /// Create a new Feishu channel adapter
    #[must_use]
    pub fn new(config: FeishuConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            access_token: Arc::new(RwLock::new(None)),
            message_tx: None,
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(config: FeishuConfig) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            config,
            client: reqwest::Client::new(),
            access_token: Arc::new(RwLock::new(None)),
            message_tx: Some(tx),
            connected: Arc::new(AtomicBool::new(false)),
        };
        (channel, rx)
    }
}

/// Tenant access token request body
#[derive(Serialize)]
struct TenantTokenRequest {
    app_id: String,
    app_secret: String,
}

/// Tenant access token response
#[derive(Deserialize)]
struct TenantTokenResponse {
    code: i32,
    tenant_access_token: Option<String>,
}

/// Feishu webhook event envelope
#[derive(Debug, Deserialize)]
struct FeishuWebhookPayload {
    /// URL verification challenge (present during setup)
    challenge: Option<String>,
    /// Event header
    header: Option<FeishuEventHeader>,
    /// Event body
    event: Option<serde_json::Value>,
}

/// Event header with type info
#[derive(Debug, Deserialize)]
struct FeishuEventHeader {
    event_type: String,
}

/// Feishu message content (nested JSON)
#[derive(Debug, Deserialize)]
pub struct FeishuTextContent {
    /// Message text
    pub text: String,
}

/// Parse Feishu's nested JSON message content string
///
/// Feishu sends message content as a JSON-encoded string inside the event payload
#[must_use]
pub fn parse_feishu_text_content(content_json: &str) -> Option<String> {
    serde_json::from_str::<FeishuTextContent>(content_json)
        .ok()
        .map(|c| c.text)
}

/// Shared state for the webhook axum server
#[derive(Clone)]
struct WebhookState {
    tx: mpsc::Sender<IncomingMessage>,
}

/// Build the challenge response JSON for URL verification
#[must_use]
pub fn build_challenge_response(challenge: &str) -> serde_json::Value {
    serde_json::json!({ "challenge": challenge })
}

/// Handle incoming Feishu webhook POST
async fn handle_webhook(
    State(state): State<WebhookState>,
    axum::Json(payload): axum::Json<FeishuWebhookPayload>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // URL verification challenge
    if let Some(challenge) = &payload.challenge {
        let resp = build_challenge_response(challenge);
        return axum::Json(resp).into_response();
    }

    // Only process message events
    let Some(header) = &payload.header else {
        return StatusCode::OK.into_response();
    };

    if header.event_type != "im.message.receive_v1" {
        return StatusCode::OK.into_response();
    }

    let Some(event) = &payload.event else {
        return StatusCode::OK.into_response();
    };

    // Extract sender open_id
    let sender_id = event
        .pointer("/sender/sender_id/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Extract message fields
    let message = event.get("message");
    let content_str = message
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let text = parse_feishu_text_content(content_str).unwrap_or_default();

    let chat_id = message
        .and_then(|m| m.get("chat_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let message_id = message
        .and_then(|m| m.get("message_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let incoming = IncomingMessage {
        id: message_id,
        channel_id: chat_id,
        sender_id: sender_id.clone(),
        sender_name: sender_id,
        content: text,
        is_dm: false,
        reply_to: None,
        attachments: Vec::new(),
        thread_id: None,
        callback_data: None,
    };

    if let Err(e) = state.tx.send(incoming).await {
        tracing::warn!(error = %e, "failed to forward Feishu message");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    StatusCode::OK.into_response()
}

#[async_trait]
impl Channel for FeishuChannel {
    fn name(&self) -> &'static str {
        "feishu"
    }

    fn capabilities(&self) -> &'static [super::ChannelCapability] {
        &[]
    }

    async fn connect(&mut self) -> Result<()> {
        // Get tenant access token
        let resp = self
            .client
            .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal/")
            .json(&TenantTokenRequest {
                app_id: self.config.app_id.clone(),
                app_secret: self.config.app_secret.clone(),
            })
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Feishu token request failed: {e}")))?;

        let token_resp: TenantTokenResponse = resp
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Feishu token response parse failed: {e}")))?;

        if token_resp.code != 0 {
            return Err(Error::Channel(format!(
                "Feishu token request returned code {}",
                token_resp.code
            )));
        }

        let token = token_resp
            .tenant_access_token
            .ok_or_else(|| Error::Channel("Feishu token response missing token".to_string()))?;

        {
            let mut guard = self.access_token.write().await;
            *guard = Some(token);
        }

        // Start webhook server for inbound messages
        if let Some(tx) = &self.message_tx {
            let state = WebhookState { tx: tx.clone() };
            let app = axum::Router::new()
                .route("/feishu/event", post(handle_webhook))
                .with_state(state);

            let port = self.config.webhook_port;
            let connected = Arc::clone(&self.connected);
            tokio::spawn(async move {
                let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::error!(error = %e, port, "failed to bind Feishu webhook server");
                        return;
                    }
                };

                tracing::info!(port, "Feishu webhook server listening");

                if let Err(e) = axum::serve(listener, app).await
                    && connected.load(Ordering::Relaxed)
                {
                    tracing::error!(error = %e, "Feishu webhook server error");
                }
            });
        }

        self.connected.store(true, Ordering::Relaxed);
        tracing::info!(
            webhook_port = self.config.webhook_port,
            "Feishu channel connected"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected.store(false, Ordering::Relaxed);
        *self.access_token.write().await = None;
        tracing::info!("Feishu channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let token =
            self.access_token.read().await.clone().ok_or_else(|| {
                Error::Channel("Feishu not connected (no access token)".to_string())
            })?;

        let content = serde_json::json!({ "text": message.content }).to_string();

        let resp = self
            .client
            .post("https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id")
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "receive_id": message.channel_id,
                "msg_type": "text",
                "content": content,
            }))
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Feishu message send failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Feishu message send returned {status}: {body}"
            )));
        }

        tracing::debug!(chat_id = %message.channel_id, "Feishu message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> FeishuConfig {
        FeishuConfig {
            app_id: "cli_test123".to_string(),
            app_secret: "secret_test".to_string(),
            verification_token: "verify_test".to_string(),
            webhook_port: 8091,
        }
    }

    #[test]
    fn feishu_channel_name() {
        let channel = FeishuChannel::new(test_config());
        assert_eq!(channel.name(), "feishu");
        assert!(!channel.is_connected());
    }

    #[test]
    fn parse_feishu_text_content_valid() {
        let content = r#"{"text":"Hello from Feishu"}"#;
        let result = parse_feishu_text_content(content);
        assert_eq!(result, Some("Hello from Feishu".to_string()));
    }

    #[test]
    fn parse_feishu_text_content_invalid() {
        let result = parse_feishu_text_content("not json");
        assert_eq!(result, None);
    }

    #[test]
    fn parse_feishu_text_content_missing_field() {
        let result = parse_feishu_text_content(r#"{"other":"value"}"#);
        assert_eq!(result, None);
    }

    #[test]
    fn feishu_challenge_response() {
        let resp = build_challenge_response("test_challenge_abc");
        assert_eq!(resp["challenge"], "test_challenge_abc");
    }
}
