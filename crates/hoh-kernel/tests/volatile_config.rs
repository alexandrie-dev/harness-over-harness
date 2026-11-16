mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::Arc;
use support::*;
#[derive(Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Settings {
    #[serde(default = "title")]
    title: Volatile<String>,
    #[serde(default)]
    optional: Volatile<Option<String>>,
    #[serde(default = "fixed")]
    fixed: String,
    #[serde(default)]
    nested: Nested,
    #[serde(default)]
    group: Volatile<Group>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, Default)]
pub struct Group {
    list: Vec<String>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[schemars(inline)]
pub struct Nested {
    #[serde(default = "one")]
    #[schemars(range(min = 0))]
    count: Volatile<i64>,
}
fn one() -> Volatile<i64> {
    Volatile(1)
}
impl Default for Nested {
    fn default() -> Self {
        Self { count: one() }
    }
}
fn title() -> Volatile<String> {
    Volatile("hello".into())
}
fn fixed() -> String {
    "fixed".into()
}
#[plugin(Live, name = "live")]
async fn live(ctx: Context, config: Cfg<Settings>) -> Result<(), Error> {
    let t = ctx.tag::<Arc<Track>>().unwrap();
    t.push(config.get().title.0.clone());
    let cfg = config.clone();
    let updates = t.clone();
    let live = config.clone();
    ctx.on::<VolatileUpdate>(move |e| {
        lock(&updates.updates).push(json!({"paths":e.paths,"title":live.get().title.0}))
    })?;
    ctx.on::<Ping>(move |_| {
        t.push(cfg.get().title.0.clone());
    })?;
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn parses_references_with_defaults_and_absent_values_while_preserving_schema_metadata() {
    let (k, r, t) = harness();
    let def = PluginDef::of::<Live>();
    assert_eq!(def.schema()["properties"]["title"]["x-volatile"], true);
    assert_eq!(def.schema()["properties"]["title"]["default"], "hello");
    let f = r.plugin_dyn(&def, json!({}), Default::default()).unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["hello"]);
    assert_eq!(f.effective_config().unwrap()["title"], "hello");
    assert_eq!(f.effective_config().unwrap()["nested"]["count"], 1);
    assert_eq!(f.effective_config().unwrap()["group"], json!({"list":[]}));
    assert!(f.resolve_config(&json!({"nested":{"count":-1}})).is_err());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_repeat_wrapping_and_supports_an_entire_array_or_union_as_a_value() {
    let nested = serde_json::to_value(schemars::schema_for!(Volatile<Volatile<String>>)).unwrap();
    assert!(config::validate(&nested, &json!("value")).is_err());
    let list: Volatile<Vec<String>> = serde_json::from_value(json!(["a"])).unwrap();
    assert_eq!(list.0, ["a"]);
    let union: Volatile<serde_json::Value> = serde_json::from_value(json!(1)).unwrap();
    assert_eq!(union.0, 1);
}
#[tokio::test(start_paused = true)]
async fn rejects_placements_even_when_input_omits_the_field() {
    for field in [
        json!({"type":"array","items":{"type":"string","x-volatile":true}}),
        json!({"type":"object","additionalProperties":{"type":"string","x-volatile":true}}),
        json!({"anyOf":[{"properties":{"title":{"x-volatile":true}}},{"type":"string"}]}),
        json!({"prefixItems":[{"x-volatile":true}]}),
        json!({"allOf":[{"properties":{"title":{"x-volatile":true}}}]}),
        json!({"x-volatile":true,"properties":{"title":{"x-volatile":true}}}),
    ] {
        let schema = json!({"type":"object","properties":{"items":field}});
        assert!(config::validate(&schema, &json!({})).is_err());
    }
}
#[tokio::test(start_paused = true)]
async fn ignores_reference_contents_while_retaining_ordinary_differences() {
    let s = PluginDef::of::<Live>();
    assert!(config::equal_except_volatile(
        s.schema(),
        &json!({"title":"a"}),
        &json!({"title":"b"})
    ));
    assert!(!config::equal_except_volatile(
        s.schema(),
        &json!({"title":"a"}),
        &json!({"title":"a","fixed":"changed"})
    ));
}
#[tokio::test(start_paused = true)]
async fn commits_live_config_and_emits_one_local_event() {
    let (k, r, t) = harness();
    let f = r.plugin::<Live>(json!({})).unwrap();
    f.settled().await.unwrap();
    let events = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let e = events.clone();
    r.on::<VolatileUpdate>(move |v| lock(&e).push(v.paths.clone()))
        .unwrap();
    assert!(f.commit_volatile(json!({"title":"new"})).await.unwrap());
    r.emit(&Ping);
    assert_eq!(t.list(), ["hello", "new"]);
    assert!(lock(&events).is_empty());
    assert_eq!(
        *lock(&t.updates),
        [json!({"paths":["title"],"title":"new"})]
    );
    assert!(
        !f.commit_volatile(json!({"fixed":"changed","title":"other"}))
            .await
            .unwrap()
    );
    assert!(f.commit_volatile(json!({"title":1})).await.is_err());
    assert_eq!(f.raw_config()["title"], 1);
    assert_eq!(f.effective_config().unwrap()["title"], "new");
    k.shutdown().await;
}
