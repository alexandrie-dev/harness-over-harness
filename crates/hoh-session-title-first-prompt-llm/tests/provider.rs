use std::sync::{Arc, Mutex};

use hoh_agent::AgentRegistryPlugin;
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{
    ContentBlock, FinishReason, GenerateOptions, Llm, LlmAdapter, LlmFuture, LlmModelInfo,
    LlmModelReasoningInfo, LlmPlugin, LlmReasoningEffortInfo, LlmResolvedModelInfo, LlmStream,
    MessageSource, NewUserMessage, ReasoningEffortId, StreamChunk, create_user_message,
};
use hoh_session::{
    CreateSessionOptions, Session, SessionPlugin, SessionStore, Sessions, SurfaceIntent,
};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_session_title::{
    SessionTitle, SessionTitlePlugin, SessionTitleService, SessionTitleSource,
};
use hoh_session_title_first_prompt_llm::{Config, NAME, SessionTitleFirstPromptLlm};
use schemars::schema_for;
use serde_json::{Value, json};

const SYSTEM: &str = concat!(
    "Create a concise title for an AI coding-assistant session from the supplied human messages.\n",
    "Return only the title on one line, **in plain text of natural language**, with no quotes, prefix, explanation, Markdown, XML, or terminal control codes. No code is allowed.\n",
    "Use the language of the messages.\n",
    "Name the specific subject or intended outcome so the user can recognize the session later. Prefer a compact noun phrase or action phrase. Leave out greetings, workflow details, and claims that work is complete.\n",
    "Aim for about 5 words in non-CJK languages or 10 CJK characters.\n",
    "If the messages give little to name, still return a short best-effort title, such as Greeting, instead of explaining."
);

fn first_title_script() -> Vec<StreamChunk> {
    vec![
        StreamChunk::TextDelta {
            index: 0,
            text: "First-message model title".into(),
            wire: Default::default(),
        },
        StreamChunk::finish(FinishReason::new("stop")),
    ]
}

struct RecordingAdapter {
    script: Vec<StreamChunk>,
    requests: Mutex<Vec<GenerateOptions>>,
    efforts: Vec<ReasoningEffortId>,
}

impl RecordingAdapter {
    fn new(script: Vec<StreamChunk>, efforts: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            script,
            requests: Mutex::new(vec![]),
            efforts: efforts.iter().map(|effort| (*effort).into()).collect(),
        })
    }
}

impl LlmAdapter for RecordingAdapter {
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        let efforts = self.efforts.clone();
        Box::pin(async move {
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: model.into(),
                    ..Default::default()
                },
                reasoning: (!efforts.is_empty()).then(|| LlmModelReasoningInfo {
                    efforts: efforts
                        .into_iter()
                        .map(|id| LlmReasoningEffortInfo {
                            name: id.to_string(),
                            id,
                            description: None,
                        })
                        .collect(),
                    default_effort: None,
                }),
                ..Default::default()
            })
        })
    }

    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        self.requests.lock().unwrap().push(options);
        Ok(LlmStream::chunks(self.script.clone()))
    }
}

struct Env {
    kernel: Kernel,
    ctx: Context,
    sessions: SessionStore,
    titles: SessionTitleService,
    adapter: Arc<RecordingAdapter>,
}

impl Env {
    async fn shutdown(self) {
        self.kernel.shutdown().await;
    }
}

async fn setup(script: Vec<StreamChunk>, efforts: &[&str]) -> Env {
    setup_with_route(
        script,
        efforts,
        Some(("title-route", "title-model")),
        "title-route",
        32,
        None,
    )
    .await
}

async fn setup_with_route(
    script: Vec<StreamChunk>,
    efforts: &[&str],
    route_override: Option<(&str, &str)>,
    adapter_provider: &str,
    max_output_tokens: u64,
    max_words: Option<u64>,
) -> Env {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<LlmPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionTitlePlugin>(json!({
        "fallbackMaxWords": max_words.unwrap_or(5),
        "fallbackMaxBytes": if max_words.is_some() { 80 } else { 40 },
        "maxTitleBytes": 80
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
    let adapter = RecordingAdapter::new(script, efforts);
    ctx.get::<Llm>()
        .unwrap()
        .register_adapter(&ctx, vec![adapter_provider.into()], adapter.clone())
        .unwrap();
    let mut provider_config = json!({
        "targetWords": max_words.unwrap_or(5),
        "targetCjkCharacters": 10,
        "maxInputBytes": 1000,
        "maxOutputTokens": max_output_tokens,
        "timeoutMs": 1000
    });
    if let Some((provider, model)) = route_override {
        provider_config["provider"] = provider.into();
        provider_config["model"] = model.into();
    }
    if let Some(max_words) = max_words {
        provider_config["maxWords"] = json!(max_words);
    }
    ctx.plugin::<SessionTitleFirstPromptLlm>(provider_config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    Env {
        sessions: (*ctx.get::<Sessions>().unwrap()).clone(),
        titles: (*ctx.get::<SessionTitle>().unwrap()).clone(),
        kernel,
        ctx,
        adapter,
    }
}

#[tokio::test]
async fn inherits_the_logged_route_but_selects_its_own_reasoning_floor() {
    let env = setup_with_route(
        first_title_script(),
        &["low", "high", "max"],
        None,
        "main-route",
        64,
        None,
    )
    .await;
    let session = session(&env, "inherited-route");
    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let first = append_human(&session, "inherit this request route");
    settle().await;
    session
        .append(
            "request/header",
            json!({
                "header": {
                    "config": {
                        "provider": "main-route",
                        "model": "main-model",
                        "reasoningEffort": "max"
                    }
                },
                "reason": "initial"
            }),
            None,
        )
        .unwrap();
    settle().await;
    env.titles.refresh(&session, None).await.unwrap();

    let requests = env.adapter.requests.lock().unwrap().clone();
    assert!(!requests.is_empty());
    for options in &requests {
        assert_eq!(options.provider, "main-route");
        assert_eq!(options.model, "main-model");
        assert_eq!(options.max_tokens, Some(64));
        assert_eq!(
            options.reasoning_effort.as_ref().map(ToString::to_string),
            Some("low".into())
        );
    }
    let event = session
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|event| event.event_type() == "session/title-llm-request")
        .unwrap()
        .data()
        .clone();
    assert_eq!(
        event["route"],
        json!({"provider":"main-route","model":"main-model"})
    );
    assert_eq!(event["maxTokens"], 64);
    assert_eq!(event["reasoningEffort"], "low");
    let title = env.titles.get(&session).unwrap();
    assert_eq!(title.message_seqs, vec![first]);
    assert!(matches!(
        title.source,
        SessionTitleSource::Provider { model: Some(model), .. }
            if model.provider == "main-route" && model.model == "main-model"
    ));
    env.shutdown().await;
}

#[tokio::test]
async fn omits_unselected_reasoning_effort_from_the_exact_request_event() {
    let env = setup(first_title_script(), &[]).await;
    let session = session(&env, "no-reasoning-event");
    let first = append_human(&session, "Room \"quote\" and 番茄");
    env.titles.refresh(&session, None).await.unwrap();

    let options = env.adapter.requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(options.reasoning_effort, None);
    let event = session
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|event| event.event_type() == "session/title-llm-request")
        .unwrap()
        .data()
        .clone();
    assert_eq!(
        event,
        json!({
            "titleProvider": NAME,
            "messageSeqs": [first],
            "route": {"provider":"title-route","model":"title-model"},
            "system": SYSTEM,
            "messages": options.messages,
            "maxTokens": 32
        })
    );
    env.shutdown().await;
}

fn session(env: &Env, id: &str) -> Session {
    env.sessions
        .create(&env.ctx, Some(id.into()), CreateSessionOptions::default())
        .unwrap()
}

fn append_human(session: &Session, text: &str) -> hoh_session::SessionSeq {
    let message = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    });
    session
        .append(
            "user/message",
            serde_json::to_value(message).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap()
        .seq()
}

async fn settle() {
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}

fn chunks(values: impl IntoIterator<Item = Value>) -> Vec<StreamChunk> {
    values
        .into_iter()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect()
}

#[test]
fn validates_complete_config_schema_and_exact_errors() {
    let schema = serde_json::to_value(schema_for!(Config)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["required"],
        json!([
            "targetWords",
            "targetCjkCharacters",
            "maxInputBytes",
            "maxOutputTokens",
            "timeoutMs"
        ])
    );
    assert_eq!(schema["properties"]["targetWords"]["minimum"], 1);
    assert_eq!(schema["properties"]["targetWords"]["multipleOf"], 1);

    let base = json!({
        "targetWords": 5,
        "targetCjkCharacters": 10,
        "maxInputBytes": 1000,
        "maxOutputTokens": 32,
        "timeoutMs": 1000,
        "provider": "title-route",
        "model": "title-model"
    });
    let mut invalid = base.clone();
    invalid["targetWords"] = json!(0);
    assert!(
        serde_json::from_value::<Config>(invalid)
            .unwrap_err()
            .to_string()
            .contains("session-title-first-prompt-llm: targetWords must be a positive integer")
    );
    let mut invalid = base.clone();
    invalid["targetCjkCharacters"] = json!(1.5);
    assert!(
        serde_json::from_value::<Config>(invalid)
            .unwrap_err()
            .to_string()
            .contains(
                "session-title-first-prompt-llm: targetCjkCharacters must be a positive integer"
            )
    );
    let mut invalid = base;
    invalid["extra"] = json!(true);
    assert!(
        serde_json::from_value::<Config>(invalid)
            .unwrap_err()
            .to_string()
            .contains("session-title-llm: unknown config key \"extra\"")
    );
}

#[tokio::test]
async fn rejects_invalid_target_through_plugin_loading() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<LlmPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionTitlePlugin>(json!({
        "fallbackMaxWords": 5,
        "fallbackMaxBytes": 40,
        "maxTitleBytes": 80
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();

    let error = match ctx.plugin::<SessionTitleFirstPromptLlm>(json!({
        "targetWords": 0,
        "targetCjkCharacters": 10,
        "maxInputBytes": 1000,
        "maxOutputTokens": 32,
        "timeoutMs": 1000,
        "provider": "title-route",
        "model": "title-model"
    })) {
        Err(error) => error.to_string(),
        Ok(fiber) => fiber.settled().await.unwrap_err().to_string(),
    };
    assert_eq!(
        error,
        "invalid config:\n  - expected number >= 1 but got 0 (at targetWords)"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn registers_first_prompt_and_logs_only_the_first_message_with_reasoning_floor() {
    let env = setup(first_title_script(), &["low", "high"]).await;
    let session = session(&env, "first-plugin");
    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let first = append_human(&session, "Room \"quote\" and 番茄");
    settle().await;
    session
        .append(
            "request/header",
            json!({"header":{"config":{"provider":"main","model":"main-model"}},"reason":"initial"}),
            None,
        )
        .unwrap();
    settle().await;
    append_human(&session, "second input must be ignored");
    settle().await;
    env.titles.refresh(&session, None).await.unwrap();

    let expected_input = format!(
        "Generate the session title from this JSON array of human messages:\n[{{\"seq\":{},\"text\":\"Room \\\"quote\\\" and 番茄\"}}]",
        u64::from(first)
    );
    let requests = env.adapter.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    for options in requests.iter() {
        assert_eq!(options.provider, "title-route");
        assert_eq!(options.model, "title-model");
        assert_eq!(options.max_tokens, Some(32));
        assert_eq!(
            options.reasoning_effort.as_ref().map(ToString::to_string),
            Some("low".into())
        );
        assert_eq!(options.system.as_deref(), Some(SYSTEM));
        assert_eq!(
            options.messages[0].content()[0].text_value(),
            Some(expected_input.as_str())
        );
    }
    let options = requests.last().unwrap().clone();
    let logged = session
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|event| event.event_type() == "session/title-llm-request")
        .unwrap()
        .data()
        .clone();
    assert_eq!(
        logged,
        json!({
            "titleProvider": NAME,
            "messageSeqs": [first],
            "route": {"provider":"title-route","model":"title-model"},
            "system": SYSTEM,
            "messages": options.messages,
            "maxTokens": 32,
            "reasoningEffort": "low"
        })
    );
    let title = env.titles.get(&session).unwrap();
    assert_eq!(title.title, "First-message model title");
    assert_eq!(title.message_seqs, vec![first]);
    assert!(matches!(
        title.source,
        SessionTitleSource::Provider { provider, model: Some(model) }
            if provider.as_str() == NAME
                && model.provider == "title-route"
                && model.model == "title-model"
    ));
    env.shutdown().await;
}

#[tokio::test]
async fn takes_the_title_from_the_first_nonempty_output_line() {
    let cases: &[(&[&str], &str)] = &[
        (
            &[
                "**Continuing Previous",
                " Session**\n\nThe only message is \"continue\".",
            ],
            "Continuing Previous Session",
        ),
        (&["\n  *Greeting*  \n"], "Greeting"),
        (&["**a** and **b**\nnote"], "**a** and **b**"),
        (&["Use **bold** for emphasis"], "Use **bold** for emphasis"),
        (&["Fix *args* handling"], "Fix *args* handling"),
        (&["*args"], "*args"),
        (&["****"], "****"),
        (&["*a\u{2028}b*"], "*a b*"),
    ];
    for (index, (deltas, expected)) in cases.iter().enumerate() {
        let mut script = vec![json!({"type":"block-start","index":0,"blockType":"text"})];
        script.extend(
            deltas
                .iter()
                .map(|text| json!({"type":"text-delta","index":0,"text":text})),
        );
        script.push(json!({"type":"finish","reason":{"kind":"stop"}}));
        let env = setup(chunks(script), &[]).await;
        let session = session(&env, &format!("parse-{index}"));
        append_human(&session, "first input");
        let title = env.titles.refresh(&session, None).await.unwrap().unwrap();
        assert_eq!(title.title, *expected);
        env.shutdown().await;
    }
}

#[tokio::test]
async fn keeps_natural_connectors_within_the_content_word_limit() {
    let env = setup_with_route(
        chunks([
            json!({"type":"text-delta","index":0,"text":"Доступ к кабинету CEO"}),
            json!({"type":"finish","reason":{"kind":"stop"}}),
        ]),
        &[],
        Some(("title-route", "title-model")),
        "title-route",
        32,
        Some(3),
    )
    .await;
    let session = session(&env, "title-with-connector");
    append_human(&session, "У CEO не открывается кабинет");

    let title = env.titles.refresh(&session, None).await.unwrap().unwrap();
    assert_eq!(title.title, "Доступ к кабинету CEO");
    env.shutdown().await;
}

#[tokio::test]
async fn keeps_the_three_word_fallback_when_the_model_exceeds_the_configured_limit() {
    let env = setup_with_route(
        chunks([
            json!({"type":"text-delta","index":0,"text":"Проверка автоматических названий чатов"}),
            json!({"type":"finish","reason":{"kind":"stop"}}),
        ]),
        &[],
        Some(("title-route", "title-model")),
        "title-route",
        32,
        Some(3),
    )
    .await;
    let session = session(&env, "overlong-title");
    append_human(&session, "Сократи названия чатов до трёх слов");

    assert_eq!(
        env.titles
            .refresh(&session, None)
            .await
            .unwrap_err()
            .to_string(),
        "session-title-llm: title output exceeds 3 content words"
    );
    let title = env.titles.get(&session).unwrap();
    assert_eq!(title.title, "Сократи названия чатов");
    assert!(matches!(title.source, SessionTitleSource::Fallback));
    let system = env.adapter.requests.lock().unwrap()[0]
        .system
        .clone()
        .unwrap();
    assert!(system.contains("maximum of 3 content words"));
    env.shutdown().await;
}

#[tokio::test]
async fn rejects_nontext_terminal_results_tool_blocks_and_empty_text() {
    let cases = [
        (
            chunks([
                json!({"type":"text-delta","index":0,"text":"Title from text"}),
                json!({"type":"finish","reason":{"kind":"tool-calls"}}),
            ]),
            "session-title-llm: title output must contain text only",
        ),
        (
            chunks([
                json!({"type":"text-delta","index":0,"text":"Partial title"}),
                json!({"type":"finish","reason":{"kind":"max-tokens"}}),
            ]),
            "session-title-llm: title output reached maxOutputTokens",
        ),
        (
            chunks([
                json!({"type":"block-start","index":0,"blockType":"tool-call"}),
                json!({"type":"tool-call-delta","index":0,"id":"title-tool","name":"unexpected","argumentsDelta":"{}"}),
                json!({"type":"finish","reason":{"kind":"stop"}}),
            ]),
            "session-title-llm: title output must contain text only",
        ),
        (
            chunks([
                json!({"type":"block-start","index":0,"blockType":"reasoning"}),
                json!({"type":"reasoning-delta","index":0,"text":"no final title"}),
                json!({"type":"finish","reason":{"kind":"stop"}}),
            ]),
            "session-title-llm: title model produced no text",
        ),
    ];
    for (index, (script, expected)) in cases.into_iter().enumerate() {
        let env = setup(script, &[]).await;
        let session = session(&env, &format!("reject-{index}"));
        append_human(&session, "first input");
        assert_eq!(
            env.titles
                .refresh(&session, None)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
        env.shutdown().await;
    }
}

#[test]
fn locale_files_match_the_source_catalog() {
    let locales = [
        ("de", include_str!("../locale/de.json")),
        ("en", include_str!("../locale/en.json")),
        ("es", include_str!("../locale/es.json")),
        ("fr", include_str!("../locale/fr.json")),
        ("ja", include_str!("../locale/ja.json")),
        ("pt", include_str!("../locale/pt.json")),
        ("ru", include_str!("../locale/ru.json")),
        ("zh", include_str!("../locale/zh.json")),
    ];
    for (language, contents) in locales {
        let value: Value = serde_json::from_str(contents).unwrap();
        assert!(value["meta"]["title"].is_string(), "{language}");
    }
}
