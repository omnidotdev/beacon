//! Billing workspace resolution
//!
//! Omni bills workspaces: plans are bought for a workspace on the account hub,
//! and a user's personal workspace stands in for the individual. Beacon is a
//! personal assistant, so its plan, entitlements, meters, and managed AI
//! (credits included) all bill the signed-in user's personal workspace.
//!
//! Access tokens carry organization ids only, so the personal workspace is
//! read from Gatekeeper's userinfo (which keeps each organization's `type`)
//! with the user's own token, then cached per user. Work that runs without a
//! token (scheduled check-ins, workflows) uses the cached workspace, and falls
//! back to the user's own account until one is known

use std::time::Duration;

use mini_moka::sync::Cache;

use crate::providers::provisioner::BillingEntity;

/// Suffix of the namespaced organizations claim (`<namespace>/organizations`)
const ORGANIZATIONS_CLAIM_SUFFIX: &str = "/claims/organizations";

/// How long a resolved personal workspace is reused. A user's personal
/// workspace never changes, so this only bounds memory
const PERSONAL_WORKSPACE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a failed lookup is remembered, so an outage or a token without the
/// organization scope does not cost a userinfo call on every request
const MISS_TTL: Duration = Duration::from_secs(5 * 60);

/// Userinfo request timeout
const USERINFO_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Resolves the workspace a user's Beacon usage bills
#[derive(Clone)]
pub struct WorkspaceResolver {
    userinfo_url: Option<String>,
    http: reqwest::Client,
    personal: Cache<String, String>,
    misses: Cache<String, ()>,
}

impl WorkspaceResolver {
    /// Create a resolver against Gatekeeper at `auth_base_url`. Without one
    /// nothing can be resolved and every user bills their own account
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
        }
    }

    /// The entity a user's usage bills, from what is already known: their
    /// personal workspace once resolved, otherwise their own account
    #[must_use]
    pub fn entity_for(&self, user_id: &str) -> BillingEntity {
        self.personal
            .get(&user_id.to_string())
            .map_or_else(|| BillingEntity::user(user_id), BillingEntity::organization)
    }

    /// Resolve (and remember) the user's personal workspace with their own
    /// token, then return the entity their usage bills. Any lookup failure
    /// bills the user's own account
    pub async fn resolve(&self, user_id: &str, token: &str) -> BillingEntity {
        let key = user_id.to_string();

        if let Some(workspace_id) = self.personal.get(&key) {
            return BillingEntity::organization(workspace_id);
        }

        if self.misses.get(&key).is_some() {
            return BillingEntity::user(user_id);
        }

        match self.fetch_personal_workspace(token).await {
            Ok(Some(workspace_id)) => {
                self.remember(user_id, &workspace_id);
                BillingEntity::organization(workspace_id)
            }
            Ok(None) => {
                tracing::warn!(
                    user_id = %user_id,
                    "no personal workspace in userinfo, billing the user's own account"
                );
                self.misses.insert(key, ());
                BillingEntity::user(user_id)
            }
            Err(e) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "personal workspace lookup failed, billing the user's own account"
                );
                self.misses.insert(key, ());
                BillingEntity::user(user_id)
            }
        }
    }

    /// Record a user's personal workspace
    pub fn remember(&self, user_id: &str, workspace_id: &str) {
        let key = user_id.to_string();
        self.misses.invalidate(&key);
        self.personal.insert(key, workspace_id.to_string());
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
    fn unknown_users_bill_their_own_account() {
        let resolver = WorkspaceResolver::new(None);
        let entity = resolver.entity_for(USER);
        assert_eq!(entity.entity_type, BillingEntityType::User);
        assert_eq!(entity.entity_id, USER);
    }

    #[test]
    fn remembered_users_bill_their_personal_workspace() {
        let resolver = WorkspaceResolver::new(None);
        resolver.remember(USER, PERSONAL);
        let entity = resolver.entity_for(USER);
        assert_eq!(entity.entity_type, BillingEntityType::Organization);
        assert_eq!(entity.entity_id, PERSONAL);
        assert_eq!(entity.kind(), "organization");
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
        assert_eq!(entity, BillingEntity::organization(PERSONAL));

        // Cached for later requests and for tokenless work
        let again = resolver.resolve(USER, "user-token").await;
        assert_eq!(again, BillingEntity::organization(PERSONAL));
        assert_eq!(
            resolver.entity_for(USER),
            BillingEntity::organization(PERSONAL)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_failed_lookup_bills_the_user_and_is_not_retried_at_once() {
        let (base, calls) = spawn_gatekeeper(StatusCode::INTERNAL_SERVER_ERROR).await;
        let resolver = WorkspaceResolver::new(Some(&base));

        assert_eq!(
            resolver.resolve(USER, "user-token").await,
            BillingEntity::user(USER)
        );
        assert_eq!(
            resolver.resolve(USER, "user-token").await,
            BillingEntity::user(USER)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
