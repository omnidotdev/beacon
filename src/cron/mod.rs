//! Local cron scheduling
//!
//! Provides an in-process cron scheduler as a fallback when the Vortex
//! scheduling service is not configured

mod scheduler;

pub use scheduler::{CronAction, CronJob, LocalScheduler};
