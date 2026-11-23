//! JWT validation for Gatekeeper tokens

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::RwLock;

/// Claims extracted from the JWT
#[derive(Debug, Deserialize)]
pub struct GatekeeperClaims {
    pub sub: String,
    pub exp: u64,
    pub iss: Option<String>,
}

/// Audience Gatekeeper mints into Beacon's access tokens (the beacon-app OAuth
/// client's RFC 8707 resource), overridable with `BEACON_JWT_AUDIENCE`
pub const DEFAULT_JWT_AUDIENCE: &str = "https://gateway.beacon.omni.dev";

/// Token validation rules for Beacon access tokens
///
/// Checks the signature algorithm, expiry, and that the token was issued by our
/// identity service for Beacon. Without the audience check, a token Gatekeeper
/// minted for any other Omni product would be accepted here
#[must_use]
pub fn token_validation(alg: Algorithm, issuer: &str, audience: &str) -> Validation {
    let mut validation = Validation::new(alg);
    validation.validate_exp = true;
    validation.set_issuer(&[issuer.trim_end_matches('/')]);
    validation.set_audience(&[audience]);
    validation
}

/// Cached JWKS for validating JWTs
pub struct JwksCache {
    auth_base_url: String,
    audience: String,
    client: reqwest::Client,
    keys: Arc<RwLock<Option<CachedJwks>>>,
    jwks_uri: Arc<RwLock<Option<String>>>,
}

struct CachedJwks {
    keys: Vec<jsonwebtoken::jwk::Jwk>,
    expires_at: Instant,
}

/// OIDC discovery document (partial)
#[derive(Deserialize)]
struct OidcDiscovery {
    jwks_uri: Option<String>,
}

impl JwksCache {
    #[must_use]
    pub fn new(auth_base_url: String) -> Self {
        Self {
            auth_base_url,
            audience: std::env::var("BEACON_JWT_AUDIENCE")
                .ok()
                .filter(|aud| !aud.is_empty())
                .unwrap_or_else(|| DEFAULT_JWT_AUDIENCE.to_string()),
            client: reqwest::Client::new(),
            keys: Arc::new(RwLock::new(None)),
            jwks_uri: Arc::new(RwLock::new(None)),
        }
    }

    /// Validate a JWT and return the claims
    ///
    /// # Errors
    ///
    /// Returns an error string if the token is invalid or no matching key is found
    pub async fn validate(&self, token: &str) -> Result<GatekeeperClaims, String> {
        let jwks = self.get_jwks().await?;

        // Log JWT header for diagnostics
        let header = decode_header(token).map_err(|e| format!("invalid JWT header: {e}"))?;
        tracing::debug!(
            alg = ?header.alg,
            kid = ?header.kid,
            jwks_count = jwks.len(),
            "validating JWT"
        );

        let mut last_error = None;

        // Try each key until one works (key rotation support)
        for jwk in &jwks {
            let key = match DecodingKey::from_jwk(jwk) {
                Ok(k) => k,
                Err(e) => {
                    tracing::debug!(
                        jwk_kid = ?jwk.common.key_id,
                        jwk_alg = ?jwk.common.key_algorithm,
                        error = %e,
                        "skipping JWK: failed to create decoding key"
                    );
                    continue;
                }
            };

            // jsonwebtoken 9.x requires validation.algorithms to contain ONLY
            // algorithms matching the key's family (it checks every listed alg
            // against the key). Use the JWT header's algorithm directly
            let validation = token_validation(header.alg, &self.auth_base_url, &self.audience);

            match decode::<GatekeeperClaims>(token, &key, &validation) {
                Ok(data) => return Ok(data.claims),
                Err(e) => {
                    tracing::debug!(
                        jwk_kid = ?jwk.common.key_id,
                        error = %e,
                        "JWK did not validate token"
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(format!(
            "no valid key found for JWT (alg={:?}, kid={:?}, jwks_keys={}, last_error={:?})",
            header.alg,
            header.kid,
            jwks.len(),
            last_error,
        ))
    }

    /// Resolve the JWKS URI from OIDC discovery (cached)
    async fn resolve_jwks_uri(&self) -> Result<String, String> {
        // Check cache
        {
            let cached = self.jwks_uri.read().await;
            if let Some(uri) = cached.as_ref() {
                return Ok(uri.clone());
            }
        }

        // Try OIDC discovery first
        let discovery_url = format!("{}/.well-known/openid-configuration", self.auth_base_url);

        let jwks_url = match self.client.get(&discovery_url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<OidcDiscovery>().await {
                Ok(doc) if doc.jwks_uri.is_some() => {
                    let uri = doc.jwks_uri.unwrap();
                    tracing::info!(
                        jwks_uri = %uri,
                        "resolved JWKS URI from OIDC discovery"
                    );
                    uri
                }
                _ => {
                    let fallback = format!("{}/.well-known/jwks.json", self.auth_base_url);
                    tracing::debug!(
                        fallback = %fallback,
                        "OIDC discovery missing jwks_uri, using fallback"
                    );
                    fallback
                }
            },
            _ => {
                let fallback = format!("{}/.well-known/jwks.json", self.auth_base_url);
                tracing::debug!(
                    fallback = %fallback,
                    "OIDC discovery unavailable, using fallback"
                );
                fallback
            }
        };

        *self.jwks_uri.write().await = Some(jwks_url.clone());

        Ok(jwks_url)
    }

    /// Fetch JWKS from identity provider (cached for 1 hour)
    async fn get_jwks(&self) -> Result<Vec<jsonwebtoken::jwk::Jwk>, String> {
        // Check cache
        {
            let cache = self.keys.read().await;
            if let Some(cached) = cache.as_ref()
                && cached.expires_at > Instant::now()
            {
                return Ok(cached.keys.clone());
            }
        }

        // Resolve JWKS URI from OIDC discovery
        let url = self.resolve_jwks_uri().await?;

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("failed to fetch JWKS from {url}: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "JWKS endpoint returned {}: {url}",
                response.status()
            ));
        }

        let jwk_set: jsonwebtoken::jwk::JwkSet = response
            .json()
            .await
            .map_err(|e| format!("invalid JWKS response from {url}: {e}"))?;

        tracing::debug!(
            url = %url,
            key_count = jwk_set.keys.len(),
            "fetched JWKS"
        );

        // Cache for 1 hour
        let keys = jwk_set.keys;
        *self.keys.write().await = Some(CachedJwks {
            keys: keys.clone(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        });

        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    use super::*;

    const SECRET: &[u8] = b"test-secret";
    const ISSUER: &str = "https://identity.omni.dev";

    fn token(iss: &str, aud: &serde_json::Value) -> String {
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 600;
        encode(
            &Header::new(Algorithm::HS256),
            &json!({ "sub": "user-1", "exp": exp, "iss": iss, "aud": aud }),
            &EncodingKey::from_secret(SECRET),
        )
        .unwrap()
    }

    fn check(token: &str) -> bool {
        let validation = token_validation(
            Algorithm::HS256,
            "https://identity.omni.dev/",
            DEFAULT_JWT_AUDIENCE,
        );
        decode::<GatekeeperClaims>(token, &DecodingKey::from_secret(SECRET), &validation).is_ok()
    }

    #[test]
    fn accepts_beacon_tokens() {
        assert!(check(&token(ISSUER, &json!(DEFAULT_JWT_AUDIENCE))));
        assert!(check(&token(
            ISSUER,
            &json!([
                "https://identity.omni.dev/oauth2/userinfo",
                DEFAULT_JWT_AUDIENCE
            ])
        )));
    }

    #[test]
    fn rejects_tokens_minted_for_other_products() {
        assert!(!check(&token(
            ISSUER,
            &json!("https://api.keystone.omni.dev")
        )));
    }

    #[test]
    fn rejects_tokens_from_other_issuers() {
        assert!(!check(&token(
            "https://evil.example",
            &json!(DEFAULT_JWT_AUDIENCE)
        )));
    }
}
