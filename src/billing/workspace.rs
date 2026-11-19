//! Billing workspace resolution
//!
//! Omni bills workspaces: plans are bought for a workspace on the account hub,
//! and a user's personal workspace stands in for the individual. Beacon is a
//! personal assistant, so its plan, entitlements, meters, and managed AI
//! (credits included) all bill the signed-in user's personal workspace, and
//! never the user entity: a user-level balance would split the plan, miss the
//! card-verified free allowance, and drain a balance nobody tops up.
//!
//! Access tokens carry organization ids only, so the personal workspace is
//! read from Gatekeeper's userinfo (which keeps each organization's `type`)
//! with the user's own token, then cached per user and recorded in Beacon's
//! database. Work that runs without a token (scheduled check-ins, workflows)
//! uses the recorded workspace. Until one is known, nothing is billed and the
//! caller fails closed (the user signs in again)

use std::time::Duration;

use mini_moka::sync::Cache;

use crate::db::BillingWorkspaceRepo;
use crate::providers::provisioner::BillingEntity;

/// Suffix of the namespaced organizations claim (`<namespace>/organizations`)
const ORGANIZATIONS_CLAIM_SUFFIX: &str = "/claims/organizations";

/// How long a resolved personal workspace is reused from memory. A user's
/// personal workspace never changes, so this only bounds memory
const PERSONAL_WORKSPACE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a failed lookup is remembered, so an outage or a token without the
/// organization scope does not cost a userinfo call on every request
const MISS_TTL: Duration = Duration::from_secs(5 * 60);

/// Userinfo request timeout
const USERINFO_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a user's personal workspace could not be resolved
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceError {
    /// The user's token names no personal workspace (issued without the
    /// `organization` scope): they must sign in again
    ReauthRequired,
    /// The lookup failed or is not configured: nothing can be billed for now
    Unavailable,
}

/// Read the personal workspace id from a Gatekeeper userinfo response.
///
/// It is the organization whose `type` is `personal` in the namespaced
/// organizations claim. `None` when the claim is missing (no `organization`
/// scope) or holds no personal workspace
#[must_use]
pub fn personal_workspace_id(userinfo: &serde_json::Value) -> Option<String> {
    let claims = userinfo.as_object()?;

    claims
        .iter()
        .filter(|(key, _)| key.ends_with(ORGANIZATIONS_CLAIM_SUFFIX))
        .filter_map(|(_, value)| value.as_array())
        .flatten()
        .find(|org| org.get("type").and_then(|t| t.as_str()) == Some("personal"))
        .and_then(|org| org.get("id"))
        .and_then(|id| id.as_str())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
}

/// Cache key for a token's failed lookup (a hash, so tokens are not kept)
fn token_key(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

/// Resolves the workspace a user's Beacon usage bills
#[derive(Clone)]
pub struct WorkspaceResolver {
    userinfo_url: Option<String>,
    http: reqwest::Client,
    personal: Cache<String, String>,
    misses: Cache<u64, WorkspaceError>,
    store: Option<BillingWorkspaceRepo>,
}

impl WorkspaceResolver {
    /// Create a resolver against Gatekeeper at `auth_base_url`. Without one
    /// nothing can be resolved, so nobody is billed and managed AI fails
    /// closed
    #[must_use]
    pub fn new(auth_base_url: Option<&str>) -> Self {
        let userinfo_url = auth_base_url
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| format!("{}/oauth2/userinfo", url.trim_end_matches('/')));

        let http = reqwest::Client::builder()
            .timeout(USERINFO_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            userinfo_url,
            http,
            personal: Cache::builder()
                .max_capacity(100_000)
                .time_to_live(PERSONAL_WORKSPACE_TTL)
                .build(),
            misses: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(MISS_TTL)
                .build(),
            store: None,
        }
    }

    /// Record resolved workspaces in Beacon's database, so tokenless work
    /// keeps billing them across restarts
    #[must_use]
    pub fn with_store(mut self, store: BillingWorkspaceRepo) -> Self {
        self.store = Some(store);
        self
    }

    /// The workspace a user's usage bills, from what is already known: their
    /// personal workspace once resolved (in memory or recorded). `None` until
    /// one is known, and the caller must then bill nothing
    #[must_use]
    pub fn entity_for(&self, user_id: &str) -> Option<BillingEntity> {
        let key = user_id.to_string();

        if let Some(workspace_id) = self.personal.get(&key) {
            return Some(BillingEntity::organization(workspace_id));
        }

        let recorded = self.store.as_ref().and_then(|store| {
            store
                .get(user_id)
                .map_err(|e| {
                    tracing::warn!(user_id = %user_id, error = %e, "billing workspace read failed");
                })
                .ok()
                .flatten()
        })?;

        self.personal.insert(key, recorded.clone());
        Some(BillingEntity::organization(recorded))
    }

    /// Resolve (and remember) the user's personal workspace with their own
    /// token, then return the entity their usage bills
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::ReauthRequired`] when the token names no
    /// personal workspace, and [`WorkspaceError::Unavailable`] when the
    /// lookup fails. The user entity is never billed instead
    pub async fn resolve(
        &self,
        user_id: &str,
        token: &str,
    ) -> Result<BillingEntity, WorkspaceError> {
        if let Some(entity) = self.entity_for(user_id) {
            return Ok(entity);
        }

        // Misses are keyed by token, so signing in again is retried at once
        let miss_key = token_key(token);
        if let Some(error) = self.misses.get(&miss_key) {
            return Err(error);
        }

        match self.fetch_personal_workspace(token).await {
            Ok(Some(workspace_id)) => {
                self.remember(user_id, &workspace_id);
                Ok(BillingEntity::organization(workspace_id))
            }
            Ok(None) => {
                tracing::warn!(
                    user_id = %user_id,
                    "no personal workspace in userinfo, the user must sign in again"
                );
                self.misses.insert(miss_key, WorkspaceError::ReauthRequired);
                Err(WorkspaceError::ReauthRequired)
            }
            Err(e) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "personal workspace lookup failed, billing nothing"
                );
                self.misses.insert(miss_key, WorkspaceError::Unavailable);
                Err(WorkspaceError::Unavailable)
            }
        }
    }

    /// Record a user's personal workspace
    pub fn remember(&self, user_id: &str, workspace_id: &str) {
        self.personal
            .insert(user_id.to_string(), workspace_id.to_string());

        if let Some(store) = &self.store
            && let Err(e) = store.set(user_id, workspace_id)
        {
            tracing::warn!(user_id = %user_id, error = %e, "billing workspace write failed");
        }
    }

    async fn fetch_personal_workspace(&self, token: &str) -> Result<Option<String>, String> {
        let Some(url) = &self.userinfo_url else {
            return Err("no auth base URL configured".to_string());
        };

        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("userinfo request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("userinfo returned {status}"));
        }

        let userinfo: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("invalid userinfo response: {e}"))?;

        Ok(personal_workspace_id(&userinfo))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;

    use super::*;
    use crate::providers::provisioner::BillingEntityType;

    const USER: &str = "0b9f5e3c-6a1d-4c2e-9f3a-1b2c3d4e5f60";
    const PERSONAL: &str = "7c1e2d3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f";
    const TEAM: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

    fn userinfo() -> serde_json::Value {
        serde_json::json!({
            "sub": USER,
            "https://manifold.omni.dev/@omni/claims/organizations": [
                { "id": TEAM, "slug": "acme", "type": "team", "roles": ["owner"] },
                { "id": PERSONAL, "slug": "claude", "type": "personal", "roles": ["owner"] }
            ]
        })
    }

    #[test]
    fn reads_the_personal_workspace_from_userinfo() {
        assert_eq!(
            personal_workspace_id(&userinfo()).as_deref(),
            Some(PERSONAL)
        );
    }

    #[test]
    fn matches_the_claim_under_any_namespace() {
        let info = serde_json::json!({
            "https://manifold.example.dev/@omni/claims/organizations": [
                { "id": PERSONAL, "type": "personal" }
            ]
        });
        assert_eq!(personal_workspace_id(&info).as_deref(), Some(PERSONAL));
    }

    #[test]
    fn no_personal_workspace_without_the_claim_or_a_personal_entry() {
        assert_eq!(
            personal_workspace_id(&serde_json::json!({ "sub": USER })),
            None
        );

        let teams_only = serde_json::json!({
            "https://manifold.omni.dev/@omni/claims/organizations": [
                { "id": TEAM, "type": "team" }
            ]
        });
        assert_eq!(personal_workspace_id(&teams_only), None);

        // Slim token claims carry ids only, with no type to tell personal apart
        let slim = serde_json::json!({
            "https://manifold.omni.dev/@omni/claims/organizations": [{ "id": PERSONAL }]
        });
        assert_eq!(personal_workspace_id(&slim), None);
    }

    #[test]
    fn unknown_users_bill_nothing() {
        let resolver = WorkspaceResolver::new(None);
        assert_eq!(resolver.entity_for(USER), None);
    }

    #[test]
    fn remembered_users_bill_their_personal_workspace() {
        let resolver = WorkspaceResolver::new(None);
        resolver.remember(USER, PERSONAL);
        let entity = resolver.entity_for(USER).unwrap();
        assert_eq!(entity.entity_type, BillingEntityType::Organization);
        assert_eq!(entity.entity_id, PERSONAL);
        assert_eq!(entity.kind(), "organization");
    }

    #[test]
    fn a_recorded_workspace_survives_a_restart() {
        let store = BillingWorkspaceRepo::new(crate::db::init_memory().unwrap());

        WorkspaceResolver::new(None)
            .with_store(store.clone())
            .remember(USER, PERSONAL);

        // A fresh resolver (a restarted gateway) reads it back for tokenless work
        let restarted = WorkspaceResolver::new(None).with_store(store);
        assert_eq!(
            restarted.entity_for(USER),
            Some(BillingEntity::organization(PERSONAL))
        );
    }

    #[derive(Clone)]
    struct Mock {
        calls: Arc<AtomicUsize>,
        status: StatusCode,
    }

    async fn userinfo_handler(
        State(mock): State<Mock>,
        headers: HeaderMap,
    ) -> (StatusCode, Json<serde_json::Value>) {
        mock.calls.fetch_add(1, Ordering::SeqCst);
        let authorized =
            headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer user-token");
        if !authorized {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({})));
        }
        (mock.status, Json(userinfo()))
    }

    async fn spawn_gatekeeper(status: StatusCode) -> (String, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/oauth2/userinfo", get(userinfo_handler))
            .with_state(Mock {
                calls: Arc::clone(&calls),
                status,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), calls)
    }

    #[tokio::test]
    async fn resolves_with_the_users_token_and_caches_it() {
        let (base, calls) = spawn_gatekeeper(StatusCode::OK).await;
        let resolver = WorkspaceResolver::new(Some(&base));

        let entity = resolver.resolve(USER, "user-token").await;
        assert_eq!(entity, Ok(BillingEntity::organization(PERSONAL)));

        // Cached for later requests and for tokenless work
        let again = resolver.resolve(USER, "user-token").await;
        assert_eq!(again, Ok(BillingEntity::organization(PERSONAL)));
        assert_eq!(
            resolver.entity_for(USER),
            Some(BillingEntity::organization(PERSONAL))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_failed_lookup_bills_nothing_and_is_not_retried_at_once() {
        let (base, calls) = spawn_gatekeeper(StatusCode::INTERNAL_SERVER_ERROR).await;
        let resolver = WorkspaceResolver::new(Some(&base));

        assert_eq!(
            resolver.resolve(USER, "user-token").await,
            Err(WorkspaceError::Unavailable)
        );
        assert_eq!(
            resolver.resolve(USER, "user-token").await,
            Err(WorkspaceError::Unavailable)
        );
        assert_eq!(resolver.entity_for(USER), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_token_without_a_personal_workspace_asks_for_a_new_sign_in() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let app = Router::new().route(
            "/oauth2/userinfo",
            get(move |headers: HeaderMap| {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let token = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default();
                    if token == "Bearer fresh-token" {
                        Json(userinfo())
                    } else {
                        Json(serde_json::json!({ "sub": USER }))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let resolver = WorkspaceResolver::new(Some(&format!("http://{addr}")));

        assert_eq!(
            resolver.resolve(USER, "scopeless-token").await,
            Err(WorkspaceError::ReauthRequired)
        );
        assert_eq!(
            resolver.resolve(USER, "scopeless-token").await,
            Err(WorkspaceError::ReauthRequired)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Signing in again yields a new token, which is looked up at once
        assert_eq!(
            resolver.resolve(USER, "fresh-token").await,
            Ok(BillingEntity::organization(PERSONAL))
        );
    }
}
