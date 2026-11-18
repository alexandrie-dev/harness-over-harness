//! One round of fixes after the independent review (plans/briefs/fix-stage1-s4-system-prompt.md).
//! Each test fails against the code before its fix.

mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use common::*;
use hoh_kernel::{Kernel, Realm};
use hoh_system_prompt::{
    AssembleContext, AssembledSection, Config, PromptAssembly, PromptPresentation, SystemPrompt,
    SystemPromptAssemble, SystemPromptChange, Variables, render_prompt,
};
use serde_json::json;

fn plain() -> AssembleContext {
    AssembleContext::default()
}

fn extra_section(name: &str, text: &str) -> AssembledSection {
    AssembledSection {
        name: name.into(),
        text: text.into(),
        interpolate: None,
    }
}

fn no_identity() -> serde_json::Value {
    json!({"includeHarnessIdentity": false})
}

// ---- review 1: the realm of an assembly is the caller's, not the service's ----

#[tokio::test(start_paused = true)]
async fn assembly_dispatches_to_the_callers_realm_and_the_service_context_keeps_its_own() {
    let h = mount(no_identity()).await;
    let caller = h.ctx.isolate("systemPrompt", Realm::new());
    let root_calls = Arc::new(AtomicUsize::new(0));
    let realm_calls = Arc::new(AtomicUsize::new(0));
    for (ctx, name, calls) in [
        (&h.ctx, "from-root", root_calls.clone()),
        (&caller, "from-realm", realm_calls.clone()),
    ] {
        ctx.on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |event, next| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                event.update(|a| a.sections.push(extra_section(name, name)));
                next.run().await
            }
        })
        .unwrap();
    }

    let for_caller = h.prompt.assemble_from(&caller, plain()).await.unwrap();
    assert_eq!(
        (
            root_calls.load(Ordering::SeqCst),
            realm_calls.load(Ordering::SeqCst)
        ),
        (0, 1)
    );
    assert_eq!(render_prompt(&for_caller).unwrap(), "from-realm");

    let for_service = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(
        (
            root_calls.load(Ordering::SeqCst),
            realm_calls.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
    assert_eq!(render_prompt(&for_service).unwrap(), "from-root");
    h.kernel.shutdown().await;
}

// ---- review 2: the tail returns the object the listeners mutate ----

#[tokio::test(start_paused = true)]
async fn a_later_listeners_addition_survives_an_earlier_listener_returning_the_original() {
    let h = mount(no_identity()).await;
    // A: awaits `next()`, then returns its original assembly (the shared object).
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|event, next| async move {
            next.run().await?;
            Ok(event.assembly())
        })
        .unwrap();
    // B: awaits `next()`, appends to the returned presentation, returns it.
    let same_object = Arc::new(Mutex::new(None));
    let seen = same_object.clone();
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |event, next| {
            let seen = seen.clone();
            async move {
                let returned = next.run().await?;
                *seen.lock().unwrap() = Some(returned.same_object(&event.assembly()));
                returned.update(|a| a.sections.push(extra_section("extra", "extra text")));
                Ok(returned)
            }
        })
        .unwrap();

    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(*same_object.lock().unwrap(), Some(true));
    assert_eq!(render_prompt(&assembly).unwrap(), "extra text");
    h.kernel.shutdown().await;
}

// ---- review 3: explicit null in the configuration ----

async fn load_error(config: serde_json::Value) -> String {
    let kernel = Kernel::start();
    let error = match kernel.root().plugin::<SystemPrompt>(config) {
        Err(error) => error.to_string(),
        Ok(fiber) => fiber.settled().await.unwrap_err().to_string(),
    };
    kernel.shutdown().await;
    error
}

#[tokio::test(start_paused = true)]
async fn an_explicit_null_tool_order_fails_to_load_as_the_source_cannot_iterate_it() {
    // The source keeps the null and `for (const name of toolOrder)` throws V8's
    // `TypeError: toolOrder is not iterable`; the port reports the same text.
    let error = load_error(json!({"toolOrder": null})).await;
    assert!(error.contains("toolOrder is not iterable"), "{error}");
}

#[tokio::test(start_paused = true)]
async fn an_absent_tool_order_still_loads() {
    let h = mount(json!({})).await;
    h.prompt.tools(&h.ctx, schemas(&["b", "a"])).unwrap();
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(names(&assembly), ["a", "b"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_direct_construction_with_a_null_tool_order_fails_too() {
    let kernel = Kernel::start();
    let config = Config {
        tool_order: Some(None),
        ..Config::default()
    };
    let error = SystemPrompt::new(kernel.root(), &config).err().unwrap();
    assert!(error.to_string().contains("not iterable"), "{error}");
    kernel.shutdown().await;
}

const NULL_RULE: &str = "kernel round 7: schemastery null rule";

#[tokio::test(start_paused = true)]
#[ignore = "kernel round 7: schemastery null rule"]
async fn a_null_include_harness_identity_defaults_to_true() {
    let h = mount(json!({"includeHarnessIdentity": null})).await;
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(render_prompt(&assembly).unwrap(), IDENTITY, "{NULL_RULE}");
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
#[ignore = "kernel round 7: schemastery null rule"]
async fn a_null_include_runtime_context_defaults_to_true() {
    let h = mount(json!({"includeRuntimeContext": null})).await;
    h.prompt
        .context(&h.ctx, context("optional", 1.0, "optional fact"))
        .unwrap();
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(
        assembly
            .contexts
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["optional"]
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
#[ignore = "kernel round 7: schemastery null rule"]
async fn a_null_persona_prefix_defaults_to_empty() {
    let h = mount(json!({"personaPrefix": null})).await;
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(render_prompt(&assembly).unwrap(), IDENTITY);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
#[ignore = "kernel round 7: schemastery null rule"]
async fn a_null_persona_suffix_defaults_to_empty() {
    let h = mount(json!({"personaSuffix": null})).await;
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(render_prompt(&assembly).unwrap(), IDENTITY);
    h.kernel.shutdown().await;
}

// ---- review 4: change notifications carry no receiver filter ----

#[tokio::test(start_paused = true)]
async fn change_notifications_reach_every_listener_whatever_filter_the_context_carries() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    root.on::<SystemPromptChange>(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();

    // The constructor registers identity, persona prefix and persona suffix: three notifications.
    let blocked = root.with_filter(|_| false);
    let prompt = SystemPrompt::new(blocked, &Config::default()).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 3);

    let registration = prompt.section(&root, section("late", 5.0, "late")).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 4);
    registration.dispose().await;
    assert_eq!(count.load(Ordering::SeqCst), 5);
    kernel.shutdown().await;
}

// ---- review 7: JavaScript's enumeration order for integer-like variable names ----

#[tokio::test(start_paused = true)]
async fn a_listener_enumerates_variables_in_javascript_order() {
    let h = mount(no_identity()).await;
    h.prompt
        .section(&h.ctx, section("keys", 10.0, "{{a}}"))
        .unwrap();
    h.prompt
        .variable(&h.ctx, "a", |_| Ok(Some("a-value".into())))
        .unwrap();
    // A listener adds "10", "2", "a" (insertion order 10, 2, a) and derives text from the keys.
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|event, next| async move {
            let keys = event.update(|a| {
                a.variables = Variables::new();
                a.variables.insert("10", Some("ten".into()));
                a.variables.insert("2", Some("two".into()));
                a.variables.insert("a", Some("a-value".into()));
                a.variables.keys().collect::<Vec<_>>().join(",")
            });
            event.update(|a| a.sections.push(extra_section("derived", &keys)));
            next.run().await
        })
        .unwrap();
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    let derived = assembly
        .sections
        .iter()
        .find(|s| s.name == "derived")
        .unwrap();
    assert_eq!(derived.text, "2,10,a");
    assert_eq!(
        assembly
            .variables
            .iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>(),
        ["2", "10", "a"]
    );
    h.kernel.shutdown().await;
}

#[test]
fn only_canonical_array_index_names_enumerate_first() {
    let mut variables = Variables::new();
    for key in ["b", "01", "4294967295", "4294967294", "1", "a"] {
        variables.insert(key, None);
    }
    assert_eq!(
        variables.keys().collect::<Vec<_>>(),
        ["1", "4294967294", "b", "01", "4294967295", "a"]
    );
    // Re-setting a name keeps its place; removing and re-adding moves it to the end.
    variables.insert("b", Some("x".into()));
    variables.remove("01");
    variables.insert("01", None);
    assert_eq!(
        variables.keys().collect::<Vec<_>>(),
        ["1", "4294967294", "b", "4294967295", "a", "01"]
    );
}

#[test]
fn the_unknown_variable_diagnostic_lists_integer_like_names_first() {
    let mut variables = Variables::new();
    variables.insert("10", Some("ten".into()));
    variables.insert("2", Some("two".into()));
    variables.insert("a", Some("a".into()));
    let assembly = PromptAssembly {
        sections: vec![extra_section("s", "{{missing}}")],
        variables,
        ..PromptAssembly::default()
    };
    let error = render_prompt(&assembly).unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("registered variables: 2, 10, a"),
        "{error}"
    );
}
