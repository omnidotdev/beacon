---
"beacon-gateway": minor
---

Run managed AI on Omni credits through a product-scoped Synapse key. Cloud-mode users get a managed key provisioned with `appId: "beacon"` and billed to the user, cached per user, app, and billed entity; the Synapse gateway checks, meters, and deducts credits from the pooled Omni balance, and there is no shared-key fallback. A 402 from the gateway shows an out-of-credits message with a buy-credits/upgrade link and a 503 shows a temporarily-unavailable message across web chat, voice, scheduled runs, and channel replies. The free tier defaults to `anthropic/claude-haiku-5-5` while paid tiers keep `anthropic/claude-sonnet-5-5`, based on the Aether tier. Billing now reads the fleet-standard `BILLING_BASE_URL` / `BILLING_SERVICE_API_KEY` (legacy names still accepted) and always uses the `beacon` Aether app, so entitlement checks run in production. The per-month message quota, the requests meter, and Beacon's own Aether usage recording are removed
