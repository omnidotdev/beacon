---
"beacon-gateway": minor
---

Run attachment AI (image and video keyframe descriptions, audio transcription) only on the turn's Synapse gateway client: the user's product-scoped Beacon key in cloud mode, the operator's gateway when self-hosted. The direct Anthropic vision fallback and the unused direct OpenAI, Gemini, Deepgram, and Whisper media providers are removed, so no media path calls a provider directly. Cloud channel messages without an Omni identity keep media as placeholders instead of running unbilled AI
