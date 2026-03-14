//! WASM plugin runtime via Extism
//!
//! Provides sandboxed execution of WebAssembly plugins from Manifold artifacts.
//! No filesystem access, no network access, configurable memory limits.

#[cfg(feature = "wasm")]
use extism::{Manifest as ExtismManifest, Plugin, Wasm};

use crate::{Error, Result};

/// A loaded WASM plugin instance
#[cfg(feature = "wasm")]
pub struct WasmPlugin {
    plugin: Plugin,
    name: String,
}

#[cfg(feature = "wasm")]
impl WasmPlugin {
    /// Load a WASM plugin from raw bytes
    ///
    /// # Errors
    ///
    /// Returns error if WASM compilation or instantiation fails
    pub fn load(
        name: String,
        wasm_bytes: Vec<u8>,
        memory_limit_pages: Option<u32>,
    ) -> Result<Self> {
        let wasm = Wasm::data(wasm_bytes);
        let mut manifest = ExtismManifest::new([wasm]);

        // Apply memory limit (each page = 64KB)
        if let Some(pages) = memory_limit_pages {
            manifest = manifest.with_memory_max(pages);
        }

        // No allowed hosts = no network access
        // No allowed paths = no filesystem access
        let plugin = Plugin::new(&manifest, [], true)
            .map_err(|e| Error::Tool(format!("failed to load WASM plugin '{name}': {e}")))?;

        tracing::info!(name = %name, "loaded WASM plugin");
        Ok(Self { plugin, name })
    }

    /// Call a tool function in the WASM plugin
    ///
    /// The plugin should export a function with the given name that accepts
    /// JSON input and returns a JSON string.
    ///
    /// # Errors
    ///
    /// Returns error if the function call fails
    pub fn call_tool(&mut self, tool_name: &str, input: &str) -> Result<String> {
        let output = self
            .plugin
            .call::<&str, String>(tool_name, input)
            .map_err(|e| {
                Error::Tool(format!(
                    "WASM plugin '{}' tool '{tool_name}' failed: {e}",
                    self.name
                ))
            })?;

        Ok(output)
    }

    /// Get the plugin name
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(feature = "wasm")]
impl std::fmt::Debug for WasmPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPlugin")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Stub implementation when WASM feature is disabled
#[cfg(not(feature = "wasm"))]
pub struct WasmPlugin {
    _private: (),
}

#[cfg(not(feature = "wasm"))]
impl WasmPlugin {
    /// Attempt to load a WASM plugin (feature not enabled)
    ///
    /// # Errors
    ///
    /// Always returns error when WASM feature is disabled
    pub fn load(
        _name: String,
        _wasm_bytes: Vec<u8>,
        _memory_limit_pages: Option<u32>,
    ) -> Result<Self> {
        Err(Error::Tool(
            "WASM plugin support requires the 'wasm' feature".to_string(),
        ))
    }

    /// Stub call (never reached)
    ///
    /// # Errors
    ///
    /// Always returns error when WASM feature is disabled
    pub fn call_tool(&mut self, _tool_name: &str, _input: &str) -> Result<String> {
        Err(Error::Tool(
            "WASM plugin support requires the 'wasm' feature".to_string(),
        ))
    }

    /// Get the plugin name (stub)
    #[must_use]
    pub const fn name(&self) -> &'static str {
        ""
    }
}

#[cfg(not(feature = "wasm"))]
impl std::fmt::Debug for WasmPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPlugin")
            .field("enabled", &false)
            .finish_non_exhaustive()
    }
}
