//! Gmail channel adapter
//!
//! Connects to Gmail via the Google API with `OAuth2` service account authentication.
//! Uses polling (not push) to check for new emails.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use google_gmail1::Gmail;
use google_gmail1::api::{Message, MessagePartHeader, ModifyMessageRequest};
use google_gmail1::hyper_rustls::HttpsConnector;
use google_gmail1::hyper_util::client::legacy::connect::HttpConnector;
use tokio::sync::mpsc;

use super::{Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Gmail connection configuration
#[derive(Debug, Clone)]
pub struct GmailConfig {
    /// Path to Google service account credentials JSON
    pub credentials_path: PathBuf,
    /// Seconds between polling for new messages
    pub poll_interval_secs: u64,
    /// Gmail labels to watch (e.g. `["INBOX"]`)
    pub labels: Vec<String>,
    /// User email address to act as
    pub user_email: String,
}

/// Gmail channel adapter
pub struct GmailChannel {
    config: GmailConfig,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: Arc<AtomicBool>,
}

impl GmailChannel {
    /// Create a new Gmail channel adapter
    #[must_use]
    pub fn new(config: GmailConfig) -> Self {
        Self {
            config,
            message_tx: None,
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(config: GmailConfig) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            config,
            message_tx: Some(tx),
            connected: Arc::new(AtomicBool::new(false)),
        };
        (channel, rx)
    }
}

/// Build an authenticated Gmail hub from a service account credentials file
async fn build_hub(
    credentials_path: &std::path::Path,
    user_email: &str,
) -> Result<Gmail<HttpsConnector<HttpConnector>>> {
    let secret = yup_oauth2::read_service_account_key(credentials_path)
        .await
        .map_err(|e| Error::Channel(format!("failed to read Gmail credentials: {e}")))?;

    let auth = yup_oauth2::ServiceAccountAuthenticator::builder(secret)
        .subject(user_email.to_string())
        .build()
        .await
        .map_err(|e| Error::Channel(format!("Gmail OAuth2 authentication failed: {e}")))?;

    let client = google_gmail1::hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| Error::Channel(format!("TLS setup failed: {e}")))?
        .https_or_http()
        .enable_http2()
        .build();

    let hub = Gmail::new(
        google_gmail1::hyper_util::client::legacy::Client::builder(
            google_gmail1::hyper_util::rt::TokioExecutor::new(),
        )
        .build(client),
        auth,
    );

    Ok(hub)
}

/// Extract a header value by name from a list of message part headers
#[must_use]
pub fn extract_header(headers: &[MessagePartHeader], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|h| {
            h.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|h| h.value.clone())
}

/// Recursively extract the text/plain body from a MIME message payload
#[must_use]
pub fn extract_text_body(payload: &google_gmail1::api::MessagePart) -> String {
    // Check if this part itself is text/plain
    if payload
        .mime_type
        .as_deref()
        .is_some_and(|m| m.eq_ignore_ascii_case("text/plain"))
        && let Some(body) = &payload.body
        && let Some(data) = &body.data
    {
        let data_str = String::from_utf8_lossy(data);
        if let Ok(decoded) = base64_url_decode(&data_str) {
            return decoded;
        }
    }

    // Recurse into child parts
    if let Some(parts) = &payload.parts {
        for part in parts {
            let text = extract_text_body(part);
            if !text.is_empty() {
                return text;
            }
        }
    }

    String::new()
}

/// Decode a base64url-encoded string (Gmail API format)
fn base64_url_decode(input: &str) -> std::result::Result<String, Error> {
    use base64::Engine as _;

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|e| Error::Channel(format!("base64url decode failed: {e}")))?;

    String::from_utf8(bytes).map_err(|e| Error::Channel(format!("UTF-8 decode failed: {e}")))
}

/// Encode bytes as base64url (Gmail API format)
fn base64_url_encode(input: &[u8]) -> String {
    use base64::Engine as _;

    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}

/// Compose an RFC 2822 email message
fn compose_email(
    to: &str,
    from: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> String {
    use std::fmt::Write as _;

    let mut msg = String::new();
    let _ = write!(msg, "To: {to}\r\n");
    let _ = write!(msg, "From: {from}\r\n");
    let _ = write!(msg, "Subject: {subject}\r\n");
    msg.push_str("MIME-Version: 1.0\r\n");
    msg.push_str("Content-Type: text/plain; charset=utf-8\r\n");

    if let Some(ref_id) = in_reply_to {
        let _ = write!(msg, "In-Reply-To: {ref_id}\r\n");
        let _ = write!(msg, "References: {ref_id}\r\n");
    }

    msg.push_str("\r\n");
    msg.push_str(body);
    msg
}

#[async_trait]
impl Channel for GmailChannel {
    fn name(&self) -> &'static str {
        "gmail"
    }

    fn capabilities(&self) -> &'static [super::ChannelCapability] {
        &[]
    }

    async fn connect(&mut self) -> Result<()> {
        let hub = build_hub(&self.config.credentials_path, &self.config.user_email).await?;

        self.connected.store(true, Ordering::Release);

        // Spawn polling loop for incoming messages
        if let Some(tx) = &self.message_tx {
            let tx = tx.clone();
            let connected = Arc::clone(&self.connected);
            let poll_interval = self.config.poll_interval_secs;
            let labels = self.config.labels.clone();

            tokio::spawn(async move {
                poll_loop(hub, tx, connected, poll_interval, &labels).await;
            });
        }

        tracing::info!(
            user = %self.config.user_email,
            poll_interval = self.config.poll_interval_secs,
            "Gmail channel connected"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected.store(false, Ordering::Release);
        tracing::info!("Gmail channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let hub = build_hub(&self.config.credentials_path, &self.config.user_email).await?;

        let subject = message.reply_to.as_ref().map_or_else(
            || String::from("Message from Beacon"),
            |_| String::from("Re: "),
        );

        let raw_email = compose_email(
            &message.channel_id,
            &self.config.user_email,
            &subject,
            &message.content,
            message.reply_to.as_deref(),
        );

        let encoded = base64_url_encode(raw_email.as_bytes());

        let msg = Message {
            raw: Some(encoded.into_bytes()),
            thread_id: message.thread_id.clone(),
            ..Message::default()
        };

        hub.users()
            .messages_send(msg, "me")
            .upload(
                std::io::empty(),
                "message/rfc822".parse().expect("valid MIME type literal"),
            )
            .await
            .map_err(|e| Error::Channel(format!("Gmail send failed: {e}")))?;

        tracing::debug!(to = %message.channel_id, "Gmail message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

/// Poll Gmail for unread messages
async fn poll_loop(
    hub: Gmail<HttpsConnector<HttpConnector>>,
    tx: mpsc::Sender<IncomingMessage>,
    connected: Arc<AtomicBool>,
    poll_interval_secs: u64,
    labels: &[String],
) {
    let interval = tokio::time::Duration::from_secs(poll_interval_secs);

    while connected.load(Ordering::Acquire) {
        if let Err(e) = poll_once(&hub, &tx, labels).await {
            tracing::warn!(error = %e, "Gmail poll error");
        }

        tokio::time::sleep(interval).await;
    }

    tracing::info!("Gmail polling loop ended");
}

/// Execute a single poll cycle: fetch unread messages, forward them, mark as read
async fn poll_once(
    hub: &Gmail<HttpsConnector<HttpConnector>>,
    tx: &mpsc::Sender<IncomingMessage>,
    labels: &[String],
) -> Result<()> {
    let mut req = hub
        .users()
        .messages_list("me")
        .q("is:unread")
        .max_results(20);

    for label in labels {
        req = req.add_label_ids(label);
    }

    let (_, list) = req
        .doit()
        .await
        .map_err(|e| Error::Channel(format!("Gmail list failed: {e}")))?;

    let Some(messages) = list.messages else {
        return Ok(());
    };

    for msg_stub in &messages {
        let Some(msg_id) = &msg_stub.id else {
            continue;
        };

        // Fetch the full message
        let (_, full_msg) = hub
            .users()
            .messages_get("me", msg_id)
            .format("full")
            .doit()
            .await
            .map_err(|e| Error::Channel(format!("Gmail get message failed: {e}")))?;

        let Some(payload) = &full_msg.payload else {
            continue;
        };

        let headers = payload.headers.as_deref().unwrap_or_default();
        let from = extract_header(headers, "From").unwrap_or_default();
        let subject = extract_header(headers, "Subject").unwrap_or_default();
        let message_id_header = extract_header(headers, "Message-ID");
        let body = extract_text_body(payload);

        let content = if subject.is_empty() {
            body
        } else {
            format!("[{subject}] {body}")
        };

        let incoming = IncomingMessage {
            id: msg_id.clone(),
            channel_id: from.clone(),
            sender_id: from.clone(),
            sender_name: from,
            content,
            is_dm: true,
            reply_to: message_id_header,
            attachments: Vec::new(),
            thread_id: full_msg.thread_id.clone(),
            callback_data: None,
        };

        if let Err(e) = tx.send(incoming).await {
            tracing::warn!(error = %e, "failed to forward Gmail message");
            break;
        }

        // Mark as read (remove UNREAD label)
        let modify = ModifyMessageRequest {
            remove_label_ids: Some(vec!["UNREAD".to_string()]),
            ..ModifyMessageRequest::default()
        };

        if let Err(e) = hub
            .users()
            .messages_modify(modify, "me", msg_id)
            .doit()
            .await
        {
            tracing::warn!(message_id = %msg_id, error = %e, "failed to mark Gmail message as read");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_channel_name() {
        let config = GmailConfig {
            credentials_path: PathBuf::from("/tmp/credentials.json"),
            poll_interval_secs: 30,
            labels: vec!["INBOX".to_string()],
            user_email: "test@example.com".to_string(),
        };

        let channel = GmailChannel::new(config);
        assert_eq!(channel.name(), "gmail");
        assert!(!channel.is_connected());
    }

    #[test]
    fn extract_header_finds_subject() {
        let headers = vec![
            MessagePartHeader {
                name: Some("From".to_string()),
                value: Some("alice@example.com".to_string()),
            },
            MessagePartHeader {
                name: Some("Subject".to_string()),
                value: Some("Hello world".to_string()),
            },
            MessagePartHeader {
                name: Some("Date".to_string()),
                value: Some("Mon, 1 Jan 2024 00:00:00 +0000".to_string()),
            },
        ];

        assert_eq!(
            extract_header(&headers, "Subject"),
            Some("Hello world".to_string())
        );
        assert_eq!(
            extract_header(&headers, "From"),
            Some("alice@example.com".to_string())
        );
        assert_eq!(extract_header(&headers, "X-Missing"), None);
    }

    #[test]
    fn extract_header_case_insensitive() {
        let headers = vec![MessagePartHeader {
            name: Some("Content-Type".to_string()),
            value: Some("text/plain".to_string()),
        }];

        assert_eq!(
            extract_header(&headers, "content-type"),
            Some("text/plain".to_string())
        );
    }

    #[test]
    fn compose_email_basic() {
        let email = compose_email(
            "bob@example.com",
            "alice@example.com",
            "Test Subject",
            "Hello Bob",
            None,
        );

        assert!(email.contains("To: bob@example.com"));
        assert!(email.contains("From: alice@example.com"));
        assert!(email.contains("Subject: Test Subject"));
        assert!(email.contains("Hello Bob"));
        assert!(!email.contains("In-Reply-To"));
    }

    #[test]
    fn compose_email_with_reply() {
        let email = compose_email(
            "bob@example.com",
            "alice@example.com",
            "Re: Test",
            "Reply body",
            Some("<msg123@example.com>"),
        );

        assert!(email.contains("In-Reply-To: <msg123@example.com>"));
        assert!(email.contains("References: <msg123@example.com>"));
    }

    #[test]
    fn base64_url_roundtrip() {
        let original = "Hello, Gmail!";
        let encoded = base64_url_encode(original.as_bytes());
        let decoded = base64_url_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }
}
