---
"beacon-gateway": minor
---

Take managed models from the Synapse gateway's Beacon policy

- `GET /api/providers/models` lists the managed models the signed-in user may pick, from the gateway's `GET /v1/models` on the user's product-scoped key: ids, display names, context windows, retail credit prices, and the default for the user's plan (the gateway default on the free tier, the configured default on paid tiers when the gateway lists it). Empty outside cloud mode
- The free tier sends the gateway's virtual `auto` model instead of a pinned Haiku id, so the gateway's Beacon policy decides the free default
- Provider descriptions no longer advertise smart routing or retired model names
