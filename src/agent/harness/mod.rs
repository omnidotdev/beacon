//! Harness adapter system for delegating agent execution to external CLI tools

pub mod claude_cli;
pub mod runner;
pub mod session;
pub mod types;

pub use claude_cli::ClaudeCliAdapter;
pub use runner::{AdapterRegistry, HarnessTurnDeps, run_harness_turn};
pub use session::HarnessSessionRepo;
pub use types::{
    Execution, HarnessAdapter, HarnessConfig, HarnessOutput, HarnessUsage, SessionMode,
};
