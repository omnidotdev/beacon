//! Reload classification for config changes
//!
//! Compare old and new config TOML content and classify which subsystems
//! need hot-reloading vs a full restart

/// Diff result indicating which config sections changed
#[derive(Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct ReloadDiff {
    /// Persona identity/prompt changed
    pub persona: bool,
    /// Skills configuration changed
    pub skills: bool,
    /// Hooks configuration changed
    pub hooks: bool,
    /// Tool policy changed
    pub tool_policy: bool,
    /// Memory configuration changed
    pub memory: bool,
    /// Change requires a full daemon restart
    pub requires_restart: bool,
}

/// Keys that can be hot-reloaded without restart
const HOT_KEYS: &[&str] = &["persona", "skills", "hooks", "media", "memory"];

/// Keys that require a full restart
const RESTART_KEYS: &[&str] = &["channels", "api_keys", "server", "llm", "agents"];

impl ReloadDiff {
    /// Whether any hot-reloadable section changed
    #[must_use]
    pub const fn has_hot_changes(&self) -> bool {
        self.persona || self.skills || self.hooks || self.tool_policy || self.memory
    }
}

/// Compare two TOML config strings and classify what changed
///
/// Parses both strings as `toml::Value` tables and compares top-level keys.
#[must_use]
pub fn classify_changes(old: &str, new: &str) -> ReloadDiff {
    let old_table = old.parse::<toml::Value>().ok();
    let new_table = new.parse::<toml::Value>().ok();

    let (Some(toml::Value::Table(old_map)), Some(toml::Value::Table(new_map))) =
        (old_table, new_table)
    else {
        // If either fails to parse, assume restart required
        return ReloadDiff {
            requires_restart: true,
            ..ReloadDiff::default()
        };
    };

    let mut diff = ReloadDiff::default();

    // Collect all keys from both configs
    let all_keys: std::collections::HashSet<&str> = old_map
        .keys()
        .chain(new_map.keys())
        .map(String::as_str)
        .collect();

    for key in all_keys {
        let old_val = old_map.get(key);
        let new_val = new_map.get(key);

        if old_val == new_val {
            continue;
        }

        match key {
            "persona" => diff.persona = true,
            "skills" => diff.skills = true,
            "hooks" => diff.hooks = true,
            "media" => {} // hot-reloadable but no dedicated flag
            "memory" => diff.memory = true,
            "tools" | "tool_policy" => diff.tool_policy = true,
            k if RESTART_KEYS.contains(&k) => diff.requires_restart = true,
            // Unknown keys default to restart-required for safety
            _ => diff.requires_restart = true,
        }
    }

    diff
}

/// Check whether a key is hot-reloadable
#[must_use]
pub fn is_hot_key(key: &str) -> bool {
    HOT_KEYS.contains(&key) || key == "tools" || key == "tool_policy"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_persona_change() {
        let old = r#"
[persona]
name = "orin"
"#;
        let new = r#"
[persona]
name = "beacon"
"#;
        let diff = classify_changes(old, new);
        assert!(diff.persona);
        assert!(diff.has_hot_changes());
        assert!(!diff.requires_restart);
    }

    #[test]
    fn detects_restart_required() {
        let old = r#"
[server]
port = 3000
"#;
        let new = r#"
[server]
port = 4000
"#;
        let diff = classify_changes(old, new);
        assert!(diff.requires_restart);
        assert!(!diff.has_hot_changes());
    }

    #[test]
    fn no_change_is_empty() {
        let config = r#"
[persona]
name = "orin"

[server]
port = 3000
"#;
        let diff = classify_changes(config, config);
        assert_eq!(diff, ReloadDiff::default());
        assert!(!diff.has_hot_changes());
        assert!(!diff.requires_restart);
    }

    #[test]
    fn detects_tool_policy_change() {
        let old = r#"
[tool_policy]
allow_all = true
"#;
        let new = r#"
[tool_policy]
allow_all = false
"#;
        let diff = classify_changes(old, new);
        assert!(diff.tool_policy);
        assert!(diff.has_hot_changes());
    }

    #[test]
    fn detects_tools_key_as_tool_policy() {
        let old = "";
        let new = r#"
[tools]
enabled = ["web_search"]
"#;
        let diff = classify_changes(old, new);
        assert!(diff.tool_policy);
    }

    #[test]
    fn mixed_hot_and_restart() {
        let old = r#"
[persona]
name = "orin"

[channels]
discord = true
"#;
        let new = r#"
[persona]
name = "beacon"

[channels]
discord = false
"#;
        let diff = classify_changes(old, new);
        assert!(diff.persona);
        assert!(diff.has_hot_changes());
        assert!(diff.requires_restart);
    }
}
