//! CLI helpers for config get/set/path

use std::path::PathBuf;

/// Print the config file path
///
/// # Errors
///
/// Returns error if the config directory cannot be determined
pub fn config_path() -> anyhow::Result<()> {
    match super::file::config_file_path() {
        Some(path) => {
            println!("{}", path.display());
            Ok(())
        }
        None => anyhow::bail!("could not determine config directory"),
    }
}

/// Get a config value by dotted key path
///
/// # Errors
///
/// Returns error if the config file cannot be read or key is not found
pub fn config_get(key: &str) -> anyhow::Result<()> {
    let path = config_file_or_bail()?;

    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;

    let doc: toml::Value =
        toml::from_str(&content).map_err(|e| anyhow::anyhow!("failed to parse config: {e}"))?;

    let value = navigate_toml(&doc, key)
        .ok_or_else(|| anyhow::anyhow!("key '{key}' not found in config"))?;

    print_toml_value(value);

    Ok(())
}

/// Set a config value by dotted key path
///
/// # Errors
///
/// Returns error if the config file cannot be read or written
pub fn config_set(key: &str, value: &str) -> anyhow::Result<()> {
    let path = config_file_or_bail()?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Read existing or start fresh
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };

    let mut doc: toml::Value = if content.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&content).map_err(|e| anyhow::anyhow!("failed to parse config: {e}"))?
    };

    // Parse the value (try bool, int, float, then fall back to string)
    let toml_value = parse_value(value);

    set_toml_value(&mut doc, key, toml_value)?;

    let output = toml::to_string_pretty(&doc)
        .map_err(|e| anyhow::anyhow!("failed to serialize config: {e}"))?;

    std::fs::write(&path, output)?;

    println!("set {key} = {value}");

    Ok(())
}

fn config_file_or_bail() -> anyhow::Result<PathBuf> {
    super::file::config_file_path()
        .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))
}

/// Navigate a TOML value by dotted key path
fn navigate_toml<'a>(value: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut current = value;
    for part in key.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Set a value in a TOML document by dotted key path
fn set_toml_value(doc: &mut toml::Value, key: &str, value: toml::Value) -> anyhow::Result<()> {
    let parts: Vec<&str> = key.split('.').collect();

    let mut current = doc;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Set the leaf value
            match current {
                toml::Value::Table(table) => {
                    table.insert((*part).to_string(), value);
                    return Ok(());
                }
                _ => anyhow::bail!("cannot set key on non-table value"),
            }
        }
        // Navigate or create intermediate tables
        match current {
            toml::Value::Table(table) => {
                current = table
                    .entry((*part).to_string())
                    .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            }
            _ => anyhow::bail!("intermediate key '{part}' is not a table"),
        }
    }

    Ok(())
}

/// Parse a string value into the most appropriate TOML type
fn parse_value(s: &str) -> toml::Value {
    if s == "true" {
        return toml::Value::Boolean(true);
    }
    if s == "false" {
        return toml::Value::Boolean(false);
    }
    if let Ok(i) = s.parse::<i64>() {
        return toml::Value::Integer(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return toml::Value::Float(f);
    }
    toml::Value::String(s.to_string())
}

fn print_toml_value(value: &toml::Value) {
    match value {
        toml::Value::String(s) => println!("{s}"),
        toml::Value::Integer(i) => println!("{i}"),
        toml::Value::Float(f) => println!("{f}"),
        toml::Value::Boolean(b) => println!("{b}"),
        toml::Value::Array(arr) => {
            for item in arr {
                print_toml_value(item);
            }
        }
        toml::Value::Table(_) => {
            if let Ok(s) = toml::to_string_pretty(value) {
                print!("{s}");
            }
        }
        toml::Value::Datetime(dt) => println!("{dt}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigate_dotted_path() {
        let doc: toml::Value = toml::from_str(
            r#"
            [llm]
            model = "claude-sonnet-4-20250514"
            [voice]
            enabled = true
            "#,
        )
        .unwrap();

        assert_eq!(
            navigate_toml(&doc, "llm.model"),
            Some(&toml::Value::String("claude-sonnet-4-20250514".to_string()))
        );
        assert_eq!(
            navigate_toml(&doc, "voice.enabled"),
            Some(&toml::Value::Boolean(true))
        );
        assert!(navigate_toml(&doc, "missing.key").is_none());
    }

    #[test]
    fn set_creates_intermediate_tables() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_toml_value(
            &mut doc,
            "llm.model",
            toml::Value::String("gpt-4o".to_string()),
        )
        .unwrap();

        assert_eq!(
            navigate_toml(&doc, "llm.model"),
            Some(&toml::Value::String("gpt-4o".to_string()))
        );
    }

    #[test]
    fn parse_value_types() {
        assert_eq!(parse_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_value("42"), toml::Value::Integer(42));
        assert_eq!(
            parse_value("hello"),
            toml::Value::String("hello".to_string())
        );
    }
}
