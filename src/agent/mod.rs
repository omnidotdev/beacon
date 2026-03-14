//! Agentic runner — shared logic for proactive and WebSocket-driven turns

pub mod binding;
pub mod registry;
pub mod runner;

pub use binding::{AgentBinding, BindingRouter};
pub use registry::{AgentConfig, AgentId, AgentRegistry};
pub use runner::{AgentNotifyEvent, AgentRunConfig, run_agent_turn};
