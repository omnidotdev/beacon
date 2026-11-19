//! Aether billing integration for Beacon gateway
//!
//! Beacon is a credit product: managed AI runs through a product-scoped
//! Synapse key (`appId` = [`BEACON_APP_ID`], billed to the user's personal
//! workspace, see [`workspace`]), and the Synapse gateway is the only place
//! cost is converted to credits, checked (402 before spend), and deducted from
//! that workspace's balance. Beacon never meters, prices, or deducts usage
//! itself.
//!
//! This module covers what Beacon still asks Aether directly: feature
//! entitlements (`api_access`, `voice_enabled`), the conversations meter, and
//! the plan tier that picks the default model, all for the user's personal
//! workspace. Enabled when `BILLING_BASE_URL` is set.

pub mod card_offer;
pub mod gateway_error;
pub mod middleware;
pub mod pricing;
pub mod tier;
pub mod workspace;

use std::sync::Arc;
use std::time::Duration;

use mini_moka::sync::Cache;
use secrecy::{ExposeSecret, SecretString};
use synapse_billing::AetherClient;

pub use tier::{GATEWAY_DEFAULT_MODEL, PlanTier};
pub use workspace::{WorkspaceError, WorkspaceResolver};

use crate::providers::provisioner::BillingEntity;

/// Aether app id Beacon bills and checks entitlements against.
///
/// Fixed rather than configurable: a managed key and the entitlements that
/// gate it must name the same product, and Beacon is never billed as Synapse
pub const BEACON_APP_ID: &str = "beacon";

/// Billing base URL env names, fleet standard first, legacy names after
const BASE_URL_VARS: &[&str] = &["BILLING_BASE_URL", "BILLING_API_URL", "AETHER_URL"];

/// Billing service key env names, fleet standard first, legacy names after
const SERVICE_KEY_VARS: &[&str] = &[
    "BILLING_SERVICE_API_KEY",
    "BILLING_API_KEY",
    "AETHER_SERVICE_API_KEY",
];

/// Fail mode env names, fleet standard first
const FAIL_MODE_VARS: &[&str] = &["BILLING_FAIL_MODE", "AETHER_FAIL_MODE"];

/// Cache TTL env names, fleet standard first
const CACHE_TTL_VARS: &[&str] = &["BILLING_CACHE_TTL_SECS", "AETHER_CACHE_TTL_SECS"];

/// Gatekeeper base URL env names (the personal workspace lookup), the same
/// ones JWT validation reads
const AUTH_BASE_URL_VARS: &[&str] = &["AUTH_BASE_URL", "GATEKEEPER_URL"];

/// Fail mode used when Aether is unreachable
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailMode {
    /// Allow the request through and log a warning
    Open,
    /// Reject the request with 503
    Closed,
}

/// Billing settings resolved from the environment
#[derive(Clone, Debug)]
pub struct BillingConfig {
    /// Aether base URL
    pub base_url: url::Url,
    /// Aether service API key
    pub service_api_key: SecretString,
    /// Behavior when Aether is unreachable
    pub fail_mode: FailMode,
    /// TTL for cached entitlement, usage, and tier results
    pub cache_ttl_secs: u64,
    /// Gatekeeper base URL, for resolving a user's personal workspace
    pub auth_base_url: Option<String>,
}

impl BillingConfig {
    /// Resolve billing settings through `lookup` (the process env in
    /// production).
    ///
    /// Returns `None` when no billing base URL is set (billing disabled).
    ///
    /// # Errors
    ///
    /// Returns an error if the base URL is invalid or no service key is set
    /// alongside it.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Self>> {
        let first = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| lookup(name).filter(|value| !value.trim().is_empty()))
        };

        let Some(base_url) = first(BASE_URL_VARS) else {
            return Ok(None);
        };
        let base_url: url::Url = base_url.trim().parse()?;

        let service_api_key = first(SERVICE_KEY_VARS).ok_or_else(|| {
            anyhow::anyhow!("BILLING_SERVICE_API_KEY is required when BILLING_BASE_URL is set")
        })?;

        let fail_mode = match first(FAIL_MODE_VARS).as_deref().map(str::trim) {
            Some("closed") => FailMode::Closed,
            _ => FailMode::Open,
        };

        let cache_ttl_secs = first(CACHE_TTL_VARS)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(60);

        Ok(Some(Self {
            base_url,
            service_api_key: SecretString::new(service_api_key.into()),
            fail_mode,
            cache_ttl_secs,
            auth_base_url: first(AUTH_BASE_URL_VARS),
        }))
    }
}

/// Shared billing state passed to the middleware
#[derive(Clone)]
pub struct BillingState {
    /// Aether API client, scoped to [`BEACON_APP_ID`]
    pub client: Arc<AetherClient>,
    /// Fail mode for Aether errors
    pub fail_mode: FailMode,
    /// TTL cache for entitlement and usage results
    pub cache: BillingCache,
    /// Which workspace each user's usage bills
    pub workspaces: WorkspaceResolver,
    /// Read-only Aether access for the plan display (credit balance)
    balance_reader: Arc<BalanceReader>,
}

impl BillingState {
    /// Aether base URL
    #[must_use]
    pub fn base_url(&self) -> &url::Url {
        &self.balance_reader.base_url
    }
}

/// Reads a billing entity's credit balance for display. Never checks or
/// spends credits: the Synapse gateway is the only place credits are checked
/// and deducted
struct BalanceReader {
    base_url: url::Url,
    service_api_key: SecretString,
    http: reqwest::Client,
}

/// Read a credit balance (retail USD) from Aether's balance response. The
/// balance is a `numeric` column, so it may arrive as a JSON number or as a
/// numeric string
#[must_use]
pub fn balance_from_json(body: &serde_json::Value) -> Option<f64> {
    match body.get("balance")? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|balance: &f64| balance.is_finite())
}

impl BillingState {
    /// Initialize from environment variables.
    ///
    /// Returns `None` if `BILLING_BASE_URL` (or a legacy alias) is not set
    /// (billing disabled).
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid or the `AetherClient`
    /// cannot be constructed.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        BillingConfig::from_lookup(|name| std::env::var(name).ok())?
            .map(Self::from_config)
            .transpose()
    }

    /// Build billing state from resolved settings
    ///
    /// # Errors
    ///
    /// Returns an error if the `AetherClient` cannot be constructed.
    pub fn from_config(config: BillingConfig) -> anyhow::Result<Self> {
        let balance_reader = BalanceReader {
            base_url: config.base_url.clone(),
            service_api_key: SecretString::from(config.service_api_key.expose_secret().to_owned()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?,
        };

        let client = AetherClient::new(
            config.base_url,
            BEACON_APP_ID.to_string(),
            config.service_api_key,
        )?;

        if config.auth_base_url.is_none() {
            tracing::warn!(
                "AUTH_BASE_URL not set, personal workspaces cannot be resolved and managed AI is disabled"
            );
        }

        tracing::info!(app_id = BEACON_APP_ID, "Aether billing enabled");

        Ok(Self {
            client: Arc::new(client),
            fail_mode: config.fail_mode,
            cache: BillingCache::new(config.cache_ttl_secs),
            workspaces: WorkspaceResolver::new(config.auth_base_url.as_deref()),
            balance_reader: Arc::new(balance_reader),
        })
    }

    /// Record each user's resolved personal workspace in Beacon's database,
    /// so tokenless work keeps billing it across restarts
    #[must_use]
    pub fn with_workspace_store(mut self, store: crate::db::BillingWorkspaceRepo) -> Self {
        self.workspaces = self.workspaces.with_store(store);
        self
    }

    /// A billing entity's credit balance (retail USD), for display only.
    /// `Ok(None)` when the entity has no billing account yet
    ///
    /// # Errors
    ///
    /// Returns an error if Aether is unreachable or refuses the read
    pub async fn credit_balance(&self, entity: &BillingEntity) -> Result<Option<f64>, String> {
        let reader = &self.balance_reader;
        let url = reader
            .base_url
            .join(&format!(
                "credits/{BEACON_APP_ID}/{}/{}/balance",
                entity.kind(),
                entity.entity_id
            ))
            .map_err(|e| format!("invalid balance URL: {e}"))?;

        let response = reader
            .http
            .get(url)
            .header("x-service-api-key", reader.service_api_key.expose_secret())
            .send()
            .await
            .map_err(|e| format!("balance request failed: {e}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(format!("balance read returned {status}"));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("invalid balance response: {e}"))?;

        Ok(balance_from_json(&body))
    }

    /// Resolve the plan tier of the workspace a user's usage bills (their
    /// personal workspace), cached for the TTL.
    ///
    /// An unknown workspace or any Aether failure resolves to `Free`, so it
    /// can only make the default model cheaper, never more expensive
    pub async fn user_tier(&self, user_id: &str) -> PlanTier {
        match self.workspaces.entity_for(user_id) {
            Some(entity) => self.entity_tier(&entity).await,
            None => PlanTier::Free,
        }
    }

    /// Resolve a billing entity's plan tier, cached for the TTL
    pub async fn entity_tier(&self, entity: &BillingEntity) -> PlanTier {
        if let Some(tier) = self.cache.get_tier(entity) {
            return tier;
        }

        let tier = match self
            .client
            .get_entitlements(entity.kind(), &entity.entity_id)
            .await
        {
            Ok(entitlements) => tier::tier_from_entitlements(&entitlements),
            Err(e) => {
                tracing::warn!(error = %e, "tier lookup failed, defaulting to free tier model");
                PlanTier::Free
            }
        };

        self.cache.put_tier(entity, tier);
        tier
    }
}

/// Default managed model for a user: the free tier gets [`GATEWAY_DEFAULT_MODEL`],
/// paid tiers keep `paid_default`. Without billing the tier is unknown, so the
/// free default applies
pub async fn default_model_for_user(
    billing: Option<&BillingState>,
    user_id: &str,
    paid_default: &str,
) -> String {
    let tier = match billing {
        Some(state) => state.user_tier(user_id).await,
        None => PlanTier::Free,
    };
    tier.default_model(paid_default)
}

/// Cache key for entitlement checks
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
struct EntitlementKey {
    entity_type: String,
    entity_id: String,
    feature_key: String,
}

/// Cache key for usage checks
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
struct UsageKey {
    entity_type: String,
    entity_id: String,
    meter_key: String,
}

/// Cached entitlement result
///
/// Both granted (`has_access: true`) and denied (`has_access: false`) outcomes
/// are cached for the full TTL. This is intentional: it protects Aether from
/// per-request load and is consistent with synapse-gateway's approach. The
/// tradeoff is that a newly upgraded user may wait up to one TTL period before
/// their entitlement is reflected.
#[derive(Clone, Debug)]
pub struct CachedEntitlement {
    pub has_access: bool,
}

/// Cached usage check result
///
/// `allowed: false` results are also cached for the full TTL so that
/// exhausted-quota requests don't spam Aether. Users who add more quota will
/// see it reflected after at most one TTL period.
#[derive(Clone, Debug)]
pub struct CachedUsage {
    pub allowed: bool,
}

/// TTL-based cache for entitlement and usage check results
#[derive(Clone, Debug)]
pub struct BillingCache {
    entitlements: Cache<EntitlementKey, CachedEntitlement>,
    usage: Cache<UsageKey, CachedUsage>,
    tiers: Cache<BillingEntity, PlanTier>,
}

impl BillingCache {
    /// Create a new cache with the given TTL in seconds
    #[must_use]
    pub fn new(ttl_secs: u64) -> Self {
        let ttl = Duration::from_secs(ttl_secs);
        Self {
            entitlements: Cache::builder()
                .max_capacity(1024)
                .time_to_live(ttl)
                .build(),
            usage: Cache::builder()
                .max_capacity(1024)
                .time_to_live(ttl)
                .build(),
            tiers: Cache::builder()
                .max_capacity(4096)
                .time_to_live(ttl)
                .build(),
        }
    }

    /// Look up a cached entitlement result
    #[must_use]
    pub fn get_entitlement(
        &self,
        entity_type: &str,
        entity_id: &str,
        feature_key: &str,
    ) -> Option<CachedEntitlement> {
        self.entitlements.get(&EntitlementKey {
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            feature_key: feature_key.to_string(),
        })
    }

    /// Store an entitlement result in the cache
    pub fn put_entitlement(
        &self,
        entity_type: &str,
        entity_id: &str,
        feature_key: &str,
        value: CachedEntitlement,
    ) {
        self.entitlements.insert(
            EntitlementKey {
                entity_type: entity_type.to_string(),
                entity_id: entity_id.to_string(),
                feature_key: feature_key.to_string(),
            },
            value,
        );
    }

    /// Look up a cached usage check result
    #[must_use]
    pub fn get_usage(
        &self,
        entity_type: &str,
        entity_id: &str,
        meter_key: &str,
    ) -> Option<CachedUsage> {
        self.usage.get(&UsageKey {
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            meter_key: meter_key.to_string(),
        })
    }

    /// Store a usage check result in the cache
    pub fn put_usage(
        &self,
        entity_type: &str,
        entity_id: &str,
        meter_key: &str,
        value: CachedUsage,
    ) {
        self.usage.insert(
            UsageKey {
                entity_type: entity_type.to_string(),
                entity_id: entity_id.to_string(),
                meter_key: meter_key.to_string(),
            },
            value,
        );
    }

    /// Look up a cached plan tier for a billing entity
    #[must_use]
    pub fn get_tier(&self, entity: &BillingEntity) -> Option<PlanTier> {
        self.tiers.get(entity)
    }

    /// Store a billing entity's plan tier in the cache
    pub fn put_tier(&self, entity: &BillingEntity, tier: PlanTier) {
        self.tiers.insert(entity.clone(), tier);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use secrecy::ExposeSecret;

    use super::*;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn disabled_without_a_base_url() {
        let config =
            BillingConfig::from_lookup(lookup(&[("BILLING_SERVICE_API_KEY", "k")])).unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn reads_fleet_standard_names() {
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "https://api.billing.omni.dev"),
            ("BILLING_SERVICE_API_KEY", "svc-key"),
            ("BILLING_FAIL_MODE", "closed"),
            ("BILLING_CACHE_TTL_SECS", "30"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(config.base_url.as_str(), "https://api.billing.omni.dev/");
        assert_eq!(config.service_api_key.expose_secret(), "svc-key");
        assert_eq!(config.fail_mode, FailMode::Closed);
        assert_eq!(config.cache_ttl_secs, 30);
    }

    #[test]
    fn falls_back_to_the_names_prod_sets_today() {
        // The current FractalService sets BILLING_API_URL / BILLING_API_KEY
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_API_URL", "https://api.billing.omni.dev"),
            ("BILLING_API_KEY", "legacy-key"),
            ("BILLING_FAIL_MODE", "open"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(config.service_api_key.expose_secret(), "legacy-key");
        assert_eq!(config.fail_mode, FailMode::Open);
    }

    #[test]
    fn falls_back_to_the_original_aether_names() {
        let config = BillingConfig::from_lookup(lookup(&[
            ("AETHER_URL", "https://aether.example"),
            ("AETHER_SERVICE_API_KEY", "old-key"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(config.base_url.as_str(), "https://aether.example/");
        assert_eq!(config.service_api_key.expose_secret(), "old-key");
        assert_eq!(config.cache_ttl_secs, 60);
    }

    #[test]
    fn standard_names_win_over_legacy_names() {
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "https://new.example"),
            ("BILLING_API_URL", "https://old.example"),
            ("BILLING_SERVICE_API_KEY", "new"),
            ("BILLING_API_KEY", "old"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(config.base_url.as_str(), "https://new.example/");
        assert_eq!(config.service_api_key.expose_secret(), "new");
    }

    #[test]
    fn errors_without_a_service_key() {
        let result =
            BillingConfig::from_lookup(lookup(&[("BILLING_BASE_URL", "https://x.example")]));
        assert!(result.is_err());
    }

    #[test]
    fn app_id_is_beacon_regardless_of_env() {
        // BILLING_APP_ID / AETHER_APP_ID are ignored; Beacon never bills as Synapse
        assert_eq!(BEACON_APP_ID, "beacon");
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "https://api.billing.omni.dev"),
            ("BILLING_SERVICE_API_KEY", "k"),
            ("BILLING_APP_ID", "synapse"),
        ]))
        .unwrap()
        .unwrap();
        assert!(BillingState::from_config(config).is_ok());
    }

    #[test]
    fn balance_reads_both_wire_shapes() {
        // Aether's balance is a numeric column: a JSON number or a string
        assert_eq!(
            balance_from_json(&serde_json::json!({ "balance": 3.5 })),
            Some(3.5)
        );
        assert_eq!(
            balance_from_json(&serde_json::json!({ "balance": "3.5000" })),
            Some(3.5)
        );
        assert_eq!(
            balance_from_json(&serde_json::json!({ "balance": 0 })),
            Some(0.0)
        );
        assert_eq!(
            balance_from_json(&serde_json::json!({ "balance": "n/a" })),
            None
        );
        assert_eq!(balance_from_json(&serde_json::json!({})), None);
    }

    #[tokio::test]
    async fn default_model_without_billing_is_the_free_tier_model() {
        let model = default_model_for_user(None, "u", "anthropic/claude-sonnet-5-5").await;
        assert_eq!(model, GATEWAY_DEFAULT_MODEL);
    }

    #[tokio::test]
    async fn cached_paid_tier_keeps_the_paid_default() {
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "http://127.0.0.1:9"),
            ("BILLING_SERVICE_API_KEY", "k"),
        ]))
        .unwrap()
        .unwrap();
        let state = BillingState::from_config(config).unwrap();
        state.workspaces.remember("paid-user", "paid-workspace");
        state.cache.put_tier(
            &BillingEntity::organization("paid-workspace"),
            PlanTier::Paid,
        );
        let model =
            default_model_for_user(Some(&state), "paid-user", "anthropic/claude-sonnet-5-5").await;
        assert_eq!(model, "anthropic/claude-sonnet-5-5");
    }

    #[tokio::test]
    async fn the_tier_follows_the_personal_workspace_not_the_user() {
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "http://127.0.0.1:9"),
            ("BILLING_SERVICE_API_KEY", "k"),
        ]))
        .unwrap()
        .unwrap();
        let state = BillingState::from_config(config).unwrap();
        // Another workspace's paid tier never applies to this user
        state.cache.put_tier(
            &BillingEntity::organization("team-workspace"),
            PlanTier::Paid,
        );
        assert_eq!(state.user_tier("u").await, PlanTier::Free);
        state.workspaces.remember("u", "personal-workspace");
        state.cache.put_tier(
            &BillingEntity::organization("personal-workspace"),
            PlanTier::Free,
        );
        assert_eq!(state.user_tier("u").await, PlanTier::Free);
    }

    #[test]
    fn reads_the_auth_base_url_for_the_workspace_lookup() {
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "https://api.billing.omni.dev"),
            ("BILLING_SERVICE_API_KEY", "k"),
            ("AUTH_BASE_URL", "https://identity.omni.dev"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(
            config.auth_base_url.as_deref(),
            Some("https://identity.omni.dev")
        );

        let legacy = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "https://api.billing.omni.dev"),
            ("BILLING_SERVICE_API_KEY", "k"),
            ("GATEKEEPER_URL", "https://gatekeeper.example"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(
            legacy.auth_base_url.as_deref(),
            Some("https://gatekeeper.example")
        );
    }

    #[tokio::test]
    async fn unreachable_aether_defaults_to_the_free_tier_model() {
        let config = BillingConfig::from_lookup(lookup(&[
            ("BILLING_BASE_URL", "http://127.0.0.1:9"),
            ("BILLING_SERVICE_API_KEY", "k"),
        ]))
        .unwrap()
        .unwrap();
        let state = BillingState::from_config(config).unwrap();
        let model = default_model_for_user(Some(&state), "u", "anthropic/claude-sonnet-5-5").await;
        assert_eq!(model, GATEWAY_DEFAULT_MODEL);
    }
}
