---
"beacon-gateway": minor
---

Run semantic memory (embeddings, fact extraction, retrieval query rewriting) through the Synapse gateway instead of a raw `OPENAI_API_KEY`. In cloud mode it uses each user's product-scoped Beacon key, so memory AI is charged to the user's Omni credits like chat; self-hosted instances use their own gateway. Refused or failed memory calls degrade to keyword search and skip indexing, and are never retried
