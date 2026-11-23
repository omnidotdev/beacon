//! Authentication middleware
//!
//! Supports two authentication methods:
//! 1. API key (simple Bearer token matching `BEACON_API_KEY`)
//! 2. Gatekeeper JWT (validated via JWKS endpoint)
//!
//! In development mode (no API key configured, no JWKS), all requests pass through

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};

use super::ApiState;

/// Authenticated user identity extracted from the request
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuthIdentity {
    /// User ID (from JWT `sub` claim or "api-key" for API key auth)
    pub user_id: String,
    /// Authentication method used
    pub method: AuthMethod,
}

/// How the user was authenticated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AuthMethod {
    /// Matched the configured `BEACON_API_KEY`
    ApiKey,
    /// Validated via Gatekeeper JWT
    Jwt,
    /// No auth configured (development mode)
    Anonymous,
}

/// Extract Bearer token from Authorization header
fn extract_bearer(req: &Request) -> Option<&str> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Who may call a route on a cloud (multi-user) gateway
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteAccess {
    /// Anyone: health, public persona info, signature-verified webhooks, and
    /// the chat socket (which authenticates its own connection)
    Public,
    /// A signed-in user (Gatekeeper JWT) or the operator API key
    User,
    /// The operator API key only. Gateway-wide data and host control (admin,
    /// memories, usage, browser, plugins, skill installs) must never be
    /// reachable by an end user
    Operator,
}

/// Whether `path` is `prefix` itself or nested under it
fn under(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Classify a request for the cloud route guard
#[must_use]
pub fn classify_route(method: &Method, path: &str) -> RouteAccess {
    const OPERATOR: &[&str] = &[
        "/api/admin",
        "/api/memories",
        "/api/knowledge",
        "/api/usage",
        "/api/browser",
        "/api/nodes",
        "/api/plugins",
        "/api/canvas",
        "/api/hooks",
        "/ws/node",
        "/ws/canvas",
    ];

    if OPERATOR.iter().any(|prefix| under(path, prefix)) {
        return RouteAccess::Operator;
    }

    // Skills are gateway-wide and installing one can run package managers on
    // the host, so users may only read them
    if under(path, "/api/skills") {
        return if method == Method::GET {
            RouteAccess::User
        } else {
            RouteAccess::Operator
        };
    }

    if under(path, "/api/personas/marketplace") {
        return RouteAccess::User;
    }

    let public = matches!(
        path,
        "/health"
            | "/ready"
            | "/api/status"
            | "/api/persona"
            | "/api/personas"
            | "/api/pair/gateway"
    ) || (path.starts_with("/api/personas/") && path.ends_with("/activate"))
        || under(path, "/api/webhooks")
        || under(path, "/ws/chat");

    if public || !(under(path, "/api") || under(path, "/ws")) {
        return RouteAccess::Public;
    }

    RouteAccess::User
}

/// Cloud route guard: enforce [`classify_route`] on every request
///
/// A cloud gateway serves many users from one process, so per-router auth
/// that passes everything through when `BEACON_API_KEY` is unset (the
/// self-hosted development default) would expose every user's data and the
/// host itself. In cloud mode this guard fails closed instead: operator
/// routes need the API key and are refused outright when none is configured.
/// Self-hosted gateways pass through unchanged
///
/// # Errors
///
/// Returns `401 Unauthorized` when a cloud request lacks the credentials its
/// route requires
pub async fn cloud_route_guard(
    State(state): State<Arc<ApiState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !state.cloud_mode {
        return Ok(next.run(req).await);
    }

    let access = classify_route(req.method(), req.uri().path());
    if access == RouteAccess::Public {
        return Ok(next.run(req).await);
    }

    let token = extract_bearer(&req).map(str::to_owned);

    if let (Some(expected), Some(token)) = (&state.api_key, &token)
        && token == expected
    {
        req.extensions_mut().insert(AuthIdentity {
            user_id: "api-key".to_string(),
            method: AuthMethod::ApiKey,
        });
        return Ok(next.run(req).await);
    }

    if access == RouteAccess::User
        && let (Some(jwt_cache), Some(token)) = (&state.jwt_cache, &token)
        && let Ok(claims) = jwt_cache.validate(token).await
    {
        req.extensions_mut().insert(AuthIdentity {
            user_id: claims.sub,
            method: AuthMethod::Jwt,
        });
        return Ok(next.run(req).await);
    }

    tracing::debug!(path = %req.uri().path(), ?access, "cloud route guard refused request");
    Err(StatusCode::UNAUTHORIZED)
}

/// Middleware to verify API key (admin endpoints)
///
/// # Errors
///
/// Returns `401 Unauthorized` when a key is configured and the request does
/// not carry it
pub async fn require_api_key(
    State(state): State<Arc<ApiState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // The cloud route guard already authenticated this request
    if req.extensions().get::<AuthIdentity>().is_some() {
        return Ok(next.run(req).await);
    }

    // If no API key configured, allow all requests (development mode)
    let Some(expected_key) = &state.api_key else {
        tracing::warn!("API key not configured - allowing unauthenticated access");
        return Ok(next.run(req).await);
    };

    let provided_key = extract_bearer(&req);

    match provided_key {
        Some(key) if key == expected_key => Ok(next.run(req).await),
        Some(_) => {
            tracing::warn!("invalid API key provided");
            Err(StatusCode::UNAUTHORIZED)
        }
        None => {
            tracing::debug!("no API key provided");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Middleware that accepts either API key or Gatekeeper JWT
///
/// On success, inserts `AuthIdentity` into request extensions.
/// In development mode (no auth configured), passes through as anonymous.
#[allow(dead_code)]
///
/// # Errors
///
/// Returns `401 Unauthorized` when auth is configured and the request carries
/// no valid API key or JWT
pub async fn require_auth(
    State(state): State<Arc<ApiState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = extract_bearer(&req);

    // Try API key first
    if let Some(expected_key) = &state.api_key
        && let Some(key) = token
        && key == expected_key
    {
        req.extensions_mut().insert(AuthIdentity {
            user_id: "api-key".to_string(),
            method: AuthMethod::ApiKey,
        });
        return Ok(next.run(req).await);
    }

    // Try JWT validation via HIDRA Gatekeeper
    if let Some(ref jwt_cache) = state.jwt_cache
        && let Some(token) = token
    {
        match jwt_cache.validate(token).await {
            Ok(claims) => {
                tracing::debug!(user_id = %claims.sub, "authenticated via Gatekeeper JWT");
                req.extensions_mut().insert(AuthIdentity {
                    user_id: claims.sub,
                    method: AuthMethod::Jwt,
                });
                return Ok(next.run(req).await);
            }
            Err(e) => {
                tracing::debug!(error = %e, "JWT validation failed");
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    // Development mode: no API key and no JWKS configured
    if state.api_key.is_none() && state.jwt_cache.is_none() {
        req.extensions_mut().insert(AuthIdentity {
            user_id: "anonymous".to_string(),
            method: AuthMethod::Anonymous,
        });
        return Ok(next.run(req).await);
    }

    // Auth is configured but no valid credentials provided
    tracing::debug!("no valid credentials provided");
    Err(StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn classifies_operator_routes() {
        for path in [
            "/api/admin/sessions",
            "/api/admin/sessions/abc/messages",
            "/api/memories",
            "/api/usage",
            "/api/browser/navigate",
            "/api/plugins",
            "/api/canvas",
            "/api/hooks/emit",
            "/ws/node",
            "/ws/canvas",
        ] {
            assert_eq!(
                classify_route(&Method::GET, path),
                RouteAccess::Operator,
                "{path}"
            );
        }
    }

    #[test]
    fn skills_are_read_only_for_users() {
        assert_eq!(
            classify_route(&Method::GET, "/api/skills"),
            RouteAccess::User
        );
        assert_eq!(
            classify_route(&Method::GET, "/api/skills/search"),
            RouteAccess::User
        );
        for (method, path) in [
            (Method::POST, "/api/skills/install"),
            (Method::POST, "/api/skills/install/local"),
            (Method::POST, "/api/skills/abc/install-deps"),
            (Method::DELETE, "/api/skills/abc"),
            (Method::PATCH, "/api/skills/abc/enabled"),
        ] {
            assert_eq!(
                classify_route(&method, path),
                RouteAccess::Operator,
                "{path}"
            );
        }
    }

    #[test]
    fn classifies_public_and_user_routes() {
        for path in [
            "/health",
            "/ready",
            "/api/status",
            "/api/persona",
            "/api/personas",
            "/api/pair/gateway",
            "/api/webhooks/telegram",
            "/ws/chat/web-123",
            "/",
            "/assets/app.js",
        ] {
            assert_eq!(
                classify_route(&Method::GET, path),
                RouteAccess::Public,
                "{path}"
            );
        }
        assert_eq!(
            classify_route(&Method::POST, "/api/personas/microcap/activate"),
            RouteAccess::Public
        );
        for path in [
            "/api/providers",
            "/api/billing/plan",
            "/api/voice/capabilities",
            "/api/personas/marketplace",
            "/api/personas/marketplace/search",
        ] {
            assert_eq!(
                classify_route(&Method::GET, path),
                RouteAccess::User,
                "{path}"
            );
        }
    }

    #[test]
    fn prefix_match_respects_segment_boundaries() {
        assert_eq!(
            classify_route(&Method::GET, "/api/usage-report"),
            RouteAccess::User
        );
        assert_eq!(
            classify_route(&Method::GET, "/api/adminx"),
            RouteAccess::User
        );
    }

    #[test]
    fn extracts_bearer_token() {
        let mut req = Request::builder().body(Body::empty()).unwrap();

        // No header
        assert_eq!(extract_bearer(&req), None);

        // With Bearer token
        req.headers_mut().insert(
            "authorization",
            HeaderValue::from_static("Bearer test-key-123"),
        );
        assert_eq!(extract_bearer(&req), Some("test-key-123"));
    }

    #[test]
    fn auth_identity_methods() {
        let api_key = AuthIdentity {
            user_id: "api-key".to_string(),
            method: AuthMethod::ApiKey,
        };
        assert_eq!(api_key.method, AuthMethod::ApiKey);

        let jwt = AuthIdentity {
            user_id: "user-123".to_string(),
            method: AuthMethod::Jwt,
        };
        assert_eq!(jwt.method, AuthMethod::Jwt);
    }
}
