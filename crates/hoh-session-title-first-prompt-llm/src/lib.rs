//! First-human-message LLM provider for session titles.

use std::{num::NonZeroUsize, sync::Arc};

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{Llm, LlmResolvedModelInfo, ReasoningEffortId};
use hoh_session::Sessions;
use hoh_session_title::{
    SessionTitle, SessionTitleAutomaticMode, SessionTitleProvider, SessionTitleProviderId,
    SessionTitleProviderRequest, SessionTitleProviderResult, SessionTitleUserMessage,
    normalize_session_title,
};
use hoh_session_title_llm::{
    ResolvedSessionTitleLlmConfig, SessionTitleLlmConfig, SessionTitleLlmFinish,
    SessionTitleLlmPreparedRequest, SessionTitleLlmResponse, execute_session_title_llm,
    resolve_session_title_llm_config, resolve_session_title_llm_config_value,
};
use hoh_util_values::{js_number_string, js_trim};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Loader-visible plugin and provider id.
pub const NAME: &str = "session-title-first-prompt-llm";

/// Required execution controls plus provider-owned title-length targets.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[schemars(range(min = 1), extend("multipleOf" = 1))]
    pub target_words: f64,
    /// Optional content-word ceiling for titles written with spaces between words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_words: Option<NonZeroUsize>,
    #[schemars(range(min = 1), extend("multipleOf" = 1))]
    pub target_cjk_characters: f64,
    #[serde(flatten)]
    pub execution: SessionTitleLlmConfig,
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        resolve_config_value(&value).map_err(serde::de::Error::custom)
    }
}

fn number(object: &serde_json::Map<String, Value>, key: &str) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn assert_target(name: &str, value: f64) -> Result<f64, BoxError> {
    if !value.is_finite() || value <= 0.0 || value.fract() != 0.0 {
        return Err(
            format!("session-title-first-prompt-llm: {name} must be a positive integer").into(),
        );
    }
    Ok(value)
}

fn resolve_config_value(value: &Value) -> Result<Config, BoxError> {
    let object = value
        .as_object()
        .ok_or_else(|| BoxError::from("session-title-llm: configuration is required"))?;
    let mut execution = object.clone();
    execution.shift_remove("targetWords");
    execution.shift_remove("targetCjkCharacters");
    let max_words = execution.shift_remove("maxWords");
    let execution = Value::Object(execution);
    resolve_session_title_llm_config_value(&execution)?;
    let target_words = assert_target("targetWords", number(object, "targetWords"))?;
    let target_cjk_characters =
        assert_target("targetCjkCharacters", number(object, "targetCjkCharacters"))?;
    Ok(Config {
        target_words,
        max_words: serde_json::from_value(max_words.unwrap_or(Value::Null))?,
        target_cjk_characters,
        execution: serde_json::from_value(execution)?,
    })
}

#[derive(Clone)]
struct ResolvedConfig {
    execution: ResolvedSessionTitleLlmConfig,
    target_words: f64,
    max_words: Option<NonZeroUsize>,
    target_cjk_characters: f64,
}

impl ResolvedConfig {
    fn new(config: &Config) -> Result<Self, BoxError> {
        Ok(Self {
            execution: resolve_session_title_llm_config(&config.execution)?,
            target_words: assert_target("targetWords", config.target_words)?,
            max_words: config.max_words,
            target_cjk_characters: assert_target(
                "targetCjkCharacters",
                config.target_cjk_characters,
            )?,
        })
    }
}

fn select_reasoning_effort(model: &LlmResolvedModelInfo) -> Option<ReasoningEffortId> {
    model
        .reasoning
        .as_ref()?
        .efforts
        .first()
        .map(|effort| effort.id.clone())
}

fn system_prompt(config: &ResolvedConfig) -> String {
    let word_limit = match config.max_words {
        Some(max_words) => format!(" with a maximum of {max_words} content words"),
        None => String::new(),
    };
    let connectors = if config.max_words.is_some() {
        " Keep necessary connecting words."
    } else {
        ""
    };
    [
        "Create a concise title for an AI coding-assistant session from the supplied human messages.".into(),
        "Return only the title on one line, **in plain text of natural language**, with no quotes, prefix, explanation, Markdown, XML, or terminal control codes. No code is allowed.".into(),
        "Use the language of the messages.".into(),
        "Name the specific subject or intended outcome so the user can recognize the session later. Prefer a compact noun phrase or action phrase. Leave out greetings, workflow details, and claims that work is complete.".into(),
        format!(
            "Aim for about {} words in non-CJK languages{} or {} CJK characters.{connectors}",
            js_number_string(config.target_words),
            word_limit,
            js_number_string(config.target_cjk_characters)
        ),
        "If the messages give little to name, still return a short best-effort title, such as Greeting, instead of explaining.".into(),
    ]
    .join("\n")
}

fn frame_messages(messages: &[SessionTitleUserMessage]) -> Result<String, BoxError> {
    Ok(format!(
        "Generate the session title from this JSON array of human messages:\n{}",
        serde_json::to_string(messages)?
    ))
}

fn emphasis_inner(line: &str) -> Option<&str> {
    for marker_len in (1..=3).rev() {
        let marker = "*".repeat(marker_len);
        let Some(inner) = line
            .strip_prefix(&marker)
            .and_then(|rest| rest.strip_suffix(&marker))
        else {
            continue;
        };
        if inner.is_empty()
            || inner
                .chars()
                .next()
                .is_none_or(hoh_util_values::is_js_white_space)
            || inner
                .chars()
                .next_back()
                .is_none_or(hoh_util_values::is_js_white_space)
        {
            continue;
        }
        if inner
            .chars()
            .any(|character| matches!(character, '\r' | '\u{2028}' | '\u{2029}'))
        {
            return None;
        }
        return (!inner.contains(&marker)).then_some(inner);
    }
    None
}

fn title_from_output(text: &str) -> String {
    let line = text
        .split('\n')
        .map(|item| js_trim(item.strip_suffix('\r').unwrap_or(item)))
        .find(|item| !item.is_empty())
        .unwrap_or_default();
    emphasis_inner(line).unwrap_or(line).into()
}

fn is_title_connector(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "a" | "an"
            | "the"
            | "and"
            | "or"
            | "of"
            | "to"
            | "for"
            | "with"
            | "in"
            | "on"
            | "at"
            | "from"
            | "by"
            | "about"
            | "before"
            | "after"
            | "during"
            | "à"
            | "au"
            | "aux"
            | "avec"
            | "chez"
            | "dans"
            | "de"
            | "des"
            | "du"
            | "en"
            | "et"
            | "la"
            | "le"
            | "les"
            | "par"
            | "pour"
            | "sans"
            | "sous"
            | "sur"
            | "un"
            | "une"
            | "в"
            | "во"
            | "на"
            | "и"
            | "или"
            | "с"
            | "со"
            | "к"
            | "ко"
            | "по"
            | "из"
            | "для"
            | "без"
            | "от"
            | "про"
            | "при"
            | "о"
            | "об"
            | "обо"
            | "у"
            | "за"
            | "до"
            | "через"
            | "после"
            | "перед"
            | "между"
            | "над"
            | "под"
    )
}

fn is_english_language_hint(word: &str) -> bool {
    matches!(
        word,
        "the" | "and" | "with" | "from" | "about" | "before" | "after" | "during" | "very"
    )
}

fn has_supported_source_hint(text: &str) -> bool {
    let mut words = text
        .split(|character: char| !character.is_alphabetic())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase);
    let Some(first) = words.next() else {
        return false;
    };
    matches!(
        first.as_str(),
        "add"
            | "build"
            | "change"
            | "check"
            | "create"
            | "explain"
            | "fix"
            | "improve"
            | "implement"
            | "please"
            | "refactor"
            | "remove"
            | "rename"
            | "update"
            | "добавь"
            | "исправь"
            | "настрой"
            | "обнови"
            | "проверь"
            | "сделай"
            | "создай"
            | "ускорь"
    ) || is_english_language_hint(&first)
        || words.any(|word| is_english_language_hint(&word))
}

fn has_supported_connector_language(word: &str) -> bool {
    let word = word.to_lowercase();
    is_english_language_hint(&word)
        || matches!(
            word.as_str(),
            "au" | "aux"
                | "avec"
                | "chez"
                | "du"
                | "pour"
                | "sans"
                | "sous"
                | "или"
                | "для"
                | "через"
                | "после"
                | "перед"
                | "между"
        )
        || (word.chars().all(|ch| matches!(ch, 'а'..='я' | 'ё'))
            && (word.contains("ческих") || word.contains(['ё', 'ы', 'э'])))
}

fn title_from_response(
    response: &SessionTitleLlmResponse,
    max_words: Option<NonZeroUsize>,
    source: &str,
) -> Result<String, BoxError> {
    match &response.finish {
        SessionTitleLlmFinish::Stop(_) => {}
        SessionTitleLlmFinish::ToolCalls(_) => {
            return Err("session-title-llm: title output must contain text only".into());
        }
        SessionTitleLlmFinish::MaxTokens(_) => {
            return Err("session-title-llm: title output reached maxOutputTokens".into());
        }
    }
    if response
        .blocks
        .iter()
        .any(|block| block.r#type == "tool-call")
    {
        return Err("session-title-llm: title output must contain text only".into());
    }
    let text = response
        .blocks
        .iter()
        .filter(|block| block.r#type == "text")
        .filter_map(|block| block.text_value())
        .collect::<Vec<_>>()
        .join(" ");
    let title =
        normalize_session_title(&title_from_output(&text), usize::MAX).map_err(BoxError::from)?;
    if title.is_empty() {
        return Err("session-title-llm: title model produced no text".into());
    }
    if let Some(max_words) = max_words {
        if title.chars().count() > 48
            || title.split_whitespace().count() > max_words.get().saturating_mul(2)
        {
            return Err("session-title-llm: title output exceeds the short title limit".into());
        }
        let supported = title
            .split_whitespace()
            .any(has_supported_connector_language)
            || has_supported_source_hint(source);
        if supported
            && title
                .split_whitespace()
                .filter(|word| !is_title_connector(word))
                .count()
                > max_words.get()
        {
            return Err(format!(
                "session-title-llm: title output exceeds {max_words} content words"
            )
            .into());
        }
    }
    Ok(title)
}

struct FirstPromptProvider {
    ctx: Context,
    id: SessionTitleProviderId,
    config: ResolvedConfig,
}

impl SessionTitleProvider for FirstPromptProvider {
    fn id(&self) -> &SessionTitleProviderId {
        &self.id
    }

    fn automatic(&self) -> SessionTitleAutomaticMode {
        SessionTitleAutomaticMode::FirstPrompt
    }

    fn generate(
        &self,
        request: SessionTitleProviderRequest,
    ) -> BoxFuture<'static, Result<SessionTitleProviderResult, BoxError>> {
        let ctx = self.ctx.clone();
        let id = self.id.clone();
        let config = self.config.clone();
        Box::pin(async move {
            let first = request.messages.first().cloned().ok_or_else(|| {
                BoxError::from("first-prompt title provider requires one human message")
            })?;
            let response = execute_session_title_llm(
                &ctx,
                &config.execution,
                &request,
                id,
                &SessionTitleLlmPreparedRequest {
                    system: system_prompt(&config),
                    input: frame_messages(std::slice::from_ref(&first))?,
                    message_seqs: vec![first.seq],
                    select_reasoning_effort: Arc::new(select_reasoning_effort),
                },
            )
            .await?;
            Ok(SessionTitleProviderResult {
                title: title_from_response(&response, config.max_words, &first.text)?,
                message_seqs: vec![first.seq],
                model: Some(response.model),
            })
        })
    }
}

#[derive(Inject)]
pub struct Deps {
    session_title: Service<SessionTitle>,
    _llm: Service<Llm>,
    _sessions: Service<Sessions>,
}

/// Register the first-prompt title provider.
pub struct SessionTitleFirstPromptLlm;

impl Plugin for SessionTitleFirstPromptLlm {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let provider = Arc::new(FirstPromptProvider {
            ctx: ctx.clone(),
            id: NAME.into(),
            config: ResolvedConfig::new(&config.get())?,
        });
        deps.session_title.register(&ctx, provider)?;
        Ok(())
    }
}

/// Public alias matching the package's default plugin export.
pub type SessionTitleFirstPromptLlmPlugin = SessionTitleFirstPromptLlm;

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;
    use hoh_session::Session;
    use hoh_timeout::AbortController;

    #[tokio::test]
    async fn rejects_an_impossible_empty_provider_request_at_its_boundary() {
        let kernel = Kernel::start();
        let provider = FirstPromptProvider {
            ctx: kernel.root(),
            id: NAME.into(),
            config: ResolvedConfig::new(&Config {
                target_words: 5.0,
                max_words: None,
                target_cjk_characters: 10.0,
                execution: SessionTitleLlmConfig {
                    max_input_bytes: 1_000.0,
                    max_output_tokens: 32.0,
                    timeout_ms: 1_000.0,
                    provider: Some("title-route".into()),
                    model: Some("title-model".into()),
                },
            })
            .unwrap(),
        };
        let error = provider
            .generate(SessionTitleProviderRequest {
                session: Session::create("empty-first-provider".into(), None, None, None, None)
                    .unwrap(),
                messages: vec![],
                route: None,
                current_title: None,
                signal: AbortController::new().signal(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "first-prompt title provider requires one human message"
        );
        kernel.shutdown().await;
    }

    #[test]
    fn typed_provider_policy_preserves_the_source_validation_error() {
        let result = ResolvedConfig::new(&Config {
            target_words: 0.0,
            max_words: None,
            target_cjk_characters: 10.0,
            execution: SessionTitleLlmConfig {
                max_input_bytes: 1_000.0,
                max_output_tokens: 32.0,
                timeout_ms: 1_000.0,
                provider: Some("title-route".into()),
                model: Some("title-model".into()),
            },
        });
        let Err(error) = result else {
            panic!("invalid targetWords was accepted");
        };
        assert_eq!(
            error.to_string(),
            "session-title-first-prompt-llm: targetWords must be a positive integer"
        );
    }
}
