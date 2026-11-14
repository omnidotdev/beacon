<div align="center">

# Beacon

Voice and messaging gateway for AI assistants

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE.md)

[Website](https://beacon.omni.dev) | [Discord](https://discord.gg/omnidotdev) | [GitHub](https://github.com/omnidotdev/beacon)

</div>

## Overview

Beacon is a Rust daemon that connects AI assistants to voice and 13+ messaging platforms through a single gateway. It handles wake word detection, speech processing, multi-channel messaging, persona management, persistent memory, and tool execution -- all local-first with BYOK provider keys.

## Installation

### From source (current)

```bash
cargo install --path .
```

Requires [Rust](https://rustup.rs) 1.88+.

### Via Omni CLI (coming soon)

```bash
omni install beacon
```

The [Omni CLI](https://github.com/omnidotdev/cli) will be the recommended way to install and manage Beacon once available.

## Quick start

### 1. Configure

```bash
# Interactive setup (recommended)
beacon setup

# Or manually: copy the template and add your API keys
cp .env.local.template .env.local
```

At minimum you need one AI provider key (`ANTHROPIC_API_KEY` or `OPENAI_API_KEY`).

### 2. Run

```bash
# Voice + messaging
beacon --foreground -v

# Messaging only (headless)
beacon --disable-voice
```

The gateway starts on `http://localhost:18789` and connects to any configured channels.

### 3. Diagnostics

```bash
beacon doctor    # Health check
beacon status    # Service status
```

## Cloud mode and Omni credits

With `BEACON_CLOUD_MODE=true` (the hosted deployment), Beacon requires a Gatekeeper JWT and runs managed AI on Omni credits:

- Each user gets a product-scoped Synapse key (`appId: "beacon"`, billed to the user) provisioned through `SYNAPSE_API_URL` and cached per user, app, and billed entity
- The Synapse gateway checks credits before spend, meters the request, and deducts from the user's pooled Omni balance (one balance shared across Omni products). Beacon keeps no ledger, price markup, or usage meter of its own
- Out of credits (402) shows an out-of-credits message linking to `BEACON_BILLING_URL` (default `https://beacon.omni.dev/settings`) to buy credits or upgrade; 503 shows a temporarily-unavailable message. Neither is retried on another model. This applies to web chat, voice, and channel replies
- There is no shared-key fallback: if a managed key cannot be provisioned, AI is reported as unavailable
- The free tier defaults to `anthropic/claude-haiku-5-5`; paid tiers default to `BEACON_LLM_MODEL` (`anthropic/claude-sonnet-5-5`). The tier comes from Aether entitlements
- Entitlements (`api_access`, `voice_enabled`) are enforced via Aether when `BILLING_BASE_URL` and `BILLING_SERVICE_API_KEY` are set

BYOK keys and self-hosted mode are unaffected.

## Development

```bash
cargo build
cargo test
cargo clippy
cargo run -- --foreground -v
```

## Ecosystem

- **[Omni CLI](https://github.com/omnidotdev/cli)** -- Agentic CLI sharing the agent-core library
- **[Omni Terminal](https://github.com/omnidotdev/terminal)** -- GPU-accelerated terminal emulator
- **[persona.json](https://persona.omni.dev)** -- Portable entity identity spec
- **[life.json](https://life.omni.dev)** -- Portable human identity spec

## License

The code in this repository is licensed under Apache 2.0, &copy; [Omni LLC](https://omni.dev). See [LICENSE.md](LICENSE.md) for more information.
