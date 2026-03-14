//! SSRF (Server-Side Request Forgery) protection
//!
//! Validates URLs before fetching to prevent access to internal networks

use std::net::IpAddr;

use crate::{Error, Result};

/// Validate that a URL is safe to fetch (not targeting internal networks)
///
/// # Errors
///
/// Returns error if the URL targets a private/reserved IP range or uses a non-HTTP scheme
pub async fn validate_url(url: &str, allowed_internal_hosts: &[String]) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|e| Error::Link(format!("invalid URL: {e}")))?;

    // Block non-HTTP schemes
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(Error::Link(format!("blocked non-HTTP scheme: {scheme}")));
        }
    }

    let Some(host) = parsed.host_str() else {
        return Err(Error::Link("URL has no host".to_string()));
    };

    // Check allowlist for internal services
    if allowed_internal_hosts.iter().any(|h| h == host) {
        return Ok(());
    }

    // Check if host is a literal IP
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(Error::Link(format!("blocked private IP: {ip}")));
        }
        return Ok(());
    }

    // DNS resolution check — resolve hostname and verify it doesn't resolve to a private IP
    let addrs = tokio::net::lookup_host(format!("{host}:{}", parsed.port().unwrap_or(80)))
        .await
        .map_err(|e| Error::Link(format!("DNS resolution failed for {host}: {e}")))?;

    for addr in addrs {
        if is_private_ip(&addr.ip()) {
            return Err(Error::Link(format!(
                "host {host} resolves to private IP: {}",
                addr.ip()
            )));
        }
    }

    Ok(())
}

/// Check if an IP address is in a private or reserved range
const fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()                          // 127.0.0.0/8
                || v4.is_private()                    // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()                 // 169.254.0.0/16
                || v4.is_unspecified()                // 0.0.0.0
                || v4.is_broadcast()                  // 255.255.255.255
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGNAT)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()                          // ::1
                || v6.is_unspecified()                // ::
                || {
                    let segments = v6.segments();
                    (segments[0] & 0xFE00) == 0xFC00  // fc00::/7 (ULA)
                        || (segments[0] == 0xFE80)    // fe80::/10 (link-local)
                }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ipv4_addresses() {
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(is_private_ip(&"169.254.1.1".parse().unwrap()));
        assert!(is_private_ip(&"0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn public_ipv4_addresses() {
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip(&"93.184.216.34".parse().unwrap()));
    }

    #[test]
    fn private_ipv6_addresses() {
        assert!(is_private_ip(&"::1".parse().unwrap()));
        assert!(is_private_ip(&"::".parse().unwrap()));
        assert!(is_private_ip(&"fc00::1".parse().unwrap()));
        assert!(is_private_ip(&"fd00::1".parse().unwrap()));
        assert!(is_private_ip(&"fe80::1".parse().unwrap()));
    }

    #[tokio::test]
    async fn blocks_non_http_scheme() {
        let result = validate_url("file:///etc/passwd", &[]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("non-HTTP"));
    }

    #[tokio::test]
    async fn blocks_private_ip_literal() {
        let result = validate_url("http://127.0.0.1/admin", &[]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("private IP"));
    }

    #[tokio::test]
    async fn allows_allowlisted_host() {
        let allowed = vec!["trellis.internal".to_string()];
        let result = validate_url("http://trellis.internal/api", &allowed).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn blocks_missing_host() {
        let result = validate_url("http://", &[]).await;
        // url::Url::parse may or may not parse this - either way should not allow
        assert!(result.is_err());
    }
}
