//! Cron tools for scheduling recurring tasks
//!
//! Provides agent-accessible tools for scheduling, listing, and canceling
//! recurring tasks through either the Vortex scheduling service or a local
//! in-process scheduler

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use agent_core::tools::{ToolKind, ToolProvider};

use crate::Result;
use crate::cron::{CronAction, LocalScheduler};
use crate::integrations::{ScheduleRequest, VortexClient};

/// Backend for cron scheduling operations
#[derive(Debug, Clone)]
pub enum CronBackend {
    /// Remote Vortex scheduling service
    Vortex {
        /// Vortex API client
        client: VortexClient,
        /// Base URL for callbacks
        callback_base_url: String,
    },
    /// Local in-process scheduler
    Local(Arc<LocalScheduler>),
}

/// Tools for managing scheduled tasks
#[derive(Debug, Clone)]
pub struct CronTools {
    /// Scheduling backend
    backend: CronBackend,
}

impl CronTools {
    /// Create a new `CronTools` instance with a Vortex backend
    ///
    /// # Arguments
    ///
    /// * `vortex` - Configured Vortex client
    /// * `callback_base_url` - Base URL where Vortex will send callbacks
    #[must_use]
    pub fn new(vortex: VortexClient, callback_base_url: impl Into<String>) -> Self {
        Self {
            backend: CronBackend::Vortex {
                client: vortex,
                callback_base_url: callback_base_url.into(),
            },
        }
    }

    /// Create a new `CronTools` instance with a local scheduler backend
    #[must_use]
    pub const fn local(scheduler: Arc<LocalScheduler>) -> Self {
        Self {
            backend: CronBackend::Local(scheduler),
        }
    }

    /// Schedule a recurring task
    ///
    /// # Arguments
    ///
    /// * `cron` - Cron expression (e.g., "0 9 * * MON" for 9 AM every Monday)
    /// * `action` - Action type to trigger (e.g., `remind`, `check_in`)
    /// * `payload` - Arbitrary JSON data to include in callback
    ///
    /// # Returns
    ///
    /// The schedule ID on success
    ///
    /// # Errors
    ///
    /// Returns an error if the scheduling operation fails
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let tools = CronTools::new(vortex, "http://localhost:8080/webhooks/vortex");
    /// let id = tools.schedule(
    ///     "0 9 * * MON",
    ///     "remind",
    ///     serde_json::json!({ "message": "Weekly standup" }),
    /// ).await?;
    /// ```
    pub async fn schedule(
        &self,
        cron: &str,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<String> {
        match &self.backend {
            CronBackend::Vortex {
                client,
                callback_base_url,
            } => {
                let request = ScheduleRequest {
                    cron: cron.to_string(),
                    callback_url: callback_base_url.clone(),
                    action: action.to_string(),
                    payload,
                    description: None,
                    timezone: None,
                };

                let schedule = client.schedule(&request).await?;
                Ok(schedule.id)
            }
            CronBackend::Local(scheduler) => {
                let cron_action = CronAction::Webhook {
                    url: action.to_string(),
                    payload: Some(payload),
                };
                let job = scheduler.schedule(cron, cron_action, None, None).await?;
                Ok(job.id)
            }
        }
    }

    /// Schedule a recurring task with additional options
    ///
    /// # Arguments
    ///
    /// * `params` - Full schedule parameters
    ///
    /// # Returns
    ///
    /// The schedule ID on success
    ///
    /// # Errors
    ///
    /// Returns an error if the scheduling operation fails
    pub async fn schedule_with_options(&self, params: ScheduleParams) -> Result<String> {
        match &self.backend {
            CronBackend::Vortex {
                client,
                callback_base_url,
            } => {
                let request = ScheduleRequest {
                    cron: params.cron,
                    callback_url: callback_base_url.clone(),
                    action: params.action,
                    payload: params.payload,
                    description: params.description,
                    timezone: params.timezone,
                };

                let schedule = client.schedule(&request).await?;
                Ok(schedule.id)
            }
            CronBackend::Local(scheduler) => {
                let cron_action = CronAction::Webhook {
                    url: params.action,
                    payload: Some(params.payload),
                };
                let job = scheduler
                    .schedule(
                        &params.cron,
                        cron_action,
                        params.description,
                        params.timezone,
                    )
                    .await?;
                Ok(job.id)
            }
        }
    }

    /// List all scheduled tasks
    ///
    /// # Errors
    ///
    /// Returns an error if the listing operation fails
    pub async fn list(&self) -> Result<Vec<ScheduleInfo>> {
        match &self.backend {
            CronBackend::Vortex { client, .. } => {
                let schedules = client.list_schedules().await?;

                let infos = schedules
                    .into_iter()
                    .map(|s| ScheduleInfo {
                        id: s.id,
                        cron: s.cron,
                        action: s.action,
                        next_run: s.next_run.map(|dt| dt.to_rfc3339()),
                        description: s.description,
                        active: s.active,
                    })
                    .collect();

                Ok(infos)
            }
            CronBackend::Local(scheduler) => {
                let jobs = scheduler.list().await;

                let infos = jobs
                    .into_iter()
                    .map(|job| {
                        let next_run = croner::Cron::new(&job.schedule)
                            .parse()
                            .ok()
                            .and_then(|c| c.find_next_occurrence(&Utc::now(), false).ok())
                            .map(|dt| dt.to_rfc3339());

                        let action = match &job.action {
                            CronAction::AgentPrompt { channel, .. } => channel.clone(),
                            CronAction::Webhook { url, .. } => url.clone(),
                        };

                        ScheduleInfo {
                            id: job.id,
                            cron: job.schedule,
                            action,
                            next_run,
                            description: job.description,
                            active: true,
                        }
                    })
                    .collect();

                Ok(infos)
            }
        }
    }

    /// Cancel a scheduled task
    ///
    /// # Arguments
    ///
    /// * `schedule_id` - ID of the schedule to cancel
    ///
    /// # Errors
    ///
    /// Returns an error if the cancellation fails
    pub async fn cancel(&self, schedule_id: &str) -> Result<()> {
        match &self.backend {
            CronBackend::Vortex { client, .. } => client.cancel_schedule(schedule_id).await,
            CronBackend::Local(scheduler) => scheduler.cancel(schedule_id).await,
        }
    }

    /// Get details of a specific schedule
    ///
    /// # Arguments
    ///
    /// * `schedule_id` - ID of the schedule to retrieve
    ///
    /// # Errors
    ///
    /// Returns an error if the schedule is not found or the lookup fails
    pub async fn get(&self, schedule_id: &str) -> Result<ScheduleInfo> {
        match &self.backend {
            CronBackend::Vortex { client, .. } => {
                let schedule = client.get_schedule(schedule_id).await?;

                Ok(ScheduleInfo {
                    id: schedule.id,
                    cron: schedule.cron,
                    action: schedule.action,
                    next_run: schedule.next_run.map(|dt| dt.to_rfc3339()),
                    description: schedule.description,
                    active: schedule.active,
                })
            }
            CronBackend::Local(scheduler) => {
                let job = scheduler.get(schedule_id).await.ok_or_else(|| {
                    crate::Error::NotFound(format!("cron job not found: {schedule_id}"))
                })?;

                let next_run = croner::Cron::new(&job.schedule)
                    .parse()
                    .ok()
                    .and_then(|c| c.find_next_occurrence(&Utc::now(), false).ok())
                    .map(|dt| dt.to_rfc3339());

                let action = match &job.action {
                    CronAction::AgentPrompt { channel, .. } => channel.clone(),
                    CronAction::Webhook { url, .. } => url.clone(),
                };

                Ok(ScheduleInfo {
                    id: job.id,
                    cron: job.schedule,
                    action,
                    next_run,
                    description: job.description,
                    active: true,
                })
            }
        }
    }
}

/// Parameters for scheduling a task with full options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleParams {
    /// Cron expression (e.g., "0 9 * * MON")
    pub cron: String,
    /// Action type (e.g., `remind`, `check_in`)
    pub action: String,
    /// Arbitrary payload data
    pub payload: serde_json::Value,
    /// Optional human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional timezone (defaults to UTC)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// Summary information about a scheduled task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleInfo {
    /// Unique identifier
    pub id: String,
    /// Cron expression
    pub cron: String,
    /// Action type
    pub action: String,
    /// Next scheduled run time (ISO 8601 string)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    /// Description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the schedule is active
    pub active: bool,
}

/// Built-in cron tools wrapper for the tool executor
///
/// Provides tool definitions and dispatch for the LLM to schedule,
/// list, cancel, and inspect cron schedules
pub struct BuiltinCronTools {
    cron: CronTools,
}

impl BuiltinCronTools {
    /// Create a new set of built-in cron tools
    #[must_use]
    pub const fn new(cron: CronTools) -> Self {
        Self { cron }
    }

    /// Return tool definitions for all cron tools
    #[must_use]
    pub fn tool_definitions() -> Vec<synapse_client::ToolDefinition> {
        Self::core_definitions()
            .iter()
            .map(crate::tools::to_synapse_definition)
            .collect()
    }

    /// Return agent-core tool definitions (shared by trait and legacy path)
    fn core_definitions() -> Vec<agent_core::types::Tool> {
        vec![
            agent_core::types::Tool {
                name: "cron_schedule".to_string(),
                description: "Schedule a recurring task. Provide a cron expression, action type, and payload.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "cron": {
                            "type": "string",
                            "description": "Cron expression (e.g., '0 9 * * MON' for 9 AM every Monday)"
                        },
                        "action": {
                            "type": "string",
                            "description": "Action type to trigger (e.g., 'remind', 'check_in')"
                        },
                        "payload": {
                            "type": "object",
                            "description": "Arbitrary data to include in the callback"
                        },
                        "description": {
                            "type": "string",
                            "description": "Human-readable description of the schedule"
                        },
                        "timezone": {
                            "type": "string",
                            "description": "IANA timezone (e.g., 'America/New_York'). Defaults to UTC"
                        }
                    },
                    "required": ["cron", "action", "payload"]
                }),
            },
            agent_core::types::Tool {
                name: "cron_list".to_string(),
                description: "List all scheduled tasks with their cron expressions and next run times.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            agent_core::types::Tool {
                name: "cron_cancel".to_string(),
                description: "Cancel a scheduled task by its ID.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "schedule_id": {
                            "type": "string",
                            "description": "ID of the schedule to cancel"
                        }
                    },
                    "required": ["schedule_id"]
                }),
            },
            agent_core::types::Tool {
                name: "cron_get".to_string(),
                description: "Get details of a specific scheduled task by its ID.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "schedule_id": {
                            "type": "string",
                            "description": "ID of the schedule to retrieve"
                        }
                    },
                    "required": ["schedule_id"]
                }),
            },
        ]
    }

    /// Execute a named cron tool
    ///
    /// # Errors
    ///
    /// Returns error if arguments are malformed or the backend call fails
    pub async fn execute(&self, name: &str, arguments: &str) -> crate::Result<String> {
        self.dispatch(name, arguments).await
    }

    /// Internal dispatch for cron tool execution
    async fn dispatch(&self, name: &str, arguments: &str) -> crate::Result<String> {
        match name {
            "cron_schedule" => {
                let params: ScheduleParams = serde_json::from_str(arguments).map_err(|e| {
                    crate::Error::Tool(format!("cron_schedule: invalid arguments: {e}"))
                })?;
                let id = self.cron.schedule_with_options(params).await?;
                Ok(serde_json::json!({ "status": "scheduled", "id": id }).to_string())
            }
            "cron_list" => {
                let schedules = self.cron.list().await?;
                Ok(serde_json::json!({ "schedules": schedules }).to_string())
            }
            "cron_cancel" => {
                #[derive(serde::Deserialize)]
                struct CancelArgs {
                    schedule_id: String,
                }
                let args: CancelArgs = serde_json::from_str(arguments).map_err(|e| {
                    crate::Error::Tool(format!("cron_cancel: invalid arguments: {e}"))
                })?;
                self.cron.cancel(&args.schedule_id).await?;
                Ok(
                    serde_json::json!({ "status": "cancelled", "id": args.schedule_id })
                        .to_string(),
                )
            }
            "cron_get" => {
                #[derive(serde::Deserialize)]
                struct GetArgs {
                    schedule_id: String,
                }
                let args: GetArgs = serde_json::from_str(arguments)
                    .map_err(|e| crate::Error::Tool(format!("cron_get: invalid arguments: {e}")))?;
                let info = self.cron.get(&args.schedule_id).await?;
                Ok(serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string()))
            }
            _ => Err(crate::Error::Tool(format!("unknown cron tool: {name}"))),
        }
    }
}

#[async_trait::async_trait]
impl ToolProvider for BuiltinCronTools {
    fn definitions(&self) -> Vec<agent_core::types::Tool> {
        Self::core_definitions()
    }

    async fn execute(&self, name: &str, arguments: &str) -> anyhow::Result<String> {
        self.dispatch(name, arguments)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn kind(&self, name: &str) -> ToolKind {
        match name {
            "cron_list" | "cron_get" => ToolKind::Read,
            _ => ToolKind::Mutate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_info_serialization() {
        let info = ScheduleInfo {
            id: "sched_123".to_string(),
            cron: "0 9 * * MON".to_string(),
            action: "remind".to_string(),
            next_run: Some("2024-01-08T09:00:00Z".to_string()),
            description: Some("Weekly reminder".to_string()),
            active: true,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("sched_123"));
        assert!(json.contains("0 9 * * MON"));
    }

    #[test]
    fn cron_tool_definitions_count() {
        let defs = BuiltinCronTools::tool_definitions();
        assert_eq!(defs.len(), 4);
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(names.contains(&"cron_schedule"));
        assert!(names.contains(&"cron_list"));
        assert!(names.contains(&"cron_cancel"));
        assert!(names.contains(&"cron_get"));
    }

    #[test]
    fn test_schedule_params_deserialization() {
        let json = r#"{
            "cron": "0 9 * * MON",
            "action": "remind",
            "payload": { "message": "Test" }
        }"#;

        let params: ScheduleParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.cron, "0 9 * * MON");
        assert_eq!(params.action, "remind");
        assert!(params.description.is_none());
    }

    #[tokio::test]
    async fn local_backend_schedule_and_list() {
        let scheduler = Arc::new(LocalScheduler::new());
        let tools = CronTools::local(Arc::clone(&scheduler));

        let id = tools
            .schedule("0 9 * * MON", "remind", serde_json::json!({"msg": "hi"}))
            .await
            .unwrap();

        assert!(!id.is_empty());

        let list = tools.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
    }

    #[tokio::test]
    async fn local_backend_cancel_and_get() {
        let scheduler = Arc::new(LocalScheduler::new());
        let tools = CronTools::local(Arc::clone(&scheduler));

        let id = tools
            .schedule_with_options(ScheduleParams {
                cron: "0 12 * * *".to_string(),
                action: "check_in".to_string(),
                payload: serde_json::json!({}),
                description: Some("Daily check-in".to_string()),
                timezone: None,
            })
            .await
            .unwrap();

        let info = tools.get(&id).await.unwrap();
        assert_eq!(info.cron, "0 12 * * *");
        assert_eq!(info.description.as_deref(), Some("Daily check-in"));

        tools.cancel(&id).await.unwrap();
        let list = tools.list().await.unwrap();
        assert!(list.is_empty());
    }
}
