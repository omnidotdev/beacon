//! STT provider trait and fallback router
//!
//! Abstracts speech-to-text behind a provider trait so multiple backends
//! (`Synapse`/Whisper, `OpenAI` Whisper direct, Deepgram) can be configured with
//! ordered fallback

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use synapse_client::SynapseClient;

use crate::{Error, Result};

/// Trait for speech-to-text providers
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Provider name for logging
    fn name(&self) -> &str;

    /// Transcribe audio data to text
    ///
    /// # Errors
    ///
    /// Returns error if the provider fails to transcribe the audio
    async fn transcribe(&self, audio_data: &[u8], mime_type: &str) -> Result<String>;
}

/// Router that tries STT providers in order with fallback
pub struct SttRouter {
    providers: Vec<Box<dyn SttProvider>>,
}

impl SttRouter {
    /// Create a new STT router with the given providers (tried in order)
    #[must_use]
    pub fn new(providers: Vec<Box<dyn SttProvider>>) -> Self {
        Self { providers }
    }

    /// Transcribe audio, trying each provider in order until one succeeds
    ///
    /// # Errors
    ///
    /// Returns error if all providers fail or none are configured
    pub async fn transcribe(&self, data: &[u8], mime: &str) -> Result<String> {
        let mut last_err = None;
        for provider in &self.providers {
            match provider.transcribe(data, mime).await {
                Ok(text) => return Ok(text),
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        error = %e,
                        "STT provider failed, trying next"
                    );
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Media("no STT providers configured".into())))
    }
}

// ---------------------------------------------------------------------------
// Concrete providers
// ---------------------------------------------------------------------------

/// Synapse-backed STT provider (provider-agnostic routing)
pub struct SynapseStt {
    synapse: Arc<SynapseClient>,
    model: String,
}

impl SynapseStt {
    /// Create a `Synapse` STT provider
    #[must_use]
    pub const fn new(synapse: Arc<SynapseClient>, model: String) -> Self {
        Self { synapse, model }
    }
}

#[async_trait]
impl SttProvider for SynapseStt {
    fn name(&self) -> &'static str {
        "synapse"
    }

    async fn transcribe(&self, audio_data: &[u8], mime_type: &str) -> Result<String> {
        let ext = extension_for_mime(mime_type);
        let filename = format!("audio.{ext}");

        let result = self
            .synapse
            .transcribe(audio_data.to_vec().into(), &filename, &self.model)
            .await
            .map_err(|e| Error::Stt(format!("Synapse transcription failed: {e}")))?;

        Ok(result.text)
    }
}

/// `OpenAI` Whisper STT provider (direct API)
pub struct WhisperStt {
    client: Client,
    api_key: String,
    model: String,
    language: Option<String>,
}

impl WhisperStt {
    /// Create a Whisper STT provider
    ///
    /// # Errors
    ///
    /// Returns error if `api_key` is empty
    pub fn new(api_key: String, model: String, language: Option<String>) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "OPENAI_API_KEY required for Whisper STT".into(),
            ));
        }
        Ok(Self {
            client: Client::new(),
            api_key,
            model,
            language,
        })
    }
}

#[async_trait]
impl SttProvider for WhisperStt {
    fn name(&self) -> &'static str {
        "whisper"
    }

    async fn transcribe(&self, audio_data: &[u8], mime_type: &str) -> Result<String> {
        let ext = extension_for_mime(mime_type);
        let filename = format!("audio.{ext}");

        let part = Part::bytes(audio_data.to_vec())
            .file_name(filename)
            .mime_str(mime_type)
            .map_err(|e| Error::Stt(format!("invalid MIME type: {e}")))?;

        let mut form = Form::new()
            .text("model", self.model.clone())
            .part("file", part);

        if let Some(ref lang) = self.language {
            form = form.text("language", lang.clone());
        }

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
            .map_err(|e| Error::Stt(format!("Whisper request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Stt(format!("Whisper API error: {status} - {body}")));
        }

        let result: TranscriptionResponse = response
            .json()
            .await
            .map_err(|e| Error::Stt(format!("failed to parse Whisper response: {e}")))?;

        Ok(result.text)
    }
}

/// Deepgram STT provider
pub struct DeepgramStt {
    client: Client,
    api_key: String,
}

impl DeepgramStt {
    /// Create a Deepgram STT provider
    ///
    /// # Errors
    ///
    /// Returns error if `api_key` is empty
    pub fn new(api_key: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "DEEPGRAM_API_KEY required for Deepgram STT".into(),
            ));
        }
        Ok(Self {
            client: Client::new(),
            api_key,
        })
    }
}

#[async_trait]
impl SttProvider for DeepgramStt {
    fn name(&self) -> &'static str {
        "deepgram"
    }

    async fn transcribe(&self, audio_data: &[u8], mime_type: &str) -> Result<String> {
        let response = self
            .client
            .post("https://api.deepgram.com/v1/listen")
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Content-Type", mime_type)
            .body(audio_data.to_vec())
            .send()
            .await
            .map_err(|e| Error::Stt(format!("Deepgram request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Stt(format!("Deepgram API error: {status} - {body}")));
        }

        let result: DeepgramResponse = response
            .json()
            .await
            .map_err(|e| Error::Stt(format!("failed to parse Deepgram response: {e}")))?;

        result
            .results
            .and_then(|r| r.channels)
            .and_then(|c| c.into_iter().next())
            .and_then(|c| c.alternatives)
            .and_then(|a| a.into_iter().next())
            .map(|a| a.transcript)
            .ok_or_else(|| Error::Stt("empty response from Deepgram".to_string()))
    }
}

// ---------------------------------------------------------------------------
// Shared helpers and API types
// ---------------------------------------------------------------------------

/// Get file extension for an audio MIME type
fn extension_for_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/mp4" | "audio/m4a" => "m4a",
        "audio/wav" | "audio/wave" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/flac" => "flac",
        _ => "mp3",
    }
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

// Deepgram response types
#[derive(Deserialize)]
struct DeepgramResponse {
    results: Option<DeepgramResults>,
}

#[derive(Deserialize)]
struct DeepgramResults {
    channels: Option<Vec<DeepgramChannel>>,
}

#[derive(Deserialize)]
struct DeepgramChannel {
    alternatives: Option<Vec<DeepgramAlternative>>,
}

#[derive(Deserialize)]
struct DeepgramAlternative {
    transcript: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct MockStt {
        should_fail: bool,
    }

    #[async_trait]
    impl SttProvider for MockStt {
        fn name(&self) -> &str {
            "mock"
        }

        async fn transcribe(&self, _: &[u8], _: &str) -> Result<String> {
            if self.should_fail {
                Err(Error::Media("mock failure".into()))
            } else {
                Ok("hello world".into())
            }
        }
    }

    #[tokio::test]
    async fn stt_router_tries_fallback() {
        let router = SttRouter::new(vec![
            Box::new(MockStt { should_fail: true }),
            Box::new(MockStt { should_fail: false }),
        ]);

        let result = router.transcribe(b"fake", "audio/wav").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello world");
    }

    #[tokio::test]
    async fn stt_router_all_fail() {
        let router = SttRouter::new(vec![
            Box::new(MockStt { should_fail: true }),
            Box::new(MockStt { should_fail: true }),
        ]);

        let result = router.transcribe(b"fake", "audio/wav").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn stt_router_no_providers() {
        let router = SttRouter::new(vec![]);
        let result = router.transcribe(b"fake", "audio/wav").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no STT providers configured")
        );
    }

    #[test]
    fn deepgram_stt_provider_name() {
        let provider = DeepgramStt::new("test-key".into()).unwrap();
        assert_eq!(provider.name(), "deepgram");
    }

    #[test]
    fn whisper_stt_provider_name() {
        let provider = WhisperStt::new("test-key".into(), "whisper-1".into(), None).unwrap();
        assert_eq!(provider.name(), "whisper");
    }
}
