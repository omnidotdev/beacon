---
"beacon-gateway": minor
---

Lighting the beacon

- Multi-channel AI assistant gateway — voice, Discord, Telegram, Slack, and WebSocket
- Built-in sandboxed shell execution tool with timeout and PATH augmentation
- Skill management with compact prompt injection and on-demand skill reading
- Per-channel tool policy enforcement
- Memory tools for persistent, session-spanning context
- Cron scheduling for automated tasks
- Plugin extensibility via MCP
- Direct MCP server management over stdio transport with `[[mcp_servers]]` TOML config
- MCP tools automatically registered in `ToolExecutor` alongside Synapse/plugin tools
- Plugin MCP configs (`transport: "mcp-stdio"`) auto-spawned at daemon startup
- `require_auth` middleware supporting both API key and Gatekeeper JWT
- Trellis knowledge garden API client with `[ecosystem]` config section
- Browser automation tools: navigate, click, type, screenshot, extract
- BM25 keyword scorer for ranked memory search with hybrid vector+keyword matching
- 10 bundled skills: summarize, translate, code-review, explain, meeting-notes, proofread, data-analysis, email-draft, debug, and default
- `beacon setup` wizard with channel configuration, MCP server discovery, and life.json setup
- Consolidated shared modules into agent-core (loop detection, web fetch/search/readability, tool policy, skill types)
- Multi-agent support: per-agent config (model override, persona, skill filter, DM policy), binding router for channel-to-agent routing, and agent-isolated sessions
- Config hot-reload: watch config.toml for changes, classify and apply hot-reloadable sections without restart
- IRC channel adapter for standard IRC protocol support
- Gmail/email channel adapter with OAuth2 service account auth and polling
- Extended hook lifecycle events: `session:created`, `session:ended`, `channel:connected`, `channel:disconnected`
- PDF text extraction for message attachments via pdf-extract
- Twilio SMS/voice channel adapter for phone calling
- Feishu (Lark) channel adapter for enterprise messaging
- Line messaging channel adapter for APAC coverage
- Rule-based message routing and gating system with channel/sender/content filtering
- Local in-process cron scheduler via croner (fallback when Vortex unavailable)
- Extensible hook events with custom namespaces, wildcard matching, and `POST /api/hooks/emit` endpoint
- Multi-provider vision processing with ordered fallback (Synapse, OpenAI, Gemini)
- Multi-provider STT with ordered fallback (Synapse, Whisper, Deepgram)
- Harness adapter system for delegating agent execution to external CLI tools (Claude Code CLI) with session persistence, cost tracking, and MCP integration
