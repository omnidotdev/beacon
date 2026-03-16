//! Twilio SMS channel adapter
//!
//! Sends SMS via the Twilio REST API and receives inbound SMS via an axum webhook server

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use axum::extract::State;
use axum::routing::post;
use tokio::sync::mpsc;

use super::{Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Twilio connection configuration
#[derive(Debug, Clone)]
pub struct TwilioConfig {
    /// Twilio Account SID
    pub account_sid: String,
    /// Twilio Auth Token
    pub auth_token: String,
    /// Phone number to send from (E.164 format, e.g. "+1234567890")
    pub phone_number: String,
    /// Port for the inbound webhook HTTP server
    pub webhook_port: u16,
}

/// Twilio SMS channel adapter
pub struct TwilioChannel {
    config: TwilioConfig,
    client: reqwest::Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: Arc<AtomicBool>,
}

impl TwilioChannel {
    /// Create a new Twilio channel adapter
    #[must_use]
    pub fn new(config: TwilioConfig) -> Self {
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
    pub fn with_receiver(config: TwilioConfig) -> (Self, mpsc::Receiver<IncomingMessage>) {
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

/// Shared state for the webhook axum server
#[derive(Clone)]
struct WebhookState {
    tx: mpsc::Sender<IncomingMessage>,
}

/// Form data Twilio sends to the webhook endpoint
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TwilioIncomingSms {
    /// Sender phone number
    pub from: String,
    /// Recipient phone number
    pub to: String,
    /// Message body
    pub body: String,
    /// Twilio message SID
    pub message_sid: String,
}

impl TwilioIncomingSms {
    /// Convert to an `IncomingMessage`
    #[must_use]
    pub fn into_incoming_message(self) -> IncomingMessage {
        IncomingMessage {
            id: self.message_sid,
            channel_id: self.from.clone(),
            sender_id: self.from.clone(),
            sender_name: self.from,
            content: self.body,
            is_dm: true,
            reply_to: None,
            attachments: Vec::new(),
            thread_id: None,
            callback_data: None,
        }
    }
}

/// Build the form body for sending an SMS via the Twilio REST API
#[must_use]
pub fn build_sms_form_body(to: &str, from: &str, body: &str) -> Vec<(String, String)> {
    vec![
        ("To".to_string(), to.to_string()),
        ("From".to_string(), from.to_string()),
        ("Body".to_string(), body.to_string()),
    ]
}

/// Handle incoming SMS webhook POST
async fn handle_webhook(
    State(state): State<WebhookState>,
    axum::Form(sms): axum::Form<TwilioIncomingSms>,
) -> axum::http::StatusCode {
    let incoming = sms.into_incoming_message();

    if let Err(e) = state.tx.send(incoming).await {
        tracing::warn!(error = %e, "failed to forward Twilio SMS");
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
    }

    // Return empty TwiML response
    axum::http::StatusCode::OK
}

#[async_trait]
impl Channel for TwilioChannel {
    fn name(&self) -> &'static str {
        "twilio"
    }

    fn capabilities(&self) -> &'static [super::ChannelCapability] {
        &[]
    }

    async fn connect(&mut self) -> Result<()> {
        // Validate credentials by fetching account info
        let url = format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}.json",
            self.config.account_sid
        );

        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.config.account_sid, Some(&self.config.auth_token))
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Twilio credential validation failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(Error::Channel(format!(
                "Twilio credential validation returned {}",
                resp.status()
            )));
        }

        // Start webhook server for inbound SMS
        if let Some(tx) = &self.message_tx {
            let state = WebhookState { tx: tx.clone() };
            let app = axum::Router::new()
                .route("/twilio/sms", post(handle_webhook))
                .with_state(state);

            let port = self.config.webhook_port;
            let connected = Arc::clone(&self.connected);
            tokio::spawn(async move {
                let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::error!(error = %e, port, "failed to bind Twilio webhook server");
                        return;
                    }
                };

                tracing::info!(port, "Twilio webhook server listening");

                if let Err(e) = axum::serve(listener, app).await
                    && connected.load(Ordering::Relaxed)
                {
                    tracing::error!(error = %e, "Twilio webhook server error");
                }
            });
        }

        self.connected.store(true, Ordering::Relaxed);
        tracing::info!(
            phone = %self.config.phone_number,
            webhook_port = self.config.webhook_port,
            "Twilio channel connected"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected.store(false, Ordering::Relaxed);
        tracing::info!("Twilio channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let url = format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
            self.config.account_sid
        );

        let form = build_sms_form_body(
            &message.channel_id,
            &self.config.phone_number,
            &message.content,
        );

        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.config.account_sid, Some(&self.config.auth_token))
            .form(&form)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Twilio SMS send failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Twilio SMS send returned {status}: {body}"
            )));
        }

        tracing::debug!(to = %message.channel_id, "Twilio SMS sent");
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
    fn twilio_channel_name() {
        let config = TwilioConfig {
            account_sid: "AC_test".to_string(),
            auth_token: "token_test".to_string(),
            phone_number: "+15551234567".to_string(),
            webhook_port: 8090,
        };

        let channel = TwilioChannel::new(config);
        assert_eq!(channel.name(), "twilio");
        assert!(!channel.is_connected());
    }

    #[test]
    fn build_sms_form_body_correct() {
        let form = build_sms_form_body("+15559876543", "+15551234567", "Hello from Beacon");
        assert_eq!(form.len(), 3);
        assert_eq!(form[0], ("To".to_string(), "+15559876543".to_string()));
        assert_eq!(form[1], ("From".to_string(), "+15551234567".to_string()));
        assert_eq!(
            form[2],
            ("Body".to_string(), "Hello from Beacon".to_string())
        );
    }

    #[test]
    fn parse_twilio_incoming() {
        let sms = TwilioIncomingSms {
            from: "+15559876543".to_string(),
            to: "+15551234567".to_string(),
            body: "Hey there".to_string(),
            message_sid: "SM_abc123".to_string(),
        };

        let msg = sms.into_incoming_message();
        assert_eq!(msg.id, "SM_abc123");
        assert_eq!(msg.channel_id, "+15559876543");
        assert_eq!(msg.sender_id, "+15559876543");
        assert_eq!(msg.sender_name, "+15559876543");
        assert_eq!(msg.content, "Hey there");
        assert!(msg.is_dm);
        assert!(msg.reply_to.is_none());
        assert!(msg.attachments.is_empty());
    }
}
