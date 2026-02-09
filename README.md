<div align="center">

# Beacon

Voice and messaging gateway for AI assistants

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE.md)

</div>

## Overview

Beacon provides the infrastructure for AI assistants to communicate via voice and messaging channels. It powers [Orin](https://orin.omni.dev), Omni's friendly otter assistant, and supports custom [personas](https://persona.omni.dev).

## Features

- **Always-on daemon** - Background service ready to respond
- **Wake word detection** - Local processing for instant activation
- **Hybrid voice** - Local wake word + cloud STT/TTS (BYOK)
- **Multi-channel messaging** - Discord, Slack, WhatsApp, Telegram, Signal, Teams, Matrix, Google Chat, iMessage
- **Configurable personas** - Custom wake words, voices, and personalities via [persona.json](https://persona.omni.dev)
- **Cross-platform** - Web, desktop, and mobile via Tauri
- **BYOK** - Bring your own API keys for LLM and voice providers

## Architecture

```
Interfaces (Voice, Discord, Slack, WhatsApp, ...)
                    |
            Beacon Gateway
    (Daemon, Wake Word, STT/TTS, Channels)
                    |
           Omni CLI (Agent Core)
```

## Services

| Service | Description |
|---------|-------------|
| [beacon-gateway](https://github.com/omnidotdev/beacon-gateway) | Core Rust gateway daemon |
| [beacon-api](https://github.com/omnidotdev/beacon-api) | GraphQL API (Elysia) |
| [beacon-app](https://github.com/omnidotdev/beacon-app) | Cross-platform app (React + Tauri) |

## Getting Started

1. Copy configuration templates:

   ```sh
   cp services.yaml.template services.yaml
   cp .env.local.template .env.local
   ```

2. Edit `.env.local` with your API keys

3. Start development:

   ```sh
   tilt up
   ```

## Self-Hosting

Each service includes a Dockerfile for containerized deployment:

```sh
# Build and run the gateway
docker build -t beacon-gateway services/beacon-gateway
docker run beacon-gateway

# Build and run the app
docker build -t beacon-app services/beacon-app
docker run -p 3000:3000 beacon-app
```

See `.env.local.template` for all configuration options.

## Personas

Default persona configurations are in `personas/`. [Orin](https://orin.omni.dev) is the flagship:

```json
{
  "identity": {
    "id": "orin",
    "name": "Orin",
    "tagline": "Your friendly otter guide"
  },
  "voice": {
    "wakeWords": ["hey orin"]
  }
}
```

## Documentation

- [Beacon Docs](https://omni.dev/docs/grid/beacon)
- [persona.json Spec](https://persona.omni.dev)
- [life.json Spec](https://life.omni.dev)

## License

The code in this repository is licensed under MIT, &copy; [Omni LLC](https://omni.dev). See [LICENSE.md](LICENSE.md) for more information.
