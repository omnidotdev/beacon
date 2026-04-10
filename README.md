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
