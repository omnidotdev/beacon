//! openclaw skill compatibility bridge
//!
//! Adapts openclaw-format skills for runtime compatibility in Beacon.
//! Handles environment variable validation and install requirement execution.

use crate::{Error, Result};

use super::types::{Skill, SkillInstallSpec};

/// Validate that an openclaw skill's runtime requirements are satisfied
///
/// Checks `requires_env` vars and `requires_bins` binaries.
///
/// # Errors
///
/// Returns error if required environment variables or binaries are missing
pub fn validate_requirements(skill: &Skill) -> Result<()> {
    // Check required environment variables
    let missing_env: Vec<&str> = skill
        .metadata
        .requires_env
        .iter()
        .filter(|var| std::env::var(var).is_err())
        .map(String::as_str)
        .collect();

    if !missing_env.is_empty() {
        return Err(Error::Skill(format!(
            "skill '{}' requires missing env vars: {}",
            skill.metadata.name,
            missing_env.join(", ")
        )));
    }

    // Check required binaries (all must be present)
    let missing_bins: Vec<&str> = skill
        .metadata
        .requires_bins
        .iter()
        .filter(|bin| !super::types::has_binary(bin))
        .map(String::as_str)
        .collect();

    if !missing_bins.is_empty() {
        return Err(Error::Skill(format!(
            "skill '{}' requires missing binaries: {}",
            skill.metadata.name,
            missing_bins.join(", ")
        )));
    }

    // Check any-of binaries (at least one must be present)
    if !skill.metadata.requires_any_bins.is_empty()
        && !skill
            .metadata
            .requires_any_bins
            .iter()
            .any(|bin| super::types::has_binary(bin))
    {
        return Err(Error::Skill(format!(
            "skill '{}' requires at least one of: {}",
            skill.metadata.name,
            skill.metadata.requires_any_bins.join(", ")
        )));
    }

    Ok(())
}

/// Build a shell command to install a skill dependency
///
/// Returns `None` if the install kind is not supported or the spec is incomplete.
#[must_use]
pub fn build_install_command(spec: &SkillInstallSpec) -> Option<String> {
    use super::types::InstallKind;

    match spec.kind {
        InstallKind::Brew => spec.formula.as_ref().map(|f| format!("brew install {f}")),
        InstallKind::Node => spec.package.as_ref().map(|p| format!("npm install -g {p}")),
        InstallKind::Go => spec
            .module
            .as_ref()
            .map(|m| format!("go install {m}@latest")),
        InstallKind::Uv => spec
            .package
            .as_ref()
            .map(|p| format!("uv tool install {p}")),
        InstallKind::Download => spec
            .url
            .as_ref()
            .map(|u| format!("curl -fsSL -o /tmp/download '{u}'")),
    }
}

/// Check whether a skill has openclaw-format metadata
#[must_use]
pub fn is_openclaw_skill(raw_frontmatter: &serde_yaml::Value) -> bool {
    raw_frontmatter.get("metadata").is_some_and(|m| {
        m.get("openclaw").is_some() || m.get("clawdbot").is_some() || m.get("clawdis").is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::{SkillMetadata, SkillSource};

    fn make_skill(requires_env: Vec<&str>, requires_bins: Vec<&str>) -> Skill {
        Skill {
            id: "test".to_string(),
            metadata: SkillMetadata {
                name: "test-skill".to_string(),
                description: "A test skill".to_string(),
                version: None,
                author: None,
                tags: Vec::new(),
                permissions: Vec::new(),
                always: false,
                user_invocable: true,
                disable_model_invocation: false,
                emoji: None,
                requires_env: requires_env.into_iter().map(String::from).collect(),
                os: Vec::new(),
                requires_bins: requires_bins.into_iter().map(String::from).collect(),
                requires_any_bins: Vec::new(),
                primary_env: None,
                command_dispatch: None,
                command_tool: None,
                install: Vec::new(),
                requires_config: Vec::new(),
            },
            content: String::new(),
            source: SkillSource::Local,
            location: None,
        }
    }

    #[test]
    fn validates_present_env_vars() {
        // PATH is always set
        let skill = make_skill(vec!["PATH"], vec![]);
        assert!(validate_requirements(&skill).is_ok());
    }

    #[test]
    fn rejects_missing_env_vars() {
        let skill = make_skill(vec!["DEFINITELY_NOT_SET_XYZ_123"], vec![]);
        assert!(validate_requirements(&skill).is_err());
    }

    #[test]
    fn validates_present_binaries() {
        // sh is always available
        let skill = make_skill(vec![], vec!["sh"]);
        assert!(validate_requirements(&skill).is_ok());
    }

    #[test]
    fn rejects_missing_binaries() {
        let skill = make_skill(vec![], vec!["nonexistent_binary_xyz_123"]);
        assert!(validate_requirements(&skill).is_err());
    }

    #[test]
    fn build_install_command_brew() {
        let spec = SkillInstallSpec {
            kind: super::super::types::InstallKind::Brew,
            formula: Some("jq".to_string()),
            label: None,
            bins: vec![],
            os: vec![],
            package: None,
            module: None,
            url: None,
            archive: None,
            strip_components: None,
            target_dir: None,
        };
        assert_eq!(
            build_install_command(&spec),
            Some("brew install jq".to_string())
        );
    }

    #[test]
    fn is_openclaw_skill_detects_nested_metadata() {
        let yaml: serde_yaml::Value =
            serde_yaml::from_str("metadata:\n  openclaw:\n    requires:\n      env: [TOKEN]\n")
                .unwrap();
        assert!(is_openclaw_skill(&yaml));
    }

    #[test]
    fn is_openclaw_skill_rejects_plain() {
        let yaml: serde_yaml::Value =
            serde_yaml::from_str("name: plain\ndescription: no nested\n").unwrap();
        assert!(!is_openclaw_skill(&yaml));
    }
}
