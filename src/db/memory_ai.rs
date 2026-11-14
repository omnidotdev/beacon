//! Memory AI over the Synapse gateway
//!
//! Semantic memory (embeddings), fact extraction (indexing), and retrieval
//! query condensing all run through the Synapse gateway, never a provider
//! directly. In cloud mode the client carries the user's product-scoped Beacon
//! key, so the gateway checks entitlements and charges the user's pooled Omni
//! credits under Beacon's Aether app exactly like chat (Posture A). Self-hosted
//! instances use the operator's own gateway client.
//!
//! Every failure degrades instead of blocking the turn: memory search falls
//! back to keyword search and indexing is skipped. A refused request (402 out
//! of credits, 403 no entitlement) is never retried

use std::sync::Arc;

use agent_core::knowledge::{CondenseError, QueryCondenser};
use async_trait::async_trait;
use synapse_client::{ChatRequest, EmbedInput, EmbedRequest, Message, SynapseClient};

use super::embedder::EMBEDDING_DIM;
use crate::billing::gateway_error::{self, GatewayFailure};

/// Gateway model for memory embeddings (provider-prefixed, as the gateway routes)
pub const EMBEDDING_MODEL: &str = "openai/text-embedding-3-small";

/// Small, cheap gateway model for fact extraction and query condensing
pub const MEMORY_CHAT_MODEL: &str = "anthropic/claude-haiku-5-5";

/// Output budget for fact extraction (a short JSON list of facts)
const EXTRACTION_MAX_TOKENS: u32 = 1024;

/// Output budget for a condensed retrieval query
const CONDENSE_MAX_TOKENS: u32 = 150;

/// System prompt that rewrites the latest message as a standalone query
const CONDENSE_SYSTEM_PROMPT: &str = "\
You are a search-query rewriter. Given a conversation history and the user's \
latest message, produce a single standalone search query that captures the \
user's information need. The query should:\n\
- Resolve all pronouns and references (\"that\", \"it\", \"the one\") to their concrete referents\n\
- Be concise (under 30 words)\n\
- Be suitable for semantic similarity search over a knowledge base\n\
- Contain only the rewritten query, no explanation\n\
\n\
If the latest message is already a standalone query, return it unchanged.";

/// Why a memory AI request failed
#[derive(Debug, thiserror::Error)]
pub enum MemoryAiError {
    /// The gateway refused the request (out of credits, no entitlement, or
    /// unavailable); the caller degrades and never retries
    #[error("gateway refused memory AI request: {0}")]
    Refused(String),

    /// Transport, parse, or provider failure
    #[error("memory AI request failed: {0}")]
    Failed(String),

    /// The response did not have the expected shape
    #[error("unexpected memory AI response: {0}")]
    Malformed(String),
}

impl MemoryAiError {
    /// Classify a Synapse client error
    fn from_client(error: &synapse_client::SynapseClientError) -> Self {
        let text = error.to_string();
        let refused = matches!(
            gateway_error::classify(&text),
            Some(GatewayFailure::OutOfCredits | GatewayFailure::Unavailable)
        ) || matches!(
            error,
            synapse_client::SynapseClientError::Api { status: 403, .. }
        );
        if refused {
            Self::Refused(text)
        } else {
            Self::Failed(text)
        }
    }
}

impl From<MemoryAiError> for crate::Error {
    fn from(e: MemoryAiError) -> Self {
        Self::Embedding(e.to_string())
    }
}

/// Memory AI bound to one gateway client (one user's key in cloud mode)
#[derive(Clone)]
pub struct MemoryAi {
    client: Arc<SynapseClient>,
}

impl std::fmt::Debug for MemoryAi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryAi")
            .field("base_url", &self.client.base_url().as_str())
            .finish_non_exhaustive()
    }
}

impl MemoryAi {
    /// Bind memory AI to a gateway client
    #[must_use]
    pub const fn new(client: Arc<SynapseClient>) -> Self {
        Self { client }
    }

    /// Embed one text
    ///
    /// # Errors
    ///
    /// Returns [`MemoryAiError`] when the gateway refuses or the call fails
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryAiError> {
        self.embed_batch(&[text])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| MemoryAiError::Malformed("empty embedding response".to_string()))
    }

    /// Embed several texts in one gateway request, in input order
    ///
    /// # Errors
    ///
    /// Returns [`MemoryAiError`] when the gateway refuses or the call fails
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, MemoryAiError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let request = EmbedRequest {
            input: EmbedInput::Multiple(texts.iter().map(|t| (*t).to_string()).collect()),
            model: EMBEDDING_MODEL.to_string(),
            dimensions: None,
        };
        let response = self
            .client
            .embed(&request)
            .await
            .map_err(|e| MemoryAiError::from_client(&e))?;

        let mut data = response.data;
        data.sort_by_key(|d| d.index);
        let embeddings: Vec<Vec<f32>> = data.into_iter().map(|d| d.embedding).collect();
        validate_embeddings(&embeddings, texts.len())?;
        Ok(embeddings)
    }

    /// One non-streaming completion on the memory model
    ///
    /// # Errors
    ///
    /// Returns [`MemoryAiError`] when the gateway refuses or the call fails
    pub async fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, MemoryAiError> {
        let request = ChatRequest {
            model: MEMORY_CHAT_MODEL.to_string(),
            messages: vec![Message::system(system), Message::user(user)],
            stream: false,
            // Current Claude models reject sampling parameters, so leave them unset
            temperature: None,
            top_p: None,
            max_tokens: Some(max_tokens),
            stop: None,
            tools: None,
            tool_choice: None,
        };
        let response = self
            .client
            .chat_completion(&request)
            .await
            .map_err(|e| MemoryAiError::from_client(&e))?;

        Ok(response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default())
    }

    /// Extract facts from a conversation as raw model output
    ///
    /// # Errors
    ///
    /// Returns [`MemoryAiError`] when the gateway refuses or the call fails
    pub async fn extract(&self, system: &str, user: &str) -> Result<String, MemoryAiError> {
        self.complete(system, user, EXTRACTION_MAX_TOKENS).await
    }
}

#[async_trait]
impl QueryCondenser for MemoryAi {
    async fn condense(&self, current: &str, history: &[&str]) -> Result<String, CondenseError> {
        // No history means no rewriting needed
        if history.is_empty() {
            return Ok(current.to_string());
        }

        let mut prompt = String::from("Conversation history:\n");
        for msg in history {
            prompt.push_str("- ");
            prompt.push_str(msg);
            prompt.push('\n');
        }
        prompt.push_str("\nLatest message: ");
        prompt.push_str(current);

        let condensed = self
            .complete(CONDENSE_SYSTEM_PROMPT, &prompt, CONDENSE_MAX_TOKENS)
            .await
            .map_err(|e| CondenseError::Request(e.to_string()))?;
        let condensed = condensed.trim();
        if condensed.is_empty() {
            Ok(current.to_string())
        } else {
            Ok(condensed.to_string())
        }
    }
}

/// Reject a response whose count or dimension would corrupt stored vectors
fn validate_embeddings(embeddings: &[Vec<f32>], expected: usize) -> Result<(), MemoryAiError> {
    if embeddings.len() != expected {
        return Err(MemoryAiError::Malformed(format!(
            "expected {expected} embeddings, got {}",
            embeddings.len()
        )));
    }
    if let Some(bad) = embeddings.iter().find(|e| e.len() != EMBEDDING_DIM) {
        return Err(MemoryAiError::Malformed(format!(
            "expected {EMBEDDING_DIM} dimensions, got {}",
            bad.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_provider_prefixed_gateway_ids() {
        assert_eq!(EMBEDDING_MODEL, "openai/text-embedding-3-small");
        assert!(MEMORY_CHAT_MODEL.starts_with("anthropic/"));
    }

    #[test]
    fn refusals_are_not_retryable_failures() {
        let out_of_credits = synapse_client::SynapseClientError::Api {
            status: 402,
            error_type: "insufficient_credits_error".to_string(),
            message: "balance empty".to_string(),
        };
        assert!(matches!(
            MemoryAiError::from_client(&out_of_credits),
            MemoryAiError::Refused(_)
        ));

        let forbidden = synapse_client::SynapseClientError::Api {
            status: 403,
            error_type: "forbidden".to_string(),
            message: "embeddings not enabled".to_string(),
        };
        assert!(matches!(
            MemoryAiError::from_client(&forbidden),
            MemoryAiError::Refused(_)
        ));

        let unavailable = synapse_client::SynapseClientError::Api {
            status: 503,
            error_type: "billing_unavailable_error".to_string(),
            message: "try later".to_string(),
        };
        assert!(matches!(
            MemoryAiError::from_client(&unavailable),
            MemoryAiError::Refused(_)
        ));

        let parse = synapse_client::SynapseClientError::Parse("bad json".to_string());
        assert!(matches!(
            MemoryAiError::from_client(&parse),
            MemoryAiError::Failed(_)
        ));
    }

    #[test]
    fn embedding_shape_is_validated() {
        let good = vec![vec![0.0; EMBEDDING_DIM]; 2];
        assert!(validate_embeddings(&good, 2).is_ok());
        assert!(validate_embeddings(&good, 3).is_err());
        assert!(validate_embeddings(&[vec![0.0; 8]], 1).is_err());
    }

    #[tokio::test]
    async fn condense_without_history_skips_the_gateway() {
        // An unroutable base URL would fail if a request were made
        let client = SynapseClient::new("http://127.0.0.1:9").unwrap();
        let ai = MemoryAi::new(Arc::new(client));
        assert_eq!(ai.condense("hello", &[]).await.unwrap(), "hello");
    }
}
