//! Link processing configuration

use serde::{Deserialize, Serialize};

/// Configuration for link processing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkConfig {
    /// Enable link preview extraction
    pub enabled: bool,
    /// Maximum URLs to process per message
    pub max_urls: usize,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Enable full page content extraction (beyond OG metadata)
    pub extract_content: bool,
    /// Max chars to extract from page content
    pub max_content_length: usize,
    /// Hosts allowed to bypass SSRF checks (e.g. internal services like Trellis)
    pub allowed_internal_hosts: Vec<String>,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_urls: 3,
            timeout_secs: 10,
            extract_content: false,
            max_content_length: 4000,
            allowed_internal_hosts: Vec::new(),
        }
    }
}
