use crate::compat::{Compat, ServerKind};
use hoh_error::HarnessError;
use hoh_llm::BuiltinLlmFailureCode;

/// Where reasoning text is expected. Delta fields (`reasoning_content`, `reasoning`) are
/// always honored; the setting only decides whether `<think>` tags inside content are parsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReasoningFormat {
    /// Delta fields, plus `<think>…</think>` inside content.
    #[default]
    Auto,
    /// Delta fields only; content is passed through verbatim.
    ReasoningField,
    /// `<think>…</think>` inside content (llama.cpp `--reasoning-format none`, older Ollama).
    ThinkTags,
    /// Content starts already inside the thinking block and only `</think>` appears: the Qwen3
    /// thinking templates put `<think>` into the prompt. Never guessed by `Auto`.
    ThinkTagsImplicitOpen,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ToolChoice {
    #[default]
    Auto,
    Required,
    None,
    Function(String),
}

/// What the adapter reports about the one model the server serves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelSettings {
    pub context_window: Option<u64>,
    pub default_max_tokens: Option<u64>,
    /// Images are sent as data URLs only when this is set; otherwise the runtime replaces them
    /// by a text placeholder before the adapter sees them.
    pub image_input: bool,
}

#[derive(Clone, PartialEq)]
pub struct OpenAiConfig {
    /// For example `http://127.0.0.1:8080/v1`. `/chat/completions` and `/models` are appended.
    pub base_url: String,
    pub api_key: Option<String>,
    pub provider_name: String,
    pub server: ServerKind,
    pub reasoning_format: ReasoningFormat,
    pub tool_choice: ToolChoice,
    /// Invalid tool arguments, unknown tool names and missing required properties fail the
    /// response with `MALFORMED_RESPONSE`. When false the raw arguments pass through.
    pub strict_tool_arguments: bool,
    pub model: ModelSettings,
    pub connect_timeout_ms: u64,
    /// Longest silence on the connection (headers or body bytes, SSE comments included).
    /// A local model may spend minutes on prompt processing before the first token.
    pub stream_idle_timeout_ms: u64,
}

/// Written by hand: a derived `Debug` would print the API key into logs and test failures.
impl std::fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field("provider_name", &self.provider_name)
            .field("server", &self.server)
            .field("reasoning_format", &self.reasoning_format)
            .field("tool_choice", &self.tool_choice)
            .field("strict_tool_arguments", &self.strict_tool_arguments)
            .field("model", &self.model)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("stream_idle_timeout_ms", &self.stream_idle_timeout_ms)
            .finish()
    }
}

impl OpenAiConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            provider_name: "OpenAI-compatible".into(),
            server: ServerKind::Generic,
            reasoning_format: ReasoningFormat::Auto,
            tool_choice: ToolChoice::Auto,
            strict_tool_arguments: true,
            model: ModelSettings::default(),
            connect_timeout_ms: 10_000,
            stream_idle_timeout_ms: 300_000,
        }
    }
    pub fn compat(&self) -> &'static Compat {
        self.server.compat()
    }
    /// Base URL without a trailing slash; scheme must be http or https.
    pub fn normalized_base_url(&self) -> Result<String, HarnessError> {
        normalize_base_url(&self.base_url)
    }
    pub fn validate(&self) -> Result<(), HarnessError> {
        self.normalized_base_url()?;
        if self.tool_choice == ToolChoice::Required && !self.compat().tool_choice_required {
            return Err(config_error(format!(
                "tool_choice \"required\" is not supported by {:?}",
                self.server
            )));
        }
        if self.stream_idle_timeout_ms == 0 || self.connect_timeout_ms == 0 {
            return Err(config_error("timeouts must be positive".into()));
        }
        Ok(())
    }
}

pub fn normalize_base_url(raw: &str) -> Result<String, HarnessError> {
    let url = url::Url::parse(raw.trim())
        .map_err(|e| config_error(format!("baseUrl {raw:?} is not a URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(config_error(format!(
            "baseUrl {raw:?} must be an http or https URL with a host"
        )));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub(crate) fn config_error(message: String) -> HarnessError {
    crate::error::failure(BuiltinLlmFailureCode::InvalidConfig, message)
}
