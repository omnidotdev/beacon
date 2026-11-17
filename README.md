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

- Beacon bills the user's personal workspace, like every Omni product bills a workspace: its plan sets the entitlements, plan tier, and conversations meter, and its credits pay for managed AI. The gateway reads the personal workspace from Gatekeeper userinfo (`AUTH_BASE_URL`) with the user's own token and caches it per user; work that runs without a token (scheduled check-ins, workflows) uses the cached workspace and bills the user's own account until one is known
- Each user gets a product-scoped Synapse key (`appId: "beacon"`, billed to their personal workspace) provisioned through `SYNAPSE_API_URL` and cached per user, app, and billed entity
- The Synapse gateway checks credits before spend, meters the request, and deducts from the personal workspace's Omni balance. Beacon keeps no ledger, price markup, or usage meter of its own
- `GET /api/billing/plan` (signed in) returns the personal workspace's plan tier and credit balance, so the app shows what the gateway enforces
- Out of credits (402) shows an out-of-credits message linking to `BEACON_BILLING_URL` (default `https://beacon.omni.dev/settings`) to buy credits or upgrade; 503 shows a temporarily-unavailable message. Neither is retried on another model. This applies to web chat, voice, and channel replies
- There is no shared-key fallback: if a managed key cannot be provisioned, AI is reported as unavailable
- Managed models come from the Synapse gateway's Beacon model policy. `GET /api/providers/models` (signed in) returns the models the user may pick from the gateway's `GET /v1/models` on their key (ids, names, context windows, credit prices) with the default for their plan marked; a model outside the policy is refused by the gateway
- The free tier sends the gateway's virtual `auto` model, which resolves to the policy default (Claude Haiku 5.5); paid tiers default to `BEACON_LLM_MODEL` (`anthropic/claude-sonnet-5-5`). The tier comes from Aether entitlements
- Entitlements (`api_access`, `voice_enabled`) are enforced via Aether for the personal workspace when `BILLING_BASE_URL` and `BILLING_SERVICE_API_KEY` are set
- Semantic memory (embeddings via `openai/text-embedding-3-small`, fact extraction and retrieval query rewriting via `anthropic/claude-haiku-5-5`) runs through the gateway on the same per-user key, so it is charged to the personal workspace's credits like chat. A refused or failed memory call never blocks a reply: memory search falls back to keyword search and indexing is skipped
- Attachment AI (image and video keyframe descriptions, audio transcription) runs through the gateway on the same per-user key. Channel messages without an Omni identity (for example Telegram) get no attachment AI in cloud mode: images, audio, and video stay as placeholders, and PDF text extraction (local) still runs

BYOK keys and self-hosted mode are unaffected.

## Semantic memory

Memory embeddings, fact extraction, and query rewriting always go through a Synapse gateway, never a provider API directly. In cloud mode they use each user's product-scoped key (see above); a self-hosted instance uses its own gateway (`SYNAPSE_URL`) with the operator's provider keys. Without a reachable gateway, memory falls back to keyword search.

## Attachments

Image descriptions (config `[media] vision_model`, default `anthropic/claude-sonnet-5-5`), video keyframe descriptions, and audio transcription (config `[voice] stt_model`) also go only through a Synapse gateway, on the same client as memory: the user's product-scoped key in cloud mode, the operator's gateway (`SYNAPSE_URL`) when self-hosted. Beacon never calls a vision or speech-to-text provider directly. Without a gateway, attachments stay as placeholders.

## Development

```bash
cargo build
cargo test
cargo clippy
cargo run -- --foreground -v
```

`rust-toolchain.toml` pins the stable toolchain, so local builds and the pre-commit hook (`cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`) match CI. Git dependencies such as agent-core resolve from GitHub at the commits recorded in `Cargo.lock`; no sibling checkout is needed.

## Ecosystem

- **[Omni CLI](https://github.com/omnidotdev/cli)** -- Agentic CLI sharing the agent-core library
- **[Omni Terminal](https://github.com/omnidotdev/terminal)** -- GPU-accelerated terminal emulator
- **[persona.json](https://persona.omni.dev)** -- Portable entity identity spec
- **[life.json](https://life.omni.dev)** -- Portable human identity spec

## License

The code in this repository is licensed under Apache 2.0, &copy; [Omni LLC](https://omni.dev). See [LICENSE.md](LICENSE.md) for more information.
