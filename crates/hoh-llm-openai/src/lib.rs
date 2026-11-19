//! Model adapter for a local OpenAI-compatible Chat Completions server.
//!
//! New HoH code, no dsh source. It plugs into `hoh-llm` where dsh's DeepSeek adapter does:
//! it implements [`hoh_llm::LlmAdapter`] and produces `StreamChunk`s.
pub mod adapter;
pub mod compat;
pub mod config;
pub mod error;
pub mod http;
pub mod probe;
pub mod request;
pub mod sse;
pub mod stream;
pub mod think;
pub mod tool_check;

pub use adapter::OpenAiAdapter;
pub use compat::{Compat, ServerKind, ThinkingControl, ToolCallIds};
pub use config::{ModelSettings, OpenAiConfig, ReasoningFormat, ToolChoice};
pub use probe::{ProbeError, ProbeOptions, ProbeResult, probe};
