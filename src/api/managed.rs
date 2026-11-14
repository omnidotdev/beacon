//! Per-user managed Synapse clients (cloud mode)
//!
//! Every managed request runs on the user's product-scoped key, so the
//! Synapse gateway checks and charges credits against Beacon's Aether app and
//! the user's pooled Omni balance. There is no shared-key fallback: when a key
//! cannot be provisioned the AI feature is unavailable

use std::sync::Arc;

use synapse_client::SynapseClient;

use super::ApiState;
use crate::billing::gateway_error::{
    self, CODE_AI_UNAVAILABLE, GatewayFailure, UNAVAILABLE_MESSAGE,
};
use crate::db::MemoryAi;
use crate::providers::provisioner::{BillingEntity, ProvisionError};

/// Error code when the account lacks the `api_access` entitlement
pub const CODE_ACCESS_DENIED: &str = "access_denied";

/// A user's managed client plus the plan-based default model
pub struct ManagedClient {
    /// Synapse client authenticated with the product-scoped key
    pub client: Arc<SynapseClient>,
    /// Default model for the user's plan tier
    pub default_model: String,
}

/// Why a managed client could not be built
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedError {
    /// The account lacks managed AI access
    Denied,
    /// Provisioning failed or is not configured
    Unavailable,
}

impl ManagedError {
    /// Stable error code sent to clients
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Denied => CODE_ACCESS_DENIED,
            Self::Unavailable => CODE_AI_UNAVAILABLE,
        }
    }

    /// User-facing message
    #[must_use]
    pub fn user_message(self) -> String {
        match self {
            Self::Denied => format!(
                "Your plan doesn't include Beacon AI access. Upgrade to keep going: {}",
                gateway_error::billing_url()
            ),
            Self::Unavailable => UNAVAILABLE_MESSAGE.to_string(),
        }
    }
}

/// The entity Beacon bills for a user's managed AI.
///
/// Beacon is a personal assistant: conversations, memory, and personas are
/// keyed by the user, so the user's own account pays
#[must_use]
pub fn billing_entity_for(user_id: &str) -> BillingEntity {
    BillingEntity::user(user_id)
}

/// Base URL of the Synapse gateway managed requests go to
fn synapse_base_url(state: &ApiState) -> String {
    state.synapse.as_ref().map_or_else(
        || "http://localhost:6000".to_string(),
        |c| c.base_url().to_string(),
    )
}

/// Build a gateway client on the user's product-scoped Beacon key
///
/// # Errors
///
/// Returns [`ManagedError`] when no product-scoped key can be provisioned;
/// callers must surface it rather than fall back to another client
pub async fn managed_synapse_client(
    state: &ApiState,
    user_id: &str,
) -> Result<Arc<SynapseClient>, ManagedError> {
    let Some(provisioner) = &state.key_provisioner else {
        tracing::warn!(user_id = %user_id, "managed AI requested but no key provisioner configured");
        return Err(ManagedError::Unavailable);
    };

    let entity = billing_entity_for(user_id);
    let key = provisioner
        .managed_key(user_id, &entity)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, user_id = %user_id, "managed key provisioning failed");
            match e {
                ProvisionError::Denied(_) => ManagedError::Denied,
                ProvisionError::Unavailable(_) => ManagedError::Unavailable,
            }
        })?;

    let base_url = synapse_base_url(state);
    let client = SynapseClient::new(&base_url)
        .map_err(|e| {
            tracing::error!(error = %e, url = %base_url, "failed to create managed SynapseClient");
            ManagedError::Unavailable
        })?
        .with_api_key(key.to_string());

    Ok(Arc::new(client))
}

/// Build the user's managed Synapse client
///
/// # Errors
///
/// Returns [`ManagedError`] when no product-scoped key can be provisioned;
/// callers must surface it rather than fall back to another client
pub async fn managed_client(
    state: &ApiState,
    user_id: &str,
) -> Result<ManagedClient, ManagedError> {
    let client = managed_synapse_client(state, user_id).await?;

    let default_model = crate::billing::default_model_for_user(
        state.billing_state.as_ref(),
        user_id,
        &state.llm_model,
    )
    .await;

    Ok(ManagedClient {
        client,
        default_model,
    })
}

/// Memory AI (embeddings, fact extraction, query condensing) for a turn
///
/// Cloud mode always uses the user's product-scoped Beacon key, so memory AI
/// is charged to the user's Omni credits like chat; without an authenticated
/// user there is no billable key and memory falls back to keyword search.
/// Self-hosted instances use the turn's own gateway client (the operator's
/// gateway and keys). `None` means memory AI is off for this turn
pub async fn memory_ai_for(
    state: &ApiState,
    gatekeeper_user_id: Option<&str>,
    turn_client: Option<&Arc<SynapseClient>>,
) -> Option<MemoryAi> {
    if state.cloud_mode {
        let user_id = gatekeeper_user_id?;
        return managed_synapse_client(state, user_id)
            .await
            .map_err(|e| {
                tracing::debug!(user_id = %user_id, error = ?e, "memory AI unavailable for this turn");
            })
            .ok()
            .map(MemoryAi::new);
    }

    turn_client
        .cloned()
        .or_else(|| state.synapse.clone())
        .map(MemoryAi::new)
}

/// React to a gateway refusal on a managed request: a rejected key is
/// dropped so the next request provisions a fresh one
pub fn note_gateway_failure(state: &ApiState, user_id: &str, failure: GatewayFailure) {
    if failure == GatewayFailure::Unauthorized
        && let Some(provisioner) = &state.key_provisioner
    {
        provisioner.invalidate(user_id, &billing_entity_for(user_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::provisioner::BillingEntityType;

    #[test]
    fn beacon_bills_the_user() {
        let entity = billing_entity_for("u-1");
        assert_eq!(entity.entity_type, BillingEntityType::User);
        assert_eq!(entity.entity_id, "u-1");
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(ManagedError::Denied.code(), "access_denied");
        assert_eq!(ManagedError::Unavailable.code(), "ai_unavailable");
        assert!(ManagedError::Denied.user_message().contains("Upgrade"));
        assert_eq!(
            ManagedError::Unavailable.user_message(),
            UNAVAILABLE_MESSAGE
        );
    }
}
