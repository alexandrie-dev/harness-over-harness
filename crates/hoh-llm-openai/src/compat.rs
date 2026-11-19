//! Table of server quirks, as data. Parsing is tolerant for every server; the table only
//! decides what is *sent* and which response fields are *read*.
//!
//! Sources of the entries (documented formats, not verified against live servers: README):
//! vLLM OpenAI-compatible server docs (reasoning outputs, `chat_template_kwargs`);
//! llama.cpp `tools/server/README.md` (`reasoning_format`, `chat_template_kwargs`);
//! Ollama `docs/openai.md` (`reasoning_effort`, `stream_options`);
//! SGLang OpenAI-compatible API docs (`reasoning_parser`, `chat_template_kwargs`).

/// Which server software sits behind `baseUrl`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ServerKind {
    /// Unknown OpenAI-compatible server: only standard fields are sent.
    #[default]
    Generic,
    Vllm,
    LlamaCpp,
    Ollama,
    Sglang,
    /// OpenAI itself and gateways that copy its newer API: the output limit is
    /// `max_completion_tokens`, `max_tokens` is refused by the newer models.
    OpenAi,
}

impl ServerKind {
    pub const ALL: [ServerKind; 6] = [
        Self::Generic,
        Self::Vllm,
        Self::LlamaCpp,
        Self::Ollama,
        Self::Sglang,
        Self::OpenAi,
    ];
    pub fn compat(self) -> &'static Compat {
        &COMPAT_TABLE[self as usize]
    }
}

/// How the request switches Qwen thinking on and off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinkingControl {
    /// The server has no known switch: the adapter offers no reasoning efforts.
    Unsupported,
    /// `chat_template_kwargs: {"enable_thinking": bool}` (Qwen3 chat template).
    ChatTemplateKwargs,
    /// `reasoning_effort: "none" | "low" | "medium" | "high"`.
    ReasoningEffort,
}

/// What the server does with tool call ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCallIds {
    /// A non-empty id that is unique within the response is kept; otherwise one is made up.
    Trust,
    /// The server's ids repeat between responses: always replaced by a fresh unique id.
    Synthesize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compat {
    pub kind: ServerKind,
    /// Request field for the output limit (`max_tokens` or `max_completion_tokens`). A server that
    /// refuses it is retried with the other one, see `error::rejects_limit_field`.
    pub max_tokens_field: &'static str,
    /// Whether `stream_options: {"include_usage": true}` is sent. Usage is read from whichever
    /// chunk carries it: the last chunk with `choices: []` (vLLM, SGLang, Ollama, llama.cpp
    /// current) or the finish chunk (older llama.cpp).
    pub stream_usage_option: bool,
    /// Delta fields that carry reasoning text, in order of preference.
    pub reasoning_fields: &'static [&'static str],
    pub thinking_control: ThinkingControl,
    pub tool_call_ids: ToolCallIds,
    /// Whether `tool_choice: "required"` is accepted.
    pub tool_choice_required: bool,
}

/// Indexed by `ServerKind as usize`; a test keeps the order honest.
pub static COMPAT_TABLE: [Compat; 6] = [
    Compat {
        kind: ServerKind::Generic,
        max_tokens_field: MAX_TOKENS,
        stream_usage_option: true,
        reasoning_fields: &["reasoning_content", "reasoning"],
        thinking_control: ThinkingControl::Unsupported,
        tool_call_ids: ToolCallIds::Trust,
        tool_choice_required: true,
    },
    Compat {
        kind: ServerKind::Vllm,
        max_tokens_field: MAX_TOKENS,
        stream_usage_option: true,
        // `reasoning_content` up to vLLM 0.10, `reasoning` after the rename.
        reasoning_fields: &["reasoning_content", "reasoning"],
        thinking_control: ThinkingControl::ChatTemplateKwargs,
        tool_call_ids: ToolCallIds::Trust,
        tool_choice_required: true,
    },
    Compat {
        kind: ServerKind::LlamaCpp,
        max_tokens_field: MAX_TOKENS,
        stream_usage_option: true,
        // `--reasoning-format deepseek` (default); `none` leaves `<think>` inside content.
        reasoning_fields: &["reasoning_content"],
        thinking_control: ThinkingControl::ChatTemplateKwargs,
        tool_call_ids: ToolCallIds::Trust,
        tool_choice_required: true,
    },
    Compat {
        kind: ServerKind::Ollama,
        max_tokens_field: MAX_TOKENS,
        stream_usage_option: true,
        reasoning_fields: &["reasoning", "reasoning_content"],
        thinking_control: ThinkingControl::ReasoningEffort,
        tool_call_ids: ToolCallIds::Trust,
        tool_choice_required: false,
    },
    Compat {
        kind: ServerKind::Sglang,
        max_tokens_field: MAX_TOKENS,
        stream_usage_option: true,
        reasoning_fields: &["reasoning_content", "reasoning"],
        thinking_control: ThinkingControl::ChatTemplateKwargs,
        // Index-derived ids (`call_0`) are reported for some versions.
        tool_call_ids: ToolCallIds::Synthesize,
        tool_choice_required: true,
    },
    Compat {
        kind: ServerKind::OpenAi,
        max_tokens_field: MAX_COMPLETION_TOKENS,
        stream_usage_option: true,
        // OpenAI does not return reasoning text; gateways in front of other models may.
        reasoning_fields: &["reasoning_content", "reasoning"],
        thinking_control: ThinkingControl::Unsupported,
        tool_call_ids: ToolCallIds::Trust,
        tool_choice_required: true,
    },
];

pub const MAX_TOKENS: &str = "max_tokens";
pub const MAX_COMPLETION_TOKENS: &str = "max_completion_tokens";

/// The other spelling of the output limit, for the retry after a server refuses one of them.
pub fn other_limit_field(field: &str) -> &'static str {
    if field == MAX_COMPLETION_TOKENS {
        MAX_TOKENS
    } else {
        MAX_COMPLETION_TOKENS
    }
}

/// Reasoning effort ids offered to the agent for a thinking control, as `(id, display name)`.
pub fn effort_ids(control: ThinkingControl) -> &'static [(&'static str, &'static str)] {
    match control {
        ThinkingControl::Unsupported => &[],
        ThinkingControl::ChatTemplateKwargs => &[("off", "Off"), ("on", "On")],
        ThinkingControl::ReasoningEffort => &[
            ("off", "Off"),
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
        ],
    }
}
