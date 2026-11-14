//! Aether billing integration for Beacon gateway
//!
//! Beacon is a credit product: managed AI runs through a product-scoped
//! Synapse key (`appId` = [`BEACON_APP_ID`], billed to the user), and the
//! Synapse gateway is the only place cost is converted to credits, checked
//! (402 before spend), and deducted from the pooled Omni balance. Beacon never
//! meters, prices, or deducts usage itself.
//!
//! This module covers what Beacon still asks Aether directly: feature
//! entitlements (`api_access`, `voice_enabled`), the conversations meter, and
//! the plan tier that picks the default model. Enabled when
//! `BILLING_BASE_URL` is set.

pub mod gateway_error;
pub mod middleware;
pub mod pricing;
pub mod tier;

use std::sync::Arc;
use std::time::Duration;

use mini_moka::sync::Cache;
use secrecy::SecretString;
use synapse_billing::AetherClient;

pub use tier::{FREE_TIER_MODEL, PlanTier};

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
        let client = AetherClient::new(
            config.base_url,
            BEACON_APP_ID.to_string(),
            config.service_api_key,
        )?;

        tracing::info!(app_id = BEACON_APP_ID, "Aether billing enabled");

        Ok(Self {
            client: Arc::new(client),
            fail_mode: config.fail_mode,
            cache: BillingCache::new(config.cache_ttl_secs),
        })
    }

    /// Resolve a user's plan tier, cached for the TTL.
    ///
    /// Any Aether failure resolves to `Free`, so an outage can only make the
    /// default model cheaper, never more expensive
    pub async fn user_tier(&self, user_id: &str) -> PlanTier {
        if let Some(tier) = self.cache.get_tier(user_id) {
            return tier;
        }

        let tier = match self.client.get_entitlements("user", user_id).await {
            Ok(entitlements) => tier::tier_from_entitlements(&entitlements),
            Err(e) => {
                tracing::warn!(error = %e, "tier lookup failed, defaulting to free tier model");
                PlanTier::Free
            }
        };

        self.cache.put_tier(user_id, tier);
        tier
    }
}

/// Default managed model for a user: the free tier gets [`FREE_TIER_MODEL`],
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
    tiers: Cache<String, PlanTier>,
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

    /// Look up a cached plan tier for a user
    #[must_use]
    pub fn get_tier(&self, user_id: &str) -> Option<PlanTier> {
        self.tiers.get(&user_id.to_string())
    }

    /// Store a user's plan tier in the cache
    pub fn put_tier(&self, user_id: &str, tier: PlanTier) {
        self.tiers.insert(user_id.to_string(), tier);
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

    #[tokio::test]
    async fn default_model_without_billing_is_the_free_tier_model() {
        let model = default_model_for_user(None, "u", "anthropic/claude-sonnet-5-5").await;
        assert_eq!(model, FREE_TIER_MODEL);
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
        state.cache.put_tier("paid-user", PlanTier::Paid);
        let model =
            default_model_for_user(Some(&state), "paid-user", "anthropic/claude-sonnet-5-5").await;
        assert_eq!(model, "anthropic/claude-sonnet-5-5");
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
        assert_eq!(model, FREE_TIER_MODEL);
    }
}
