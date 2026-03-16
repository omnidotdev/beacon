//! IRC channel adapter
//!
//! Connects to IRC servers using the `irc` crate for real-time messaging

use async_trait::async_trait;
use irc::client::prelude::*;
use tokio::sync::mpsc;

use super::{Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// IRC connection configuration
#[derive(Debug, Clone)]
pub struct IrcConfig {
    /// IRC server hostname
    pub server: String,
    /// IRC server port
    pub port: u16,
    /// Bot nickname
    pub nickname: String,
    /// Channels to join (e.g. `["#general", "#dev"]`)
    pub channels: Vec<String>,
    /// Use TLS for the connection
    pub use_tls: bool,
    /// Optional server password
    pub password: Option<String>,
}

/// IRC channel adapter
pub struct IrcChannel {
    config: IrcConfig,
    client: Option<irc::client::Client>,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: bool,
}

impl IrcChannel {
    /// Create a new IRC channel adapter
    #[must_use]
    pub const fn new(config: IrcConfig) -> Self {
        Self {
            config,
            client: None,
            message_tx: None,
            connected: false,
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(config: IrcConfig) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            config,
            client: None,
            message_tx: Some(tx),
            connected: false,
        };
        (channel, rx)
    }
}

#[async_trait]
impl Channel for IrcChannel {
    fn name(&self) -> &'static str {
        "irc"
    }

    fn capabilities(&self) -> &'static [super::ChannelCapability] {
        &[]
    }

    async fn connect(&mut self) -> Result<()> {
        let irc_config = Config {
            nickname: Some(self.config.nickname.clone()),
            server: Some(self.config.server.clone()),
            port: Some(self.config.port),
            channels: self.config.channels.clone(),
            use_tls: Some(self.config.use_tls),
            password: self.config.password.clone(),
            ..Config::default()
        };

        let mut client = irc::client::Client::from_config(irc_config)
            .await
            .map_err(|e| Error::Channel(format!("IRC client creation failed: {e}")))?;

        client
            .identify()
            .map_err(|e| Error::Channel(format!("IRC identify failed: {e}")))?;

        // Spawn stream listener for incoming messages
        if let Some(tx) = &self.message_tx {
            let tx = tx.clone();
            let mut stream = client
                .stream()
                .map_err(|e| Error::Channel(format!("IRC stream creation failed: {e}")))?;

            tokio::spawn(async move {
                use futures::StreamExt as _;

                while let Some(message) = stream.next().await {
                    let Ok(message) = message else {
                        tracing::warn!("IRC stream error");
                        continue;
                    };

                    // Only process PRIVMSG commands
                    if let Command::PRIVMSG(ref target, ref text) = message.command {
                        let sender_nick =
                            message.source_nickname().unwrap_or("unknown").to_string();

                        let is_dm = !target.starts_with('#');

                        let incoming = IncomingMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            channel_id: target.clone(),
                            sender_id: sender_nick.clone(),
                            sender_name: sender_nick,
                            content: text.clone(),
                            is_dm,
                            reply_to: None,
                            attachments: Vec::new(),
                            thread_id: None,
                            callback_data: None,
                        };

                        if let Err(e) = tx.send(incoming).await {
                            tracing::warn!(error = %e, "failed to forward IRC message");
                            break;
                        }
                    }
                }

                tracing::info!("IRC stream listener ended");
            });
        }

        self.client = Some(client);
        self.connected = true;
        tracing::info!(
            server = %self.config.server,
            nickname = %self.config.nickname,
            "IRC channel connected"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(client) = &self.client {
            client
                .send_quit("Beacon gateway shutting down")
                .map_err(|e| Error::Channel(format!("IRC quit failed: {e}")))?;
        }
        self.connected = false;
        self.client = None;
        tracing::info!("IRC channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| Error::Channel("IRC client not connected".to_string()))?;

        client
            .send_privmsg(&message.channel_id, &message.content)
            .map_err(|e| Error::Channel(format!("IRC send failed: {e}")))?;

        tracing::debug!(target = %message.channel_id, "IRC message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irc_channel_name() {
        let config = IrcConfig {
            server: "irc.example.com".to_string(),
            port: 6697,
            nickname: "beacon-bot".to_string(),
            channels: vec!["#test".to_string()],
            use_tls: true,
            password: None,
        };

        let channel = IrcChannel::new(config);
        assert_eq!(channel.name(), "irc");
        assert!(!channel.is_connected());
    }
}
