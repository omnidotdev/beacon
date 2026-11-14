//! Vision client for image analysis
//!
//! Routes image analysis through a Synapse gateway for provider-agnostic
//! routing, failover, and metering. There is no direct provider path: the
//! gateway client decides whose key (and credits) the request runs on

use std::sync::Arc;

use base64::Engine;
use synapse_client::SynapseClient;

use crate::{Error, Result};

/// Default vision model, in the explicit `provider/model` form Synapse resolves
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-5-5";
/// Output cap for a description; leaves headroom for the model's default thinking
const MAX_VISION_TOKENS: u32 = 1024;
const VISION_PROMPT: &str = "Describe this image concisely in 1-2 sentences. Focus on the main subject and any text visible.";

/// Vision client for image analysis
pub struct VisionClient {
    /// Gateway client the request runs on
    synapse: Arc<SynapseClient>,
    /// Model identifier
    model: String,
}

impl VisionClient {
    /// Create a new vision client backed by Synapse
    #[must_use]
    pub fn from_synapse(synapse: Arc<SynapseClient>) -> Self {
        Self {
            synapse,
            model: DEFAULT_MODEL.to_string(),
        }
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
            return Err(Error::Vision("Empty response from vision API".to_string()));
        }

        tracing::debug!(description = %description, "image described via Synapse");
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
