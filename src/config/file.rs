//! TOML configuration file loading
//!
//! Supports `~/.config/omni/beacon/config.toml` as a persistent config source.
//! All fields are optional — the file is a partial overlay on top of defaults.

use std::path::PathBuf;

use serde::Deserialize;

/// Top-level TOML configuration file schema
#[derive(Debug, Default, Deserialize)]
pub struct BeaconConfigFile {
    /// Persona identifier (e.g. "orin")
    #[serde(default)]
    pub persona: Option<String>,

    /// LLM configuration
    #[serde(default)]
    pub llm: LlmFileConfig,

    /// Voice/audio configuration
    #[serde(default)]
    pub voice: VoiceFileConfig,

    /// API keys for external services
    #[serde(default)]
    pub api_keys: ApiKeysFileConfig,

    /// Channel configuration (Discord, Slack, etc)
    #[serde(default)]
    pub channels: ChannelsFileConfig,

    /// Server/runtime configuration
    #[serde(default)]
    pub server: ServerFileConfig,

    /// Skills system configuration
    #[serde(default)]
    pub skills: SkillsFileConfig,

    /// MCP server configurations
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,

    /// Path to user's life.json file
    #[serde(default)]
    pub life_json: Option<String>,

    /// Ecosystem service URLs
    #[serde(default)]
    pub ecosystem: EcosystemFileConfig,

    /// Container sandbox configuration
    #[serde(default)]
    pub sandbox: Option<SandboxFileConfig>,

    /// Media processing configuration
    #[serde(default)]
    pub media: MediaFileConfig,

    /// Multi-agent configuration
    #[serde(default)]
    pub agents: Vec<AgentFileConfig>,
}

/// Per-agent configuration in TOML
#[derive(Debug, Default, Clone, Deserialize)]
pub struct AgentFileConfig {
    /// Unique agent identifier
    pub id: String,
    /// Optional persona override
    pub persona_id: Option<String>,
    /// Optional model override
    pub model_override: Option<String>,
    /// Optional skill filter (allowlist)
    pub skill_filter: Option<Vec<String>>,
    /// Optional DM policy override ("open", "pairing", "allowlist")
    pub dm_policy_override: Option<String>,
    /// Channels this agent is enabled for
    pub enabled_channels: Option<Vec<String>>,
    /// Channel-to-agent bindings
    #[serde(default)]
    pub bindings: Vec<AgentBindingFileConfig>,
}

/// Channel binding in TOML
#[derive(Debug, Default, Clone, Deserialize)]
pub struct AgentBindingFileConfig {
    /// Channel name
    pub channel: String,
    /// Optional channel-specific account ID
    pub account_id: Option<String>,
}

/// Ecosystem service URLs
#[derive(Debug, Default, Deserialize)]
pub struct EcosystemFileConfig {
    /// Trellis knowledge garden URL
    pub trellis_url: Option<String>,
    /// Heartbeat service monitoring URL
    pub heartbeat_url: Option<String>,
    /// Say Less content moderation URL
    pub say_less_url: Option<String>,
    /// Chronicle audit logging URL
    pub chronicle_url: Option<String>,
}

/// LLM-related configuration
#[derive(Debug, Default, Deserialize)]
pub struct LlmFileConfig {
    /// Model identifier (e.g. "claude-sonnet-4-20250514")
    pub model: Option<String>,

    /// Preferred provider ("anthropic", "openai", "openrouter")
    pub provider: Option<String>,
}

/// Voice processing configuration
#[derive(Debug, Default, Deserialize)]
pub struct VoiceFileConfig {
    /// Enable voice input/output
    pub enabled: Option<bool>,

    /// STT model (e.g. "whisper-1")
    pub stt_model: Option<String>,

    /// TTS model (e.g. "tts-1")
    pub tts_model: Option<String>,

    /// TTS voice identifier (e.g. "alloy")
    pub tts_voice: Option<String>,

    /// TTS speed multiplier
    pub tts_speed: Option<f64>,
}

/// API keys configuration
#[derive(Debug, Default, Deserialize)]
pub struct ApiKeysFileConfig {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
    pub openrouter: Option<String>,
    pub elevenlabs: Option<String>,
    pub deepgram: Option<String>,
    pub discord: Option<String>,
    pub slack: Option<String>,
    pub telegram: Option<String>,
}

/// Channel-specific configuration
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ChannelsFileConfig {
    #[serde(default)]
    pub discord: Option<ChannelToggle>,

    #[serde(default)]
    pub slack: Option<ChannelToggle>,

    #[serde(default)]
    pub telegram: Option<ChannelToggle>,

    #[serde(default)]
    pub imessage: Option<IMessageFileConfig>,

    #[serde(default)]
    pub irc: IrcFileConfig,
}

/// Simple channel toggle (token lives in `api_keys`)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ChannelToggle {
    pub enabled: Option<bool>,
}

/// IRC channel configuration
#[derive(Debug, Default, Clone, Deserialize)]
pub struct IrcFileConfig {
    /// IRC server hostname
    pub server: Option<String>,
    /// IRC server port
    pub port: Option<u16>,
    /// Bot nickname
    pub nickname: Option<String>,
    /// Channels to join
    pub channels: Option<Vec<String>>,
    /// Use TLS
    pub use_tls: Option<bool>,
    /// Server password
    pub password: Option<String>,
}

/// iMessage-specific channel config
#[derive(Debug, Default, Clone, Deserialize)]
pub struct IMessageFileConfig {
    pub enabled: Option<bool>,
    pub cli_path: Option<String>,
    pub db_path: Option<String>,
    pub region: Option<String>,
    pub service: Option<String>,
}

/// Server/runtime configuration
#[derive(Debug, Default, Deserialize)]
pub struct ServerFileConfig {
    /// API server port
    pub port: Option<u16>,

    /// Synapse AI router URL
    pub synapse_url: Option<String>,

    /// Cloud mode toggle
    pub cloud_mode: Option<bool>,
}

/// Skills system configuration
#[derive(Debug, Default, Deserialize)]
pub struct SkillsFileConfig {
    /// Path to managed skills directory
    pub managed_dir: Option<String>,
    /// Max skills in prompt
    pub max_skills_in_prompt: Option<usize>,
    /// Max total chars from skills in prompt
    pub max_skills_prompt_chars: Option<usize>,
    /// Max bytes per individual skill file
    pub max_skill_file_bytes: Option<usize>,
    /// Additional skill directories to scan
    pub extra_dirs: Option<Vec<String>>,
    /// Personal agent skills directory
    pub personal_dir: Option<String>,
    /// Bundled skill allowlist (empty = all)
    pub allow_bundled: Option<Vec<String>>,
    /// Prefer Homebrew for install automation
    pub prefer_brew: Option<bool>,
    /// Node package manager preference ("npm", "pnpm", "yarn", "bun")
    pub node_manager: Option<String>,
    /// Max candidate directories to scan per root
    pub max_candidates_per_root: Option<usize>,
    /// Max skills to load per source directory
    pub max_skills_per_source: Option<usize>,
    /// Skill include patterns for agent-level filtering
    pub skill_include: Option<Vec<String>>,
    /// Skill exclude patterns for agent-level filtering
    pub skill_exclude: Option<Vec<String>>,
}

/// Container sandbox TOML configuration
#[derive(Debug, Default, Deserialize)]
pub struct SandboxFileConfig {
    /// Container engine: "docker" or "podman" (auto-detected if empty)
    pub engine: Option<String>,
    /// Container image to use
    pub image: Option<String>,
    /// Memory limit (e.g. "512m")
    pub memory_limit: Option<String>,
    /// CPU limit (e.g. "1")
    pub cpu_limit: Option<String>,
    /// Enable container networking
    pub network: Option<bool>,
    /// Timeout in seconds
    pub timeout_secs: Option<u64>,
}

/// Media processing TOML configuration
#[derive(Debug, Default, Deserialize)]
pub struct MediaFileConfig {
    /// Maximum keyframes to extract from video
    pub max_video_frames: Option<u32>,
    /// Maximum image size in bytes
    pub max_image_size_bytes: Option<usize>,
    /// Vision model for image/video analysis
    pub vision_model: Option<String>,
}

/// Load the TOML config file from the standard path
///
/// Returns `BeaconConfigFile::default()` if the file doesn't exist or can't be parsed.
pub fn load_config_file() -> BeaconConfigFile {
    let Some(path) = config_file_path() else {
        return BeaconConfigFile::default();
    };

    if !path.exists() {
        return BeaconConfigFile::default();
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => {
                tracing::info!(path = %path.display(), "loaded config file");
                config
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to parse config file, using defaults"
                );
                BeaconConfigFile::default()
            }
        },
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to read config file"
            );
            BeaconConfigFile::default()
        }
    }
}

/// Return the config file path: `~/.config/omni/beacon/config.toml`
#[must_use]
pub fn config_file_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| {
        d.config_dir()
            .join("omni")
            .join("beacon")
            .join("config.toml")
    })
}
