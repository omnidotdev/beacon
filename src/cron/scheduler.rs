//! Local in-process cron scheduler
//!
//! Uses `croner` for cron expression parsing and provides a background task
//! that checks jobs every 30 seconds

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use croner::Cron;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{Error, Result};

/// A scheduled cron job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique identifier
    pub id: String,
    /// Cron expression (e.g., "0 9 * * MON")
    pub schedule: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Action to perform when the job fires
    pub action: CronAction,
    /// IANA timezone (defaults to UTC)
    pub timezone: Option<String>,
    /// When the job was created
    pub created_at: DateTime<Utc>,
}

/// Action to perform when a cron job fires
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CronAction {
    /// Send a prompt to an agent on a channel
    AgentPrompt {
        /// Prompt text
        prompt: String,
        /// Target channel
        channel: String,
    },
    /// Send an HTTP webhook
    Webhook {
        /// Target URL
        url: String,
        /// Optional JSON payload
        payload: Option<serde_json::Value>,
    },
}

/// In-process cron scheduler
///
/// Stores jobs in memory and runs a background task to check for
/// jobs that need to fire
pub struct LocalScheduler {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    running: Arc<AtomicBool>,
}

impl std::fmt::Debug for LocalScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalScheduler")
            .field("jobs", &"<RwLock>")
            .field("running", &self.running.load(Ordering::Relaxed))
            .finish()
    }
}

impl LocalScheduler {
    /// Create a new empty scheduler
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Schedule a new cron job
    ///
    /// Validates the cron expression, creates a job with a unique ID,
    /// and stores it for execution by the background task.
    ///
    /// # Errors
    ///
    /// Returns an error if the cron expression is invalid
    pub async fn schedule(
        &self,
        schedule_expr: &str,
        action: CronAction,
        description: Option<String>,
        timezone: Option<String>,
    ) -> Result<CronJob> {
        // Validate cron expression
        Cron::new(schedule_expr)
            .parse()
            .map_err(|e| Error::Tool(format!("invalid cron expression: {e}")))?;

        let job = CronJob {
            id: uuid::Uuid::new_v4().to_string(),
            schedule: schedule_expr.to_string(),
            description,
            action,
            timezone,
            created_at: Utc::now(),
        };

        self.jobs.write().await.insert(job.id.clone(), job.clone());

        tracing::info!(job_id = %job.id, schedule = %schedule_expr, "cron job scheduled");

        Ok(job)
    }

    /// Cancel a scheduled job by ID
    ///
    /// # Errors
    ///
    /// Returns an error if the job ID is not found
    pub async fn cancel(&self, job_id: &str) -> Result<()> {
        let removed = self.jobs.write().await.remove(job_id);

        if removed.is_some() {
            tracing::info!(job_id = %job_id, "cron job cancelled");
            Ok(())
        } else {
            Err(Error::NotFound(format!("cron job not found: {job_id}")))
        }
    }

    /// List all scheduled jobs
    pub async fn list(&self) -> Vec<CronJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    /// Get a specific job by ID
    pub async fn get(&self, job_id: &str) -> Option<CronJob> {
        self.jobs.read().await.get(job_id).cloned()
    }

    /// Start the background scheduler task
    ///
    /// Spawns a tokio task that checks all jobs every 30 seconds
    /// and calls the callback for any that should have fired since
    /// the last check.
    pub fn start<F>(&self, callback: F)
    where
        F: Fn(CronJob) + Send + Sync + 'static,
    {
        self.running.store(true, Ordering::Relaxed);

        let jobs = Arc::clone(&self.jobs);
        let running = Arc::clone(&self.running);
        let callback = Arc::new(callback);

        tokio::spawn(async move {
            let mut last_check = Utc::now();

            while running.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;

                if !running.load(Ordering::Relaxed) {
                    break;
                }

                let now = Utc::now();
                let jobs_snapshot = jobs.read().await.clone();

                for job in jobs_snapshot.values() {
                    let Ok(cron) = Cron::new(&job.schedule).parse() else {
                        continue;
                    };

                    // Check if the job should have fired between last_check and now
                    if let Ok(next) = cron.find_next_occurrence(&last_check, false)
                        && next <= now
                    {
                        tracing::info!(
                            job_id = %job.id,
                            schedule = %job.schedule,
                            "cron job triggered"
                        );
                        callback(job.clone());
                    }
                }

                last_check = now;
            }

            tracing::info!("local cron scheduler stopped");
        });

        tracing::info!("local cron scheduler started");
    }

    /// Stop the background scheduler
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Default for LocalScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schedule_validates_cron() {
        let scheduler = LocalScheduler::new();
        let result = scheduler
            .schedule(
                "0 9 * * MON",
                CronAction::Webhook {
                    url: "https://example.com".to_string(),
                    payload: None,
                },
                Some("Weekly check".to_string()),
                None,
            )
            .await;

        assert!(result.is_ok());
        let job = result.unwrap();
        assert_eq!(job.schedule, "0 9 * * MON");
        assert_eq!(job.description.as_deref(), Some("Weekly check"));
    }

    #[tokio::test]
    async fn schedule_rejects_invalid_cron() {
        let scheduler = LocalScheduler::new();
        let result = scheduler
            .schedule(
                "not a cron expression",
                CronAction::Webhook {
                    url: "https://example.com".to_string(),
                    payload: None,
                },
                None,
                None,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_returns_all_jobs() {
        let scheduler = LocalScheduler::new();

        for expr in &["0 9 * * MON", "0 12 * * *", "30 8 1 * *"] {
            scheduler
                .schedule(
                    expr,
                    CronAction::Webhook {
                        url: "https://example.com".to_string(),
                        payload: None,
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        let jobs = scheduler.list().await;
        assert_eq!(jobs.len(), 3);
    }

    #[tokio::test]
    async fn cancel_removes_job() {
        let scheduler = LocalScheduler::new();

        let job = scheduler
            .schedule(
                "0 9 * * MON",
                CronAction::Webhook {
                    url: "https://example.com".to_string(),
                    payload: None,
                },
                None,
                None,
            )
            .await
            .unwrap();

        scheduler.cancel(&job.id).await.unwrap();
        let jobs = scheduler.list().await;
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn get_returns_job() {
        let scheduler = LocalScheduler::new();

        let job = scheduler
            .schedule(
                "0 9 * * MON",
                CronAction::AgentPrompt {
                    prompt: "Good morning!".to_string(),
                    channel: "general".to_string(),
                },
                Some("Morning greeting".to_string()),
                None,
            )
            .await
            .unwrap();

        let found = scheduler.get(&job.id).await;
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.id, job.id);
        assert_eq!(found.schedule, "0 9 * * MON");
    }

    #[tokio::test]
    async fn cancel_nonexistent_returns_error() {
        let scheduler = LocalScheduler::new();
        let result = scheduler.cancel("nonexistent-id").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let scheduler = LocalScheduler::new();
        let result = scheduler.get("nonexistent-id").await;
        assert!(result.is_none());
    }
}
