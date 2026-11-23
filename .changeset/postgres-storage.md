---
"beacon-gateway": minor
---

Store gateway data in Postgres when `BEACON_DATABASE_URL` is set, so the hosted gateway keeps sessions, memories, and settings across redeploys. Self-hosted gateways keep using a local SQLite file and upgrade in place. Memory search uses pgvector on Postgres and sqlite-vec on SQLite. Also fixes memory recency weighting (accessed memories now record when they were used), keeps simultaneous first requests from racing into duplicate users, and makes deleting a user remove their sessions, memories, and context
