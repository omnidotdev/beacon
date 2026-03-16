//! Vision provider trait and fallback router
//!
//! Abstracts image understanding behind a provider trait so multiple backends
//! (`Synapse`, `OpenAI`, Gemini, Anthropic) can be configured with ordered fallback

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use synapse_client::SynapseClient;

use crate::{Error, Result};

const DEFAULT_VISION_PROMPT: &str = "Describe this image concisely in 1-2 sentences. Focus on the main subject and any text visible.";

/// Trait for vision (image understanding) providers
#[async_trait]
pub trait VisionProvider: Send + Sync {
    /// Provider name for logging
    fn name(&self) -> &str;

    /// Describe an image, returning a text description
    ///
    /// # Errors
    ///
    /// Returns error if the provider fails to process the image
    async fn describe_image(
        &self,
        image_data: &[u8],
        mime_type: &str,
        prompt: Option<&str>,
    ) -> Result<String>;
}

/// Router that tries vision providers in order with fallback
pub struct VisionRouter {
    providers: Vec<Box<dyn VisionProvider>>,
}

impl VisionRouter {
    /// Create a new vision router with the given providers (tried in order)
    #[must_use]
    pub fn new(providers: Vec<Box<dyn VisionProvider>>) -> Self {
        Self { providers }
    }

    /// Describe an image, trying each provider in order until one succeeds
    ///
    /// # Errors
    ///
    /// Returns error if all providers fail or none are configured
    pub async fn describe_image(
        &self,
        data: &[u8],
        mime: &str,
        prompt: Option<&str>,
    ) -> Result<String> {
        let mut last_err = None;
        for provider in &self.providers {
            match provider.describe_image(data, mime, prompt).await {
                Ok(desc) => return Ok(desc),
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        error = %e,
                        "vision provider failed, trying next"
                    );
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Media("no vision providers configured".into())))
    }
}

// ---------------------------------------------------------------------------
// Concrete providers
// ---------------------------------------------------------------------------

/// Synapse-backed vision provider (provider-agnostic routing)
pub struct SynapseVision {
    synapse: Arc<SynapseClient>,
    model: String,
    max_tokens: u32,
}

impl SynapseVision {
    /// Create a `Synapse` vision provider
    #[must_use]
    pub const fn new(synapse: Arc<SynapseClient>, model: String, max_tokens: u32) -> Self {
        Self {
            synapse,
            model,
            max_tokens,
        }
    }
}

#[async_trait]
impl VisionProvider for SynapseVision {
    fn name(&self) -> &'static str {
        "synapse"
    }

    async fn describe_image(
        &self,
        image_data: &[u8],
        mime_type: &str,
        prompt: Option<&str>,
    ) -> Result<String> {
        let base64_data = base64::engine::general_purpose::STANDARD.encode(image_data);
        let media_type = normalize_mime_type(mime_type);
        let data_url = format!("data:{media_type};base64,{base64_data}");
        let text = prompt.unwrap_or(DEFAULT_VISION_PROMPT);

        let content = serde_json::json!([
            { "type": "image_url", "image_url": { "url": data_url } },
            { "type": "text", "text": text }
        ]);

        let message = synapse_client::Message {
            role: "user".to_owned(),
            content,
            tool_calls: None,
            tool_call_id: None,
        };

        let request = synapse_client::ChatRequest {
            model: self.model.clone(),
            messages: vec![message],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: Some(self.max_tokens),
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let response = self
            .synapse
            .chat_completion(&request)
            .await
            .map_err(|e| Error::Vision(format!("Synapse vision request failed: {e}")))?;

        let description = response
            .choices
            .into_iter()
            .filter_map(|c| c.message.content)
            .collect::<Vec<_>>()
            .join(" ");

        if description.is_empty() {
            return Err(Error::Vision(
                "empty response from Synapse vision".to_string(),
            ));
        }

        tracing::debug!(description = %description, "image described via Synapse");
        Ok(description)
    }
}

/// `OpenAI` Vision provider (direct API)
pub struct OpenAiVision {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl OpenAiVision {
    /// Create an `OpenAI` vision provider
    ///
    /// # Errors
    ///
    /// Returns error if `api_key` is empty
    pub fn new(api_key: String, model: String, max_tokens: u32) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "OPENAI_API_KEY required for OpenAI vision".into(),
            ));
        }
        Ok(Self {
            client: Client::new(),
            api_key,
            model,
            max_tokens,
        })
    }
}

#[async_trait]
impl VisionProvider for OpenAiVision {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn describe_image(
        &self,
        image_data: &[u8],
        mime_type: &str,
        prompt: Option<&str>,
    ) -> Result<String> {
        let base64_data = base64::engine::general_purpose::STANDARD.encode(image_data);
        let data_url = format!("data:{mime_type};base64,{base64_data}");

        let request = OpenAiChatRequest {
            model: self.model.clone(),
            messages: vec![OpenAiMessage {
                role: "user".to_string(),
                content: vec![
                    OpenAiContent::Text {
                        text: prompt.unwrap_or(DEFAULT_VISION_PROMPT).to_string(),
                    },
                    OpenAiContent::ImageUrl {
                        image_url: OpenAiImageUrl { url: data_url },
                    },
                ],
            }],
            max_tokens: Some(self.max_tokens),
        };

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Vision(format!("OpenAI vision request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Vision(format!(
                "OpenAI API error: {status} - {body}"
            )));
        }

        let result: OpenAiChatResponse = response
            .json()
            .await
            .map_err(|e| Error::Vision(format!("Failed to parse OpenAI response: {e}")))?;

        result
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .ok_or_else(|| Error::Vision("empty response from OpenAI vision".to_string()))
    }
}

/// Gemini Vision provider (direct Google API)
pub struct GeminiVision {
    client: Client,
    api_key: String,
    model: String,
}

impl GeminiVision {
    /// Create a Gemini vision provider
    ///
    /// # Errors
    ///
    /// Returns error if `api_key` is empty
    pub fn new(api_key: String, model: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "GEMINI_API_KEY required for Gemini vision".into(),
            ));
        }
        Ok(Self {
            client: Client::new(),
            api_key,
            model,
        })
    }
}

#[async_trait]
impl VisionProvider for GeminiVision {
    fn name(&self) -> &'static str {
        "gemini"
    }

    async fn describe_image(
        &self,
        image_data: &[u8],
        mime_type: &str,
        prompt: Option<&str>,
    ) -> Result<String> {
        let base64_data = base64::engine::general_purpose::STANDARD.encode(image_data);
        let text = prompt.unwrap_or(DEFAULT_VISION_PROMPT);

        let body = serde_json::json!({
            "contents": [{
                "parts": [
                    { "text": text },
                    {
                        "inline_data": {
                            "mime_type": mime_type,
                            "data": base64_data
                        }
                    }
                ]
            }]
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Vision(format!("Gemini vision request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Vision(format!(
                "Gemini API error: {status} - {body}"
            )));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Vision(format!("Failed to parse Gemini response: {e}")))?;

        result
            .candidates
            .and_then(|c| c.into_iter().next())
            .and_then(|c| c.content)
            .and_then(|c| c.parts)
            .and_then(|p| p.into_iter().next())
            .and_then(|p| p.text)
            .ok_or_else(|| Error::Vision("empty response from Gemini vision".to_string()))
    }
}

// ---------------------------------------------------------------------------
// Shared helpers and API types
// ---------------------------------------------------------------------------

/// Normalize MIME type for vision APIs
fn normalize_mime_type(mime_type: &str) -> &'static str {
    match mime_type.to_lowercase().as_str() {
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => "image/jpeg",
    }
}

// OpenAI types
#[derive(Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: Vec<OpenAiContent>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum OpenAiContent {
    Text { text: String },
    ImageUrl { image_url: OpenAiImageUrl },
}

#[derive(Serialize)]
struct OpenAiImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
}

// Gemini types
#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
}

#[derive(Deserialize)]
struct GeminiContent {
    parts: Option<Vec<GeminiPart>>,
}

#[derive(Deserialize)]
struct GeminiPart {
    text: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct MockVision {
        should_fail: bool,
    }

    #[async_trait]
    impl VisionProvider for MockVision {
        fn name(&self) -> &str {
            "mock"
        }

        async fn describe_image(&self, _: &[u8], _: &str, _: Option<&str>) -> Result<String> {
            if self.should_fail {
                Err(Error::Media("mock failure".into()))
            } else {
                Ok("a test image".into())
            }
        }
    }

    #[tokio::test]
    async fn vision_router_tries_fallback() {
        let router = VisionRouter::new(vec![
            Box::new(MockVision { should_fail: true }),
            Box::new(MockVision { should_fail: false }),
        ]);

        let result = router.describe_image(b"fake", "image/png", None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "a test image");
    }

    #[tokio::test]
    async fn vision_router_all_fail() {
        let router = VisionRouter::new(vec![
            Box::new(MockVision { should_fail: true }),
            Box::new(MockVision { should_fail: true }),
        ]);

        let result = router.describe_image(b"fake", "image/png", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn vision_router_no_providers() {
        let router = VisionRouter::new(vec![]);
        let result = router.describe_image(b"fake", "image/png", None).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no vision providers configured")
        );
    }

    #[test]
    fn openai_vision_provider_name() {
        let provider = OpenAiVision::new("test-key".into(), "gpt-4o".into(), 300).unwrap();
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn gemini_vision_provider_name() {
        let provider = GeminiVision::new("test-key".into(), "gemini-2.5-flash".into()).unwrap();
        assert_eq!(provider.name(), "gemini");
    }
}
