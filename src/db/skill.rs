//! Skill repository for installed skills persistence

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use sqlx::Row;
use sqlx::any::AnyRow;

use super::DbPool;
use crate::Result;
use crate::skills::{InstalledSkill, Skill, SkillMetadata, SkillPriority, SkillSource};

/// Skill repository for CRUD operations on installed skills
#[derive(Clone)]
pub struct SkillRepo {
    pool: DbPool,
}

impl SkillRepo {
    /// Create a new skill repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Install a skill with a specific priority and optional user scope
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    #[allow(clippy::too_many_lines)]
    pub async fn install_with_priority(
        &self,
        skill: &Skill,
        priority: SkillPriority,
        user_id: Option<&str>,
    ) -> Result<InstalledSkill> {
        // Generate command name before acquiring the INSERT connection to avoid
        // holding two pool connections simultaneously (deadlocks single-conn pools)
        let command_name = if skill.metadata.user_invocable {
            let existing = self.list_command_names().await?;
            Some(crate::skills::deduplicate_command_name(
                &skill.metadata.name,
                &existing,
            ))
        } else {
            None
        };

        let id = Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(&skill.metadata.tags).unwrap_or_default();
        let permissions_json =
            serde_json::to_string(&skill.metadata.permissions).unwrap_or_default();
        let requires_env_json =
            serde_json::to_string(&skill.metadata.requires_env).unwrap_or_default();
        let os_json = serde_json::to_string(&skill.metadata.os).unwrap_or_default();
        let requires_bins_json =
            serde_json::to_string(&skill.metadata.requires_bins).unwrap_or_default();
        let requires_any_bins_json =
            serde_json::to_string(&skill.metadata.requires_any_bins).unwrap_or_default();

        let (source_type, source_namespace, source_repository) = match &skill.source {
            SkillSource::Local => ("local", None, None),
            SkillSource::Manifold {
                namespace,
                repository,
            } => (
                "manifold",
                Some(namespace.as_str()),
                Some(repository.as_str()),
            ),
            SkillSource::Bundled => ("bundled", None, None),
            SkillSource::Plugin => ("plugin", None, None),
        };

        // Compute command_dispatch_tool from metadata
        let command_dispatch_tool = if skill.metadata.command_dispatch.as_deref() == Some("tool") {
            skill.metadata.command_tool.clone()
        } else {
            None
        };

        let install_specs_json =
            serde_json::to_string(&skill.metadata.install).unwrap_or_else(|_| "[]".to_string());
        let requires_config_json = serde_json::to_string(&skill.metadata.requires_config)
            .unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            r"
            INSERT INTO installed_skills (
                id, name, description, version, author, tags, permissions,
                content, source_type, source_namespace, source_repository,
                enabled, priority, always_include, user_invocable,
                disable_model_invocation, emoji, requires_env, command_name, user_id,
                os, requires_bins, requires_any_bins, primary_env,
                command_dispatch_tool, api_key, skill_env,
                install_specs, requires_config, location
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 1, $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21, $22, $23, $24, NULL, '{}',
                $25, $26, $27
            )
            ",
        )
        .bind(id.clone())
        .bind(skill.metadata.name.clone())
        .bind(skill.metadata.description.clone())
        .bind(skill.metadata.version.clone())
        .bind(skill.metadata.author.clone())
        .bind(tags_json)
        .bind(permissions_json)
        .bind(skill.content.clone())
        .bind(source_type)
        .bind(source_namespace.map(str::to_string))
        .bind(source_repository.map(str::to_string))
        .bind(priority.as_db())
        .bind(i64::from(skill.metadata.always))
        .bind(i64::from(skill.metadata.user_invocable))
        .bind(i64::from(skill.metadata.disable_model_invocation))
        .bind(skill.metadata.emoji.clone())
        .bind(requires_env_json)
        .bind(command_name.clone())
        .bind(user_id.map(str::to_string))
        .bind(os_json)
        .bind(requires_bins_json)
        .bind(requires_any_bins_json)
        .bind(skill.metadata.primary_env.clone())
        .bind(command_dispatch_tool.clone())
        .bind(install_specs_json)
        .bind(requires_config_json)
        .bind(skill.location.clone())
        .execute(self.pool.pool())
        .await?;

        tracing::info!(skill_id = %id, name = %skill.metadata.name, priority = %priority.as_db(), "skill installed");

        let mut installed_skill = skill.clone();
        installed_skill.id = id;

        Ok(InstalledSkill {
            skill: installed_skill,
            installed_at: Utc::now(),
            enabled: true,
            priority,
            command_name,
            user_id: user_id.map(String::from),
            command_dispatch_tool,
            api_key: None,
            skill_env: HashMap::new(),
        })
    }

    /// Install a skill with default (standard) priority
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn install(&self, skill: &Skill) -> Result<InstalledSkill> {
        self.install_with_priority(skill, SkillPriority::default(), None)
            .await
    }

    /// Uninstall a skill by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn uninstall(&self, skill_id: &str) -> Result<bool> {
        let rows = sqlx::query("DELETE FROM installed_skills WHERE id = $1")
            .bind(skill_id.to_string())
            .execute(self.pool.pool())
            .await?
            .rows_affected();

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, "skill uninstalled");
        }

        Ok(rows > 0)
    }

    /// Column list for SELECT queries
    const SELECT_COLS: &str = r"
        id, name, description, version, author, tags, permissions,
        content, source_type, source_namespace, source_repository,
        enabled, installed_at, priority,
        always_include, user_invocable, disable_model_invocation,
        emoji, requires_env, command_name, user_id,
        os, requires_bins, requires_any_bins, primary_env,
        command_dispatch_tool, api_key, skill_env,
        install_specs, requires_config, location
    ";

    /// Get an installed skill by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get(&self, skill_id: &str) -> Result<Option<InstalledSkill>> {
        self.fetch_one_where("id = $1", vec![skill_id.to_string()])
            .await
    }

    /// Get an installed skill by name
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_by_name(&self, name: &str) -> Result<Option<InstalledSkill>> {
        self.fetch_one_where("name = $1", vec![name.to_string()])
            .await
    }

    /// Look up a skill by its slash command name, scoped to a user
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_by_command_name(
        &self,
        command: &str,
        user_id: Option<&str>,
    ) -> Result<Option<InstalledSkill>> {
        self.fetch_one_where(
            "command_name = $1 AND (user_id IS NULL OR user_id = $2)",
            vec![command.to_string(), user_id.unwrap_or("").to_string()],
        )
        .await
    }

    /// The first skill matching a `WHERE` clause over text binds
    async fn fetch_one_where(
        &self,
        condition: &str,
        binds: Vec<String>,
    ) -> Result<Option<InstalledSkill>> {
        let sql = format!(
            "SELECT {} FROM installed_skills WHERE {condition}",
            Self::SELECT_COLS,
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for bind in binds {
            query = query.bind(bind);
        }
        query
            .fetch_optional(self.pool.pool())
            .await?
            .as_ref()
            .map(Self::row_to_installed_skill)
            .transpose()
    }

    /// Every skill matching a `WHERE` clause over text binds, by name
    async fn fetch_all_where(
        &self,
        condition: &str,
        binds: Vec<String>,
    ) -> Result<Vec<InstalledSkill>> {
        let sql = format!(
            "SELECT {} FROM installed_skills WHERE {condition} ORDER BY name",
            Self::SELECT_COLS,
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for bind in binds {
            query = query.bind(bind);
        }
        query
            .fetch_all(self.pool.pool())
            .await?
            .iter()
            .map(Self::row_to_installed_skill)
            .collect()
    }

    /// List all installed skills (admin view, no user filtering)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list(&self) -> Result<Vec<InstalledSkill>> {
        self.fetch_all_where("1 = 1", vec![]).await
    }

    /// List enabled skills only (admin view, no user filtering)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list_enabled(&self) -> Result<Vec<InstalledSkill>> {
        self.fetch_all_where("enabled = 1", vec![]).await
    }

    /// List enabled skills visible to a specific user (shared + user-specific)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list_enabled_for_user(
        &self,
        user_id: Option<&str>,
    ) -> Result<Vec<InstalledSkill>> {
        self.fetch_all_where(
            "enabled = 1 AND (user_id IS NULL OR user_id = $1)",
            vec![user_id.unwrap_or("").to_string()],
        )
        .await
    }

    /// List all existing command names (for deduplication)
    async fn list_command_names(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT command_name FROM installed_skills WHERE command_name IS NOT NULL",
        )
        .fetch_all(self.pool.pool())
        .await?)
    }

    /// Upsert a bundled skill (INSERT OR REPLACE by name + bundled source)
    ///
    /// Preserves user settings (enabled, priority) when updating content.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn upsert_bundled(
        &self,
        skill: &Skill,
        priority: SkillPriority,
    ) -> Result<InstalledSkill> {
        // Check if already installed
        if let Some(existing) = self.get_by_name(&skill.metadata.name).await?
            && existing.skill.source == SkillSource::Bundled
        {
            // Update content but preserve user settings
            let requires_env_json =
                serde_json::to_string(&skill.metadata.requires_env).unwrap_or_default();
            let os_json = serde_json::to_string(&skill.metadata.os).unwrap_or_default();
            let requires_bins_json =
                serde_json::to_string(&skill.metadata.requires_bins).unwrap_or_default();
            let requires_any_bins_json =
                serde_json::to_string(&skill.metadata.requires_any_bins).unwrap_or_default();
            let command_dispatch_tool =
                if skill.metadata.command_dispatch.as_deref() == Some("tool") {
                    skill.metadata.command_tool.clone()
                } else {
                    None
                };
            let install_specs_json =
                serde_json::to_string(&skill.metadata.install).unwrap_or_else(|_| "[]".to_string());
            let requires_config_json = serde_json::to_string(&skill.metadata.requires_config)
                .unwrap_or_else(|_| "[]".to_string());

            sqlx::query(
                r"
                    UPDATE installed_skills
                    SET content = $1, description = $2, always_include = $3,
                        user_invocable = $4, disable_model_invocation = $5,
                        emoji = $6, requires_env = $7,
                        os = $8, requires_bins = $9, requires_any_bins = $10,
                        primary_env = $11, command_dispatch_tool = $12,
                        install_specs = $13, requires_config = $14,
                        location = $15,
                        updated_at = $16
                    WHERE id = $17
                    ",
            )
            .bind(skill.content.clone())
            .bind(skill.metadata.description.clone())
            .bind(i64::from(skill.metadata.always))
            .bind(i64::from(skill.metadata.user_invocable))
            .bind(i64::from(skill.metadata.disable_model_invocation))
            .bind(skill.metadata.emoji.clone())
            .bind(requires_env_json)
            .bind(os_json)
            .bind(requires_bins_json)
            .bind(requires_any_bins_json)
            .bind(skill.metadata.primary_env.clone())
            .bind(command_dispatch_tool.clone())
            .bind(install_specs_json)
            .bind(requires_config_json)
            .bind(skill.location.clone())
            .bind(super::now())
            .bind(existing.skill.id.clone())
            .execute(self.pool.pool())
            .await?;

            tracing::debug!(name = %skill.metadata.name, "updated bundled skill content");

            // Return with preserved settings
            return Ok(InstalledSkill {
                skill: Skill {
                    id: existing.skill.id,
                    metadata: skill.metadata.clone(),
                    content: skill.content.clone(),
                    source: SkillSource::Bundled,
                    location: skill.location.clone(),
                },
                installed_at: existing.installed_at,
                enabled: existing.enabled,
                priority: existing.priority,
                command_name: existing.command_name,
                user_id: None,
                command_dispatch_tool,
                api_key: existing.api_key,
                skill_env: existing.skill_env,
            });
        }

        // Not found or not bundled — fresh install
        let mut bundled_skill = skill.clone();
        bundled_skill.source = SkillSource::Bundled;
        self.install_with_priority(&bundled_skill, priority, None)
            .await
    }

    /// Enable or disable a skill
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn set_enabled(&self, skill_id: &str, enabled: bool) -> Result<bool> {
        let rows = sqlx::query(
            r"
            UPDATE installed_skills
            SET enabled = $1, updated_at = $2
            WHERE id = $3
            ",
        )
        .bind(i64::from(enabled))
        .bind(super::now())
        .bind(skill_id.to_string())
        .execute(self.pool.pool())
        .await?
        .rows_affected();

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, enabled = %enabled, "skill enabled state changed");
        }

        Ok(rows > 0)
    }

    /// Set the priority of a skill
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn set_priority(&self, skill_id: &str, priority: SkillPriority) -> Result<bool> {
        let rows = sqlx::query(
            r"
            UPDATE installed_skills
            SET priority = $1, updated_at = $2
            WHERE id = $3
            ",
        )
        .bind(priority.as_db())
        .bind(super::now())
        .bind(skill_id.to_string())
        .execute(self.pool.pool())
        .await?
        .rows_affected();

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, priority = %priority.as_db(), "skill priority changed");
        }

        Ok(rows > 0)
    }

    /// Update skill configuration (`api_key` and/or env overrides)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn update_skill_config(
        &self,
        skill_id: &str,
        api_key: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> Result<bool> {
        let mut updates = Vec::new();
        let mut binds: Vec<String> = Vec::new();

        if let Some(key) = api_key {
            binds.push(key.to_string());
            updates.push(format!("api_key = ${}", binds.len()));
        }
        if let Some(env_map) = env {
            binds.push(serde_json::to_string(env_map).unwrap_or_else(|_| "{}".to_string()));
            updates.push(format!("skill_env = ${}", binds.len()));
        }

        if updates.is_empty() {
            return Ok(false);
        }

        binds.push(super::now());
        updates.push(format!("updated_at = ${}", binds.len()));
        binds.push(skill_id.to_string());
        let sql = format!(
            "UPDATE installed_skills SET {} WHERE id = ${}",
            updates.join(", "),
            binds.len()
        );

        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for bind in binds {
            query = query.bind(bind);
        }
        let rows = query.execute(self.pool.pool()).await?.rows_affected();

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, "skill config updated");
        }

        Ok(rows > 0)
    }

    /// Convert a database row to an `InstalledSkill`
    #[allow(clippy::too_many_lines)]
    fn row_to_installed_skill(row: &AnyRow) -> Result<InstalledSkill> {
        let id: String = row.try_get(0)?;
        let name: String = row.try_get(1)?;
        let description: String = row.try_get(2)?;
        let version: Option<String> = row.try_get(3)?;
        let author: Option<String> = row.try_get(4)?;
        let tags_json: String = row.try_get(5)?;
        let permissions_json: String = row.try_get(6)?;
        let content: String = row.try_get(7)?;
        let source_type: String = row.try_get(8)?;
        let source_namespace: Option<String> = row.try_get(9)?;
        let source_repository: Option<String> = row.try_get(10)?;
        let enabled = row.try_get::<i64, _>(11)? != 0;
        let installed_at: String = row.try_get(12)?;
        let priority_str: String = row
            .try_get::<Option<String>, _>(13)?
            .unwrap_or_else(|| "standard".to_string());
        let always: bool = row.try_get::<Option<i64>, _>(14)?.is_some_and(|v| v != 0);
        let user_invocable: bool = row.try_get::<Option<i64>, _>(15)?.is_none_or(|v| v != 0);
        let disable_model_invocation: bool =
            row.try_get::<Option<i64>, _>(16)?.is_some_and(|v| v != 0);
        let emoji: Option<String> = row.try_get(17)?;
        let requires_env_json: String = row
            .try_get::<Option<String>, _>(18)?
            .unwrap_or_else(|| "[]".to_string());
        let command_name: Option<String> = row.try_get(19)?;
        let user_id: Option<String> = row.try_get(20)?;

        // v14 columns (safe defaults for pre-migration DBs)
        let os_json: String = row
            .try_get::<Option<String>, _>(21)?
            .unwrap_or_else(|| "[]".to_string());
        let requires_bins_json: String = row
            .try_get::<Option<String>, _>(22)?
            .unwrap_or_else(|| "[]".to_string());
        let requires_any_bins_json: String = row
            .try_get::<Option<String>, _>(23)?
            .unwrap_or_else(|| "[]".to_string());
        let primary_env: Option<String> = row.try_get(24)?;
        let command_dispatch_tool: Option<String> = row.try_get(25)?;
        let api_key: Option<String> = row.try_get(26)?;
        let skill_env_json: String = row
            .try_get::<Option<String>, _>(27)?
            .unwrap_or_else(|| "{}".to_string());

        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        let permissions: Vec<String> = serde_json::from_str(&permissions_json).unwrap_or_default();
        let requires_env: Vec<String> =
            serde_json::from_str(&requires_env_json).unwrap_or_default();
        let os: Vec<String> = serde_json::from_str(&os_json).unwrap_or_default();
        let requires_bins: Vec<String> =
            serde_json::from_str(&requires_bins_json).unwrap_or_default();
        let requires_any_bins: Vec<String> =
            serde_json::from_str(&requires_any_bins_json).unwrap_or_default();
        // v15/v16 columns
        let install_specs_json: String = row
            .try_get::<Option<String>, _>(28)?
            .unwrap_or_else(|| "[]".to_string());
        let requires_config_json: String = row
            .try_get::<Option<String>, _>(29)?
            .unwrap_or_else(|| "[]".to_string());
        // v18 column
        let location: Option<String> = row.try_get(30).unwrap_or(None);

        let skill_env: HashMap<String, String> =
            serde_json::from_str(&skill_env_json).unwrap_or_default();
        let install: Vec<crate::skills::SkillInstallSpec> =
            serde_json::from_str(&install_specs_json).unwrap_or_default();
        let requires_config: Vec<String> =
            serde_json::from_str(&requires_config_json).unwrap_or_default();

        let source = match source_type.as_str() {
            "manifold" => SkillSource::Manifold {
                namespace: source_namespace.unwrap_or_default(),
                repository: source_repository.unwrap_or_default(),
            },
            "bundled" => SkillSource::Bundled,
            "plugin" => SkillSource::Plugin,
            _ => SkillSource::Local,
        };

        let installed_at = DateTime::parse_from_rfc3339(&format!("{installed_at}Z"))
            .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));

        Ok(InstalledSkill {
            skill: Skill {
                id,
                metadata: SkillMetadata {
                    name,
                    description,
                    version,
                    author,
                    tags,
                    permissions,
                    always,
                    user_invocable,
                    disable_model_invocation,
                    emoji,
                    requires_env,
                    os,
                    requires_bins,
                    requires_any_bins,
                    primary_env,
                    command_dispatch: None,
                    command_tool: None,
                    install,
                    requires_config,
                },
                content,
                source,
                location,
            },
            installed_at,
            enabled,
            priority: SkillPriority::from_db(&priority_str),
            command_name,
            user_id,
            command_dispatch_tool,
            api_key,
            skill_env,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn test_skill() -> Skill {
        Skill {
            id: "test-skill".to_string(),
            metadata: SkillMetadata {
                name: "test-skill".to_string(),
                description: "A test skill".to_string(),
                version: Some("1.0.0".to_string()),
                author: Some("Test".to_string()),
                tags: vec!["test".to_string()],
                permissions: vec![],
                always: false,
                user_invocable: true,
                disable_model_invocation: false,
                emoji: None,
                requires_env: vec![],
                os: vec![],
                requires_bins: vec![],
                requires_any_bins: vec![],
                primary_env: None,
                command_dispatch: None,
                command_tool: None,
                install: vec![],
                requires_config: vec![],
            },
            content: "# Test Skill\n\nThis is a test.".to_string(),
            source: SkillSource::Local,
            location: None,
        }
    }

    #[tokio::test]
    async fn test_install_and_get() {
        let pool = init_memory().await.unwrap();
        let repo = SkillRepo::new(pool);

        let skill = test_skill();
        let installed = repo.install(&skill).await.unwrap();

        assert_eq!(installed.skill.metadata.name, "test-skill");
        assert!(installed.enabled);

        let fetched = repo.get_by_name("test-skill").await.unwrap().unwrap();
        assert_eq!(fetched.skill.metadata.name, "test-skill");
    }

    #[tokio::test]
    async fn test_list_and_uninstall() {
        let pool = init_memory().await.unwrap();
        let repo = SkillRepo::new(pool);

        let skill = test_skill();
        let installed = repo.install(&skill).await.unwrap();

        let list = repo.list().await.unwrap();
        assert_eq!(list.len(), 1);

        let removed = repo.uninstall(&installed.skill.id).await.unwrap();
        assert!(removed);

        let list = repo.list().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn test_enable_disable() {
        let pool = init_memory().await.unwrap();
        let repo = SkillRepo::new(pool);

        let skill = test_skill();
        let installed = repo.install(&skill).await.unwrap();

        // Disable
        repo.set_enabled(&installed.skill.id, false).await.unwrap();
        let fetched = repo.get(&installed.skill.id).await.unwrap().unwrap();
        assert!(!fetched.enabled);

        // List enabled should be empty
        let enabled = repo.list_enabled().await.unwrap();
        assert!(enabled.is_empty());

        // Re-enable
        repo.set_enabled(&installed.skill.id, true).await.unwrap();
        let enabled = repo.list_enabled().await.unwrap();
        assert_eq!(enabled.len(), 1);
    }

    #[tokio::test]
    async fn test_install_specs_db_roundtrip() {
        let pool = init_memory().await.unwrap();
        let repo = SkillRepo::new(pool);

        let mut skill = test_skill();
        skill.metadata.install = vec![crate::skills::SkillInstallSpec {
            kind: crate::skills::InstallKind::Brew,
            label: Some("jq".to_string()),
            bins: vec!["jq".to_string()],
            os: vec!["darwin".to_string(), "linux".to_string()],
            formula: Some("jq".to_string()),
            package: None,
            module: None,
            url: None,
            archive: None,
            strip_components: None,
            target_dir: None,
        }];
        skill.metadata.requires_config = vec!["voice.enabled".to_string()];

        let installed = repo.install(&skill).await.unwrap();
        let fetched = repo.get(&installed.skill.id).await.unwrap().unwrap();

        assert_eq!(fetched.skill.metadata.install.len(), 1);
        assert_eq!(
            fetched.skill.metadata.install[0].kind,
            crate::skills::InstallKind::Brew
        );
        assert_eq!(
            fetched.skill.metadata.install[0].formula.as_deref(),
            Some("jq")
        );
        assert_eq!(
            fetched.skill.metadata.requires_config,
            vec!["voice.enabled"]
        );
    }
}
