---
"beacon-gateway": patch
---

Default the chat and vision model to `anthropic/claude-sonnet-5-5` (and the OpenRouter setup default to `anthropic/claude-sonnet-5.5`), replacing the retired `claude-sonnet-4-20250514` that now 404s upstream and broke managed AI. Compaction no longer sends `temperature`, which current Claude models reject, and the vision and compaction output caps leave headroom for default thinking. Local cost estimates now price each Claude generation at its own rate instead of the old family-wide rate
