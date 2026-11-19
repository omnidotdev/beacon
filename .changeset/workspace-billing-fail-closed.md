---
"beacon-gateway": minor
---

Never bill the user's own account for managed AI. Beacon bills only the user's personal workspace: until it is known, chat, voice, and the model list refuse with `reauth_required` ("Sign in to Beacon again to keep using AI.") and nothing is charged. Scheduled check-ins and workflows, which run without the user's token, now resolve the personal workspace from Gatekeeper by user id (`GATEKEEPER_SERVICE_KEY`), so they keep billing it after a restart. Beacon's database only caches it. While the card-verified free AI credits offer is live, out-of-credits and paid-access replies also link to the account hub's AI tab to add a card for free monthly credits (`BEACON_ACCOUNT_URL`, default `https://account.omni.dev`).
