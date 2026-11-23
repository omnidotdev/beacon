//! Card-verified free AI credits offer
//!
//! Saving a card on the account hub (never charged) unlocks a small monthly
//! AI credit allowance in the user's personal workspace. Aether reports
//! whether the offer is switched on (`GET /card-verification/offer`, public),
//! so Beacon offers it only while it is live. The offer is refreshed in the
//! background and read synchronously; anything unknown, unreachable, or
//! off-contract is no offer

use std::sync::RwLock;
use std::time::Duration;

/// Where the account hub lives when the env does not override it
pub const DEFAULT_ACCOUNT_URL: &str = "https://account.omni.dev";

/// How often the offer is re-read from Aether
const REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Offer request timeout
const OFFER_TIMEOUT: Duration = Duration::from_secs(5);

/// Monthly credits of the live offer, `None` when there is no offer
static OFFER: RwLock<Option<u64>> = RwLock::new(None);

/// Read Aether's offer body strictly: the offer is live only when `enabled`
/// is a real `true` and `monthlyCredits` a positive whole number
#[must_use]
pub fn parse_offer(body: &serde_json::Value) -> Option<u64> {
    if body.get("enabled")?.as_bool()? {
        body.get("monthlyCredits")?
            .as_u64()
            .filter(|credits| *credits > 0)
    } else {
        None
    }
}

/// Monthly credits of the live offer, or `None` when there is none
#[must_use]
pub fn current() -> Option<u64> {
    OFFER.read().ok().and_then(|offer| *offer)
}

fn set(offer: Option<u64>) {
    if let Ok(mut current) = OFFER.write() {
        *current = offer;
    }
}

/// The account hub's AI credits page, where a card unlocks the offer
#[must_use]
pub fn add_card_url() -> String {
    let base = std::env::var("BEACON_ACCOUNT_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ACCOUNT_URL.to_string());
    format!(
        "{}/billing?tab=ai&product=beacon",
        base.trim().trim_end_matches('/')
    )
}

/// The "Add a card" line for a reply while an offer of `monthly_credits` is live
#[must_use]
pub fn add_card_line(monthly_credits: u64, url: &str) -> String {
    format!("Add a card to unlock {monthly_credits} free AI credits a month: {url}")
}

async fn fetch(http: &reqwest::Client, url: &str) -> Option<u64> {
    let response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    parse_offer(&body)
}

/// Keep the offer current from Aether at `billing_base_url`, in the
/// background. A no-op outside a Tokio runtime
pub fn spawn_refresh(billing_base_url: &url::Url) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let Ok(url) = billing_base_url.join("card-verification/offer") else {
        return;
    };
    let Ok(http) = reqwest::Client::builder().timeout(OFFER_TIMEOUT).build() else {
        return;
    };

    handle.spawn(async move {
        let mut interval = tokio::time::interval(REFRESH_INTERVAL);
        loop {
            interval.tick().await;
            set(fetch(&http, url.as_str()).await);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_a_live_positive_offer() {
        assert_eq!(
            parse_offer(&serde_json::json!({ "enabled": true, "monthlyCredits": 50 })),
            Some(50)
        );
        for body in [
            serde_json::json!({ "enabled": false, "monthlyCredits": 50 }),
            serde_json::json!({ "enabled": "true", "monthlyCredits": 50 }),
            serde_json::json!({ "enabled": true, "monthlyCredits": 0 }),
            serde_json::json!({ "enabled": true, "monthlyCredits": -5 }),
            serde_json::json!({ "enabled": true, "monthlyCredits": "50" }),
            serde_json::json!({ "enabled": true }),
            serde_json::json!(null),
        ] {
            assert_eq!(parse_offer(&body), None, "{body}");
        }
    }

    #[test]
    fn the_add_card_line_names_the_monthly_credits() {
        let url = "https://account.omni.dev/billing?tab=ai&product=beacon";
        assert_eq!(
            add_card_line(50, url),
            "Add a card to unlock 50 free AI credits a month: https://account.omni.dev/billing?tab=ai&product=beacon"
        );
    }

    #[test]
    fn links_the_account_hubs_ai_tab() {
        let url = add_card_url();
        assert!(url.ends_with("/billing?tab=ai&product=beacon"));
        assert_eq!(url, url.to_lowercase());
    }
}
