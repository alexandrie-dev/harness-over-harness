use hoh_kernel::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    num::NonZeroU32,
};
struct Check<C>(std::marker::PhantomData<C>);
impl<C: Config> Plugin for Check<C> {
    const NAME: &'static str = "check";
    type Config = C;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<C>) -> Result<(), Error> {
        Ok(())
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Field<T> {
    value: T,
}
async fn loads_and_rejects<T: Config>(valid: Value, invalid: Value) {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Field<T>>>(json!({"value":valid}))
        .unwrap();
    f.settled().await.unwrap();
    let error = f.update(json!({"value":invalid})).await.unwrap_err();
    assert!(error.to_string().contains("at value"), "{error}");
    k.shutdown().await;
}
#[derive(Serialize, Deserialize, JsonSchema)]
enum External {
    Text(String),
    Record { n: u32 },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
enum Internal {
    Record { n: u32 },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data")]
enum Adjacent {
    Record { n: u32 },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
enum Untagged {
    Text(String),
    Record { n: u32 },
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Inner {
    n: u32,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Flatten {
    #[serde(flatten)]
    inner: Inner,
    flag: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Strict {
    n: u32,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Recursive {
    n: u32,
    #[serde(default)]
    children: Vec<Self>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Newtype(u32);
macro_rules! case {
    ($name:ident,$t:ty,$valid:expr,$invalid:expr) => {
        #[tokio::test(start_paused = true)]
        async fn $name() {
            loads_and_rejects::<$t>($valid, $invalid).await;
        }
    };
}
case!(
    b1_sets,
    BTreeSet<String>,
    json!(["a", "b"]),
    json!(["a", "a"])
);
case!(b1_integer_map,HashMap<u32,String>,json!({"1":"one"}),json!({"bad":"one"}));
case!(b1_nonzero, NonZeroU32, json!(1), json!(0));
case!(b1_signed_nonzero, std::num::NonZeroI32, json!(-1), json!(0));
case!(b1_option, Option<u32>, json!(null), json!("bad"));
case!(
    b1_external,
    External,
    json!({"Record":{"n":1}}),
    json!({"Record":{"n":"bad"}})
);
case!(
    b1_internal,
    Internal,
    json!({"kind":"Record","n":1}),
    json!({"kind":"Record","n":"bad"})
);
case!(
    b1_adjacent,
    Adjacent,
    json!({"kind":"Record","data":{"n":1}}),
    json!({"kind":"Record","data":{"n":"bad"}})
);
case!(b1_untagged, Untagged, json!({"n":1}), json!({"n":false}));
case!(
    b1_flatten,
    Flatten,
    json!({"n":1,"flag":true}),
    json!({"n":"bad","flag":true})
);
case!(
    b1_strict,
    Strict,
    json!({"n":1}),
    json!({"n":1,"extra":true})
);
case!(
    b1_recursive,
    Recursive,
    json!({"n":1,"children":[{"n":2}]}),
    json!({"n":1,"children":[{"n":"bad"}]})
);
case!(
    b1_tuple,
    (String, u32),
    json!(["a", 1]),
    json!(["a", "bad"])
);
case!(b1_newtype, Newtype, json!(1), json!("bad"));
#[tokio::test(start_paused = true)]
async fn b1_recursive_root() {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Recursive>>(json!({"n":1,"children":[{"n":2}]}))
        .unwrap();
    f.settled().await.unwrap();
    let e = f
        .update(json!({"n":1,"children":[{"n":"bad"}]}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("children.0.n"));
    k.shutdown().await;
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Access {
    #[serde(skip_serializing, rename = "secret")]
    _secret: String,
    #[serde(skip_deserializing)]
    readonly: u32,
}
case!(
    b1_access_annotations,
    Access,
    json!({"secret":"s","readonly":0}),
    json!({"secret":false})
);
#[derive(Serialize, Deserialize, JsonSchema)]
struct LiveInner {
    v: Volatile<u32>,
    n: u32,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct LiveConfig {
    inner: LiveInner,
}
#[tokio::test(start_paused = true)]
async fn b2_nonrecursive_refs() {
    let s = PluginDef::of::<Check<LiveConfig>>();
    let a = json!({"inner":{"v":1,"n":2}});
    let b = json!({"inner":{"v":2,"n":2}});
    assert!(config::equal_except_volatile(s.schema(), &a, &b));
    assert_eq!(config::volatile_paths(s.schema(), &a, &b), ["inner.v"]);
    let k = Kernel::start();
    let f = k.root().plugin::<Check<LiveConfig>>(a).unwrap();
    f.settled().await.unwrap();
    assert!(f.commit_volatile(b).await.unwrap());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn b3_js_number_text() {
    let e = config::validate(&json!({"type":"string"}), &json!(9007199254740993_u64)).unwrap_err();
    assert_eq!(
        e.issues[0].message,
        "expected string but got 9007199254740992"
    );
}
#[tokio::test(start_paused = true)]
async fn b3_integral_float_and_range_path() {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Field<u32>>>(json!({"value":2.0}))
        .unwrap();
    f.settled().await.unwrap();
    assert_eq!(f.effective_config().unwrap()["value"], 2);
    let e = f.update(json!({"value":1e30})).await.unwrap_err();
    assert!(e.to_string().contains("at value"));
    assert!(e.to_string().contains("expected"));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn b3_exact_integer_bounds() {
    for (s, v) in [
        (
            json!({"minimum":9007199254740993_u64}),
            json!(9007199254740992_u64),
        ),
        (
            json!({"maximum":9007199254740992_u64}),
            json!(9007199254740993_u64),
        ),
        (json!({"multipleOf":2}), json!(9007199254740993_u64)),
    ] {
        assert!(config::validate(&s, &v).is_err(), "{s:?} {v:?}");
    }
}
#[tokio::test(start_paused = true)]
async fn b3_union_type_description() {
    let e = config::validate(&json!({"type":["string","null"]}), &json!(1)).unwrap_err();
    assert_eq!(e.issues[0].message, "expected string | null but got 1");
}
#[tokio::test(start_paused = true)]
async fn b3_invalid_pattern_is_schema_error() {
    let e = config::validate(&json!({"type":"string","pattern":"(?=a)"}), &json!("a")).unwrap_err();
    assert!(e.to_string().contains("invalid schema pattern"));
    assert!(e.to_string().contains("(?=a)"));
}
#[tokio::test(start_paused = true)]
async fn b3_json_text_uses_js_order() {
    let e = config::validate(
        &json!({"const":true}),
        &json!({"10":1,"2":9007199254740993_u64,"a":1e20}),
    )
    .unwrap_err();
    assert!(
        e.to_string()
            .contains(r#"{"2":9007199254740992,"10":1,"a":100000000000000000000}"#),
        "{e}"
    );
}
#[tokio::test(start_paused = true)]
async fn b3_depth_counts_values_not_refs() {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let s = serde_json::to_value(schemars::schema_for!(Recursive)).unwrap();
            let mut v = json!({"n":1});
            for _ in 0..200 {
                v = json!({"n":1,"children":[v]});
            }
            assert!(config::validate(&s, &v).is_ok());
        })
        .unwrap()
        .join()
        .unwrap();
}
#[tokio::test(start_paused = true)]
async fn b3_number_value_equality() {
    assert!(config::validate(&json!({"const":1}), &json!(1.0)).is_ok());
    assert!(config::validate(&json!({"enum":[1]}), &json!(1.0)).is_ok());
    assert!(config::equal_except_volatile(
        &json!(true),
        &json!({"n":1}),
        &json!({"n":1.0})
    ));
}
#[tokio::test(start_paused = true)]
async fn b4_unused_defs_are_inspected() {
    assert!(
        config::validate(
            &json!({"$defs":{"Unused":{"unsupported":true}}}),
            &json!({})
        )
        .is_err()
    );
}
#[tokio::test(start_paused = true)]
async fn b4_nested_unused_defs_are_inspected() {
    assert!(
        config::validate(
            &json!({"not":{"$defs":{"Unused":{"unsupported":true}},"const":false}}),
            &json!(true)
        )
        .is_err()
    );
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
struct InlineRequired {
    n: u32,
}
#[tokio::test(start_paused = true)]
async fn b5_nullable_inline_object_stays_null() {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Option<InlineRequired>>>(Value::Null)
        .unwrap();
    f.settled().await.unwrap();
    assert!(f.effective_config().unwrap().is_null());
    k.shutdown().await;
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Nested {
    known: u32,
    inner: Inner,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct NestedConfig {
    nested: Nested,
    list: Vec<Nested>,
}
#[tokio::test(start_paused = true)]
async fn b6_preserves_unknowns_recursively() {
    let k = Kernel::start();
    let raw = json!({"nested":{"known":1,"extra":2,"inner":{"n":1,"extra":3}},"list":[{"known":2,"extra":4,"inner":{"n":2,"extra":5}}]});
    let f = k.root().plugin::<Check<NestedConfig>>(raw.clone()).unwrap();
    f.settled().await.unwrap();
    assert_eq!(*f.effective_config().unwrap(), raw);
    assert_eq!(*f.raw_config(), raw);
    k.shutdown().await;
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct RecursiveLive {
    v: Volatile<u32>,
    #[serde(default)]
    children: Vec<Self>,
}
#[tokio::test(start_paused = true)]
async fn b2_recursive_backedge_is_compared_raw() {
    let s = PluginDef::of::<Check<RecursiveLive>>();
    let a = json!({"v":1,"children":[{"v":1}]});
    let b = json!({"v":1,"children":[{"v":2}]});
    assert!(!config::equal_except_volatile(s.schema(), &a, &b));
    assert!(config::volatile_paths(s.schema(), &a, &b).is_empty());
}
#[tokio::test(start_paused = true)]
async fn b6_preserves_key_order() {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Field<u32>>>(json!({"value":1,"extra":2}))
        .unwrap();
    f.settled().await.unwrap();
    assert_eq!(
        f.effective_config()
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["value", "extra"]
    );
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn optional_integral_float_loads() {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Field<Option<u32>>>>(json!({"value":2.0}))
        .unwrap();
    f.settled().await.unwrap();
    assert_eq!(f.effective_config().unwrap()["value"], 2);
    f.update(json!({"value":null})).await.unwrap();
    assert!(f.effective_config().unwrap()["value"].is_null());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn optional_integer_range_has_path() {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Field<Option<u8>>>>(json!({"value":null}))
        .unwrap();
    f.settled().await.unwrap();
    let error = f.update(json!({"value":300})).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid config:\n  - expected number <= 255 but got 300 (at value)"
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn optional_integer_format_range_has_path() {
    let k = Kernel::start();
    let f = k
        .root()
        .plugin::<Check<Field<Option<u32>>>>(json!({"value":null}))
        .unwrap();
    f.settled().await.unwrap();
    let error = f.update(json!({"value":4294967296_u64})).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid config:\n  - expected number <= 4294967295 but got 4294967296 (at value)"
    );
    k.shutdown().await;
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct WriteOnly {
    #[serde(skip_serializing)]
    _secret: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct ReadOnly {
    #[serde(skip_deserializing)]
    readonly: u32,
}
#[tokio::test(start_paused = true)]
async fn derived_access_annotations_are_emitted_and_load() {
    let write = PluginDef::of::<Check<WriteOnly>>();
    assert_eq!(write.schema()["properties"]["_secret"]["writeOnly"], true);
    let read = schemars::generate::SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<ReadOnly>();
    let read = serde_json::to_value(read).unwrap();
    assert_eq!(read["properties"]["readonly"]["readOnly"], true);
    config::validate(&read, &json!({"readonly":3})).unwrap();
    config::validate(write.schema(), &json!({"_secret":"s"})).unwrap();
    let k = Kernel::start();
    k.root()
        .plugin::<Check<WriteOnly>>(json!({"_secret":"s"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    k.root()
        .plugin::<Check<ReadOnly>>(json!({"readonly":3}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn unknown_key_uses_never_wording_and_path() {
    let def = PluginDef::of::<Check<Strict>>();
    let e = config::validate(def.schema(), &json!({"n":1,"extra":7})).unwrap_err();
    assert_eq!(
        e.issues,
        vec![Issue {
            path: vec![PathSeg::Key("extra".into())],
            message: "expected nullable but got 7".into()
        }]
    );
}
#[tokio::test(start_paused = true)]
async fn referenced_definition_is_inspected_once() {
    let e = config::validate(
        &json!({"$ref":"#/$defs/Config","$defs":{"Config":{"unsupported":true}}}),
        &json!({}),
    )
    .unwrap_err();
    assert_eq!(e.issues.len(), 1);
    assert_eq!(
        e.issues[0].message,
        "unsupported schema keyword: unsupported"
    );
}
#[tokio::test(start_paused = true)]
async fn recursive_root_is_inspected_once() {
    let e = config::validate(&json!({"$ref":"#","unsupported":true}), &json!({})).unwrap_err();
    assert_eq!(e.issues.len(), 1);
    assert_eq!(
        e.issues[0].message,
        "unsupported schema keyword: unsupported"
    );
}
#[tokio::test(start_paused = true)]
async fn non_emitted_schema_words_are_unsupported() {
    for word in [
        "contains",
        "minContains",
        "maxContains",
        "if",
        "then",
        "else",
        "dependentRequired",
        "dependentSchemas",
        "propertyNames",
        "$id",
        "minProperties",
        "maxProperties",
    ] {
        let schema = json!({word:0});
        let e = config::validate(&schema, &Value::Null).unwrap_err();
        assert_eq!(
            e.issues[0].message,
            format!("unsupported schema keyword: {word}")
        );
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FlattenedEnum {
    #[serde(flatten)]
    item: Internal,
    flag: bool,
}
#[tokio::test(start_paused = true)]
async fn derived_flatten_emits_unevaluated_properties() {
    let def = PluginDef::of::<Check<FlattenedEnum>>();
    assert_eq!(def.schema()["unevaluatedProperties"], false);
    config::validate(def.schema(), &json!({"kind":"Record","n":1,"flag":true})).unwrap();
    let e = config::validate(
        def.schema(),
        &json!({"kind":"Record","n":1,"flag":true,"extra":2}),
    )
    .unwrap_err();
    assert_eq!(e.issues[0].path, [PathSeg::Key("extra".into())]);
}
#[tokio::test(start_paused = true)]
async fn original_r48_numeric_constraints_remain_supported() {
    for (schema, valid, invalid, message) in [
        (
            json!({"exclusiveMinimum":1}),
            json!(2),
            json!(1),
            "expected number > 1 but got 1",
        ),
        (
            json!({"exclusiveMaximum":1}),
            json!(0),
            json!(1),
            "expected number < 1 but got 1",
        ),
        (
            json!({"multipleOf":2}),
            json!(4),
            json!(3),
            "expected number multiple of 2 but got 3",
        ),
    ] {
        config::validate(&schema, &valid).unwrap();
        assert_eq!(
            config::validate(&schema, &invalid).unwrap_err().issues[0].message,
            message
        );
    }
}
