//! Declarative manager for a scoped set of MCP client instances.
//!
//! Configuration replacement, explicit restarts, and registration teardown
//! pass through one actor so two connection generations never overlap.

mod config;
mod runtime;
mod types;

pub use config::{Config, McpManagedServerEntry, McpTransport};
pub use runtime::{McpManager, McpManagerError, McpServerManager};
pub use types::{McpManagedServerStatus, McpManagedServerView, McpManagerStatus};

pub const NAME: &str = "mcp-manager";
