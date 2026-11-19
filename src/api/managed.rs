//! Per-user managed Synapse clients (cloud mode)
//!
//! Every managed request runs on the user's product-scoped key, so the
//! Synapse gateway checks and charges credits against Beacon's Aether app and
//! the pooled AI credit balance of the user's personal workspace. There is no
//! shared-key fallback and no user-entity fallback: when a key cannot be
//! provisioned, or the workspace to bill is unknown, the AI feature is
//! unavailable

use std::sync::Arc;

use synapse_client::SynapseClient;

use super::ApiState;
use crate::billing::WorkspaceError;
use crate::billing::gateway_error::{
    self, CODE_AI_UNAVAILABLE, GatewayFailure, UNAVAILABLE_MESSAGE,
};
use crate::db::MemoryAi;
use crate::providers::provisioner::{BillingEntity, ProvisionError};

/// Error code when the account lacks the `api_access` entitlement
pub const CODE_ACCESS_DENIED: &str = "access_denied";

/// Error code when the user must sign in again so their personal workspace
/// (the billing scope) is known
pub const CODE_REAUTH_REQUIRED: &str = "reauth_required";

/// User-facing copy when the user must sign in again
pub const REAUTH_MESSAGE: &str = "Sign in to Beacon again to keep using AI.";

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
    /// The personal workspace to bill is unknown: the user must sign in again
    ReauthRequired,
    /// Provisioning failed or is not configured
    Unavailable,
}

impl ManagedError {
    /// Stable error code sent to clients
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Denied => CODE_ACCESS_DENIED,
            Self::ReauthRequired => CODE_REAUTH_REQUIRED,
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
            Self::ReauthRequired => REAUTH_MESSAGE.to_string(),
            Self::Unavailable => UNAVAILABLE_MESSAGE.to_string(),
        }
    }

    /// HTTP status for this failure
    #[must_use]
    pub const fn status(self) -> axum::http::StatusCode {
        match self {
            Self::Denied => axum::http::StatusCode::FORBIDDEN,
            Self::ReauthRequired => axum::http::StatusCode::UNAUTHORIZED,
            Self::Unavailable => axum::http::StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

impl From<WorkspaceError> for ManagedError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::ReauthRequired => Self::ReauthRequired,
            WorkspaceError::Unavailable => Self::Unavailable,
        }
    }
}

/// The entity Beacon bills for a user's managed AI.
///
/// Omni bills workspaces, and a user's personal workspace stands in for the
/// individual, so the personal workspace pays (its plan's entitlements,
/// included credits, and card-verified free allowance apply). The user entity
/// is never billed: the workspace comes from cache or, when none is cached
/// (e.g. after a restart), from Gatekeeper's server-side lookup by user id.
/// Without billing configured managed AI is unavailable
///
/// # Errors
///
/// Returns [`ManagedError::ReauthRequired`] when Gatekeeper knows no personal
/// workspace for the user (or no lookup is configured and none is cached), and
/// [`ManagedError::Unavailable`] when billing is not configured or the lookup
/// itself fails
pub async fn billing_entity_for(
    state: &ApiState,
    user_id: &str,
) -> Result<BillingEntity, ManagedError> {
    let Some(billing) = &state.billing_state else {
        tracing::warn!(user_id = %user_id, "managed AI requested but billing is not configured");
        return Err(ManagedError::Unavailable);
    };

    billing
        .workspaces
        .resolve_user(user_id)
        .await
        .map_err(ManagedError::from)
}

/// Base URL of the Synapse gateway managed requests go to
pub(crate) fn synapse_base_url(state: &ApiState) -> String {
    state.synapse.as_ref().map_or_else(
        || "http://localhost:6000".to_string(),
        |c| c.base_url().to_string(),
    )
}

/// The user's product-scoped Beacon key
///
/// # Errors
///
/// Returns [`ManagedError`] when no product-scoped key can be provisioned
pub async fn managed_gateway_key(
    state: &ApiState,
    user_id: &str,
) -> Result<Arc<str>, ManagedError> {
    let Some(provisioner) = &state.key_provisioner else {
        tracing::warn!(user_id = %user_id, "managed AI requested but no key provisioner configured");
        return Err(ManagedError::Unavailable);
    };

    let entity = billing_entity_for(state, user_id).await?;
    provisioner
        .managed_key(user_id, &entity)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, user_id = %user_id, "managed key provisioning failed");
            match e {
                ProvisionError::Denied(_) => ManagedError::Denied,
                ProvisionError::Unavailable(_) => ManagedError::Unavailable,
            }
        })
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
    let key = managed_gateway_key(state, user_id).await?;

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

/// The gateway client a turn's auxiliary AI (memory, vision, transcription)
/// runs on
///
/// Cloud mode always uses the user's product-scoped Beacon key, so the work is
/// charged to the user's AI credits like chat; without an authenticated user
/// there is no billable key and the caller degrades. Self-hosted instances use
/// the turn's own gateway client (the operator's gateway and keys). `None`
/// means auxiliary AI is off for this turn
pub async fn gateway_client_for(
    state: &ApiState,
    gatekeeper_user_id: Option<&str>,
    turn_client: Option<&Arc<SynapseClient>>,
) -> Option<Arc<SynapseClient>> {
    if state.cloud_mode {
        let user_id = gatekeeper_user_id?;
        return managed_synapse_client(state, user_id)
            .await
            .map_err(|e| {
                tracing::debug!(user_id = %user_id, error = ?e, "gateway AI unavailable for this turn");
            })
            .ok();
    }

    turn_client.cloned().or_else(|| state.synapse.clone())
}

/// Memory AI (embeddings, fact extraction, query condensing) for a turn, on
/// the client [`gateway_client_for`] picks. `None` means memory falls back to
/// keyword search
pub async fn memory_ai_for(
    state: &ApiState,
    gatekeeper_user_id: Option<&str>,
    turn_client: Option<&Arc<SynapseClient>>,
) -> Option<MemoryAi> {
    gateway_client_for(state, gatekeeper_user_id, turn_client)
        .await
        .map(MemoryAi::new)
}

/// React to a gateway refusal on a managed request: a rejected key is
/// dropped so the next request provisions a fresh one
pub fn note_gateway_failure(state: &ApiState, user_id: &str, failure: GatewayFailure) {
    if failure == GatewayFailure::Unauthorized
        && let Some(provisioner) = &state.key_provisioner
        && let Some(entity) = state
            .billing_state
            .as_ref()
            .and_then(|billing| billing.workspaces.entity_for(user_id))
    {
        provisioner.invalidate(user_id, &entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::provisioner::BillingEntityType;

    #[test]
    fn beacon_bills_only_the_personal_workspace() {
        let resolver = crate::billing::WorkspaceResolver::new(None);
        assert_eq!(resolver.entity_for("u-1"), None);

        resolver.remember("u-1", "ws-1");
        let known = resolver.entity_for("u-1").unwrap();
        assert_eq!(known.entity_type, BillingEntityType::Organization);
        assert_eq!(known.entity_id, "ws-1");
    }

    #[test]
    fn an_unknown_workspace_asks_for_a_new_sign_in() {
        assert_eq!(
            ManagedError::from(WorkspaceError::ReauthRequired),
            ManagedError::ReauthRequired
        );
        assert_eq!(
            ManagedError::from(WorkspaceError::Unavailable),
            ManagedError::Unavailable
        );
        assert_eq!(ManagedError::ReauthRequired.code(), "reauth_required");
        assert_eq!(
            ManagedError::ReauthRequired.status(),
            axum::http::StatusCode::UNAUTHORIZED
        );
        assert!(
            ManagedError::ReauthRequired
                .user_message()
                .contains("Sign in")
        );
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
