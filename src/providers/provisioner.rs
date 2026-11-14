//! Product-scoped managed key provisioning via Synapse API
//!
//! Beacon mints a per-user managed key with `appId` = `beacon` and the
//! billing entity that pays. The Synapse gateway then checks entitlements,
//! refuses with 402 before spend when the pooled Omni balance is empty, meters
//! the request, and deducts credits against Beacon's Aether app. Beacon keeps
//! no ledger and never falls back to a shared key: when provisioning fails the
//! AI feature is unavailable

use std::sync::Arc;
use std::time::Duration;

use mini_moka::sync::Cache;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

/// Key name prefix Synapse stores the managed key under (`"Beacon (auto)"`)
const KEY_SOURCE: &str = "Beacon";

/// How long a provisioned key is reused before a fresh one is minted
const KEY_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Response from the Synapse provision-managed-key endpoint
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedKey {
    /// Raw API key (synapse_...)
    pub api_key: String,
    /// Last 4 characters of the key
    pub key_hint: String,
    /// Synapse user ID
    pub user_id: String,
    /// User's Synapse plan tier (not Beacon's tier)
    pub plan: String,
}

/// Entity types Aether bills
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingEntityType {
    /// A personal account
    User,
    /// A workspace (organization)
    Organization,
}

/// The tenant a managed key bills
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingEntity {
    /// Entity type
    pub entity_type: BillingEntityType,
    /// Entity id (Gatekeeper user id or organization id)
    pub entity_id: String,
}

impl BillingEntity {
    /// Bill a user's personal account (Beacon is a personal assistant)
    #[must_use]
    pub fn user(user_id: impl Into<String>) -> Self {
        Self {
            entity_type: BillingEntityType::User,
            entity_id: user_id.into(),
        }
    }
}

/// Why a managed key could not be provisioned
#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    /// The billed account lacks the `api_access` entitlement (403)
    #[error("managed AI access denied: {0}")]
    Denied(String),
    /// Synapse rejected or failed the request; managed AI is unavailable
    #[error("managed key provisioning unavailable: {0}")]
    Unavailable(String),
}

/// Request body for the provision endpoint
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionRequest<'a> {
    identity_provider_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    source: &'a str,
    app_id: &'a str,
    billing_entity: &'a BillingEntity,
}

/// Cache identity for a managed key: one per (user, app, billed entity)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ManagedKeyId {
    user_id: String,
    app_id: String,
    entity: BillingEntity,
}

/// Client for provisioning product-scoped managed keys via Synapse API
#[derive(Clone)]
pub struct KeyProvisioner {
    client: Client,
    base_url: String,
    gateway_secret: String,
    app_id: String,
    keys: Cache<ManagedKeyId, Arc<str>>,
    // Serializes cache misses: every provision call rotates the key, so two
    // concurrent misses would invalidate each other's key
    miss_lock: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for KeyProvisioner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyProvisioner")
            .field("base_url", &self.base_url)
            .field("app_id", &self.app_id)
            .finish_non_exhaustive()
    }
}

impl KeyProvisioner {
    /// Create a new key provisioner
    ///
    /// # Arguments
    ///
    /// * `synapse_api_url` - Base URL for the Synapse API (e.g., `https://api.synapse.omni.dev`)
    /// * `gateway_secret` - Shared secret for `x-gateway-secret` header
    /// * `app_id` - Aether app the keys bill (Beacon's product slug)
    #[must_use]
    pub fn new(
        synapse_api_url: impl Into<String>,
        gateway_secret: impl Into<String>,
        app_id: impl Into<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            base_url: synapse_api_url.into(),
            gateway_secret: gateway_secret.into(),
            app_id: app_id.into(),
            keys: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(KEY_CACHE_TTL)
                .build(),
            miss_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Aether app the provisioned keys bill
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    fn key_id(&self, user_id: &str, entity: &BillingEntity) -> ManagedKeyId {
        ManagedKeyId {
            user_id: user_id.to_string(),
            app_id: self.app_id.clone(),
            entity: entity.clone(),
        }
    }

    /// Get the user's managed key for `entity`, provisioning one on a cache
    /// miss
    ///
    /// # Errors
    ///
    /// Returns [`ProvisionError::Denied`] when the account lacks access and
    /// [`ProvisionError::Unavailable`] for any other failure
    pub async fn managed_key(
        &self,
        user_id: &str,
        entity: &BillingEntity,
    ) -> Result<Arc<str>, ProvisionError> {
        let id = self.key_id(user_id, entity);
        if let Some(key) = self.keys.get(&id) {
            return Ok(key);
        }

        let _guard = self.miss_lock.lock().await;
        // Another task may have provisioned while this one waited
        if let Some(key) = self.keys.get(&id) {
            return Ok(key);
        }

        let provisioned = self.provision(user_id, entity).await?;
        tracing::info!(
            user_id = %user_id,
            app_id = %self.app_id,
            key_hint = %provisioned.key_hint,
            "provisioned product-scoped managed key"
        );

        let key: Arc<str> = Arc::from(provisioned.api_key);
        self.keys.insert(id, Arc::clone(&key));
        Ok(key)
    }

    /// Drop a cached key, e.g. after the gateway rejected it (rotated or
    /// revoked), so the next request provisions a fresh one
    pub fn invalidate(&self, user_id: &str, entity: &BillingEntity) {
        self.keys.invalidate(&self.key_id(user_id, entity));
    }

    /// Provision (or rotate) a managed key with Synapse
    async fn provision(
        &self,
        identity_provider_id: &str,
        entity: &BillingEntity,
    ) -> Result<ProvisionedKey, ProvisionError> {
        let url = format!(
            "{}/internal/provision-managed-key",
            self.base_url.trim_end_matches('/')
        );

        let body = ProvisionRequest {
            identity_provider_id,
            email: None,
            name: None,
            source: KEY_SOURCE,
            app_id: &self.app_id,
            billing_entity: entity,
        };

        let response = self
            .client
            .post(&url)
            .header("x-gateway-secret", &self.gateway_secret)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProvisionError::Unavailable(format!("request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(if status == StatusCode::FORBIDDEN {
                ProvisionError::Denied(body)
            } else {
                ProvisionError::Unavailable(format!("{status} - {body}"))
            });
        }

        response
            .json()
            .await
            .map_err(|e| ProvisionError::Unavailable(format!("invalid response: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use axum::extract::State;
    use axum::http::StatusCode as AxumStatus;
    use axum::routing::post;

    use super::*;

    const USER: &str = "0b9f5e3c-6a1d-4c2e-9f3a-1b2c3d4e5f60";

    #[derive(Clone)]
    struct Mock {
        calls: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<serde_json::Value>>>,
        status: AxumStatus,
    }

    async fn handler(
        State(mock): State<Mock>,
        Json(body): Json<serde_json::Value>,
    ) -> (AxumStatus, Json<serde_json::Value>) {
        let n = mock.calls.fetch_add(1, Ordering::SeqCst);
        mock.bodies.lock().unwrap().push(body);
        (
            mock.status,
            Json(serde_json::json!({
                "apiKey": format!("synapse_key_{n}"),
                "keyHint": "abcd",
                "userId": "synapse-user",
                "plan": "free"
            })),
        )
    }

    async fn spawn_mock(status: AxumStatus) -> (String, Mock) {
        let mock = Mock {
            calls: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
            status,
        };
        let app = axum::Router::new()
            .route("/internal/provision-managed-key", post(handler))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), mock)
    }

    #[test]
    fn request_carries_app_id_and_billing_entity() {
        let entity = BillingEntity::user(USER);
        let body = ProvisionRequest {
            identity_provider_id: USER,
            email: None,
            name: None,
            source: KEY_SOURCE,
            app_id: "beacon",
            billing_entity: &entity,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["identityProviderId"], USER);
        assert_eq!(json["appId"], "beacon");
        assert_eq!(json["source"], "Beacon");
        assert_eq!(json["billingEntity"]["entityType"], "user");
        assert_eq!(json["billingEntity"]["entityId"], USER);
        assert!(json.get("email").is_none());
    }

    #[test]
    fn provisioned_key_deserialization() {
        let json =
            r#"{"apiKey":"synapse_abc123","keyHint":"c123","userId":"user-456","plan":"free"}"#;
        let key: ProvisionedKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.api_key, "synapse_abc123");
        assert_eq!(key.key_hint, "c123");
        assert_eq!(key.user_id, "user-456");
        assert_eq!(key.plan, "free");
    }

    #[tokio::test]
    async fn provisions_once_per_user_app_and_entity() {
        let (url, mock) = spawn_mock(AxumStatus::OK).await;
        let provisioner = KeyProvisioner::new(url, "secret", "beacon");
        let entity = BillingEntity::user(USER);

        let first = provisioner.managed_key(USER, &entity).await.unwrap();
        let second = provisioner.managed_key(USER, &entity).await.unwrap();

        assert_eq!(&*first, "synapse_key_0");
        assert_eq!(first, second);
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);

        let bodies = mock.bodies.lock().unwrap();
        assert_eq!(bodies[0]["appId"], "beacon");
        assert_eq!(bodies[0]["billingEntity"]["entityType"], "user");
        assert_eq!(bodies[0]["billingEntity"]["entityId"], USER);
        drop(bodies);
    }

    #[tokio::test]
    async fn concurrent_misses_provision_once() {
        let (url, mock) = spawn_mock(AxumStatus::OK).await;
        let provisioner = KeyProvisioner::new(url, "secret", "beacon");
        let entity = BillingEntity::user(USER);

        let (a, b) = tokio::join!(
            provisioner.managed_key(USER, &entity),
            provisioner.managed_key(USER, &entity)
        );
        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalidate_forces_a_fresh_key() {
        let (url, mock) = spawn_mock(AxumStatus::OK).await;
        let provisioner = KeyProvisioner::new(url, "secret", "beacon");
        let entity = BillingEntity::user(USER);

        let first = provisioner.managed_key(USER, &entity).await.unwrap();
        provisioner.invalidate(USER, &entity);
        let second = provisioner.managed_key(USER, &entity).await.unwrap();

        assert_ne!(first, second);
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn forbidden_is_denied_and_not_cached() {
        let (url, mock) = spawn_mock(AxumStatus::FORBIDDEN).await;
        let provisioner = KeyProvisioner::new(url, "secret", "beacon");
        let entity = BillingEntity::user(USER);

        let err = provisioner.managed_key(USER, &entity).await.unwrap_err();
        assert!(matches!(err, ProvisionError::Denied(_)));
        let _ = provisioner.managed_key(USER, &entity).await;
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn server_errors_are_unavailable() {
        let (url, _mock) = spawn_mock(AxumStatus::INTERNAL_SERVER_ERROR).await;
        let provisioner = KeyProvisioner::new(url, "secret", "beacon");
        let err = provisioner
            .managed_key(USER, &BillingEntity::user(USER))
            .await
            .unwrap_err();
        assert!(matches!(err, ProvisionError::Unavailable(_)));
    }

    #[tokio::test]
    async fn unreachable_synapse_is_unavailable() {
        let provisioner = KeyProvisioner::new("http://127.0.0.1:9", "secret", "beacon");
        let err = provisioner
            .managed_key(USER, &BillingEntity::user(USER))
            .await
            .unwrap_err();
        assert!(matches!(err, ProvisionError::Unavailable(_)));
    }
}
