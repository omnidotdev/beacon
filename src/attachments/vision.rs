//! Vision client for image analysis
//!
//! Routes image analysis through Synapse for provider-agnostic multi-model routing,
//! failover, and circuit-breaker support. Falls back to direct Anthropic API if
//! no Synapse client is available.

use std::sync::Arc;

use base64::Engine;
use synapse_client::SynapseClient;

use crate::{Error, Result};

const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const MAX_VISION_TOKENS: u32 = 300;
const VISION_PROMPT: &str = "Describe this image concisely in 1-2 sentences. Focus on the main subject and any text visible.";

/// Vision client for image analysis
pub struct VisionClient {
    /// Synapse client for provider-agnostic routing
    synapse: Option<Arc<SynapseClient>>,
    /// Model identifier
    model: String,
}

impl VisionClient {
    /// Create a new vision client backed by Synapse
    #[must_use]
    pub fn from_synapse(synapse: Arc<SynapseClient>) -> Self {
        Self {
            synapse: Some(synapse),
            model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Create a vision client using a direct Anthropic API key (legacy fallback)
    ///
    /// # Errors
    ///
    /// Returns error if API key is empty
    pub fn new(api_key: &str) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "Anthropic API key required for vision".to_string(),
            ));
        }

        // Build a standalone Synapse client pointed at Anthropic
        // This allows us to use the same code path for both Synapse-backed and direct API
        tracing::debug!("vision client using direct Anthropic API (no Synapse routing)");
        Ok(Self {
            synapse: None,
            model: DEFAULT_MODEL.to_string(),
        })
    }

    /// Create with a specific model
    #[must_use]
    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Describe an image via Synapse chat completion with vision content
    ///
    /// Sends the image as a base64 data URL in `OpenAI` multipart content format,
    /// routed through Synapse for provider-agnostic model selection.
    ///
    /// # Errors
    ///
    /// Returns error if the API call fails or returns no content
    pub async fn describe_image(&self, image_data: &[u8], mime_type: &str) -> Result<String> {
        let Some(ref synapse) = self.synapse else {
            return self.describe_image_direct(image_data, mime_type).await;
        };

        let base64_data = base64::engine::general_purpose::STANDARD.encode(image_data);
        let media_type = normalize_mime_type(mime_type);
        let data_url = format!("data:{media_type};base64,{base64_data}");

        // Build OpenAI-format multipart content with image_url
        let content = serde_json::json!([
            {
                "type": "image_url",
                "image_url": { "url": data_url }
            },
            {
                "type": "text",
                "text": VISION_PROMPT
            }
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
            max_tokens: Some(MAX_VISION_TOKENS),
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let response = synapse
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
            return Err(Error::Vision("Empty response from vision API".to_string()));
        }

        tracing::debug!(description = %description, "image described via Synapse");
        Ok(description)
    }

    /// Direct Anthropic API fallback when no Synapse client is configured
    async fn describe_image_direct(&self, image_data: &[u8], mime_type: &str) -> Result<String> {
        use serde::{Deserialize, Serialize};

        const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

        #[derive(Serialize)]
        struct MessageRequest<'a> {
            model: &'a str,
            max_tokens: u32,
            messages: Vec<DirectMessage<'a>>,
        }

        #[derive(Serialize)]
        struct DirectMessage<'a> {
            role: &'a str,
            content: Vec<DirectContentBlock<'a>>,
        }

        #[derive(Serialize)]
        #[serde(tag = "type")]
        enum DirectContentBlock<'a> {
            #[serde(rename = "text")]
            Text { text: &'a str },
            #[serde(rename = "image")]
            Image { source: ImageSource<'a> },
        }

        #[derive(Serialize)]
        struct ImageSource<'a> {
            #[serde(rename = "type")]
            source_type: &'a str,
            media_type: &'a str,
            data: String,
        }

        #[derive(Deserialize)]
        struct MessageResponse {
            content: Vec<ResponseContent>,
        }

        #[derive(Deserialize)]
        struct ResponseContent {
            #[allow(dead_code)]
            #[serde(rename = "type")]
            content_type: String,
            text: Option<String>,
        }

        // Requires ANTHROPIC_API_KEY in environment
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
            Error::Vision("ANTHROPIC_API_KEY required for direct vision API".to_string())
        })?;

        let base64_data = base64::engine::general_purpose::STANDARD.encode(image_data);
        let media_type = normalize_mime_type(mime_type);

        let request = MessageRequest {
            model: &self.model,
            max_tokens: MAX_VISION_TOKENS,
            messages: vec![DirectMessage {
                role: "user",
                content: vec![
                    DirectContentBlock::Image {
                        source: ImageSource {
                            source_type: "base64",
                            media_type,
                            data: base64_data,
                        },
                    },
                    DirectContentBlock::Text {
                        text: VISION_PROMPT,
                    },
                ],
            }],
        };

        let client = reqwest::Client::new();
        let response = client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Vision(format!("Request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Vision(format!("API error {status}: {body}")));
        }

        let result: MessageResponse = response
            .json()
            .await
            .map_err(|e| Error::Vision(format!("Parse error: {e}")))?;

        let description = result
            .content
            .into_iter()
            .filter_map(|c| c.text)
            .collect::<Vec<_>>()
            .join(" ");

        if description.is_empty() {
            return Err(Error::Vision("Empty response from vision API".to_string()));
        }

        tracing::debug!(description = %description, "image described via direct API");
        Ok(description)
    }
}

/// Normalize MIME type for vision APIs
fn normalize_mime_type(mime_type: &str) -> &'static str {
    match mime_type.to_lowercase().as_str() {
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        // jpeg, jpg, and any unknown type default to jpeg
        _ => "image/jpeg",
    }
}
