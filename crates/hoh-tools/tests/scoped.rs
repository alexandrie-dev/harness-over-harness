//! Port of tests/scoped.spec.ts. Not ported (JavaScript-only): the `expectTypeOf` case, the readonly
//! filter-array aliasing case (the filter is moved into `restrict`), frozen-object checks, accessor
//! snapshots on the input object, `Map`/class-instance arguments and non-cloneable argument values.
//! Lossy numbers stand in for non-JSON arguments.

mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use common::*;
use hoh_scope::{CreateScopeOptions, Scope, create_scope};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, PostToolDecision, PreToolDecision, ToolDefinition,
    ToolErrorInfo, ToolExecution, ToolExecutionResult, ToolGuardDenial, ToolRestriction,
    ToolsExecute, ToolsPostExecute, ToolsPreExecute, ToolsResult, define_tool,
};
use serde_json::{Value, json};

struct Minted {
    scope: Scope,
    agent: Arc<TestAgent>,
}

fn mint(env: &Env, parent: Option<&Minted>) -> Minted {
    let agent = TestAgent::new();
    let scope = create_scope(
        &env.ctx,
        agent.key.clone(),
        CreateScopeOptions {
            parent: parent.map(|parent| parent.agent.key.clone()),
        },
    )
    .expect("scope");
    Minted { scope, agent }
}

fn tool_replying(name: &str, reply: &str) -> ToolDefinition {
    let reply = reply.to_owned();
    define_tool(DefineToolOptions::new(
        name,
        format!("tool {name}"),
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![text(value.as_str().unwrap())]),
            presentation_meta: None,
        },
        move |_, _| {
            let reply = reply.clone();
            Box::pin(async move { Ok(json!(reply)) })
        },
    ))
    .unwrap()
}

fn tool(name: &str) -> ToolDefinition {
    tool_replying(name, &format!("ran:{name}"))
}

fn counting(name: &str, counter: &Arc<AtomicUsize>) -> ToolDefinition {
    let mut definition = tool(name);
    let counter = counter.clone();
    definition.execute = Arc::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(json!(format!("ran:{}", "t"))) })
    });
    definition
}

fn add(env: &Env, definition: ToolDefinition) {
    register(env, definition);
}

fn add_in(env: &Env, minted: &Minted, definition: ToolDefinition) -> hoh_kernel::Disposer {
    env.tools
        .register(&minted.scope.ctx, definition, Default::default())
        .expect("scoped registration")
}

fn restrict(
    env: &Env,
    minted: &Minted,
    allow: Option<&[&str]>,
    deny: Option<&[&str]>,
) -> Result<hoh_kernel::Disposer, String> {
    let owned = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    env.tools
        .restrict(
            &minted.scope.ctx,
            ToolRestriction {
                allow: allow.map(owned),
                deny: deny.map(owned),
            },
        )
        .map_err(|error| error.to_string())
}

fn names(env: &Env, agent: Option<&Arc<TestAgent>>) -> Vec<String> {
    let key = agent.map(|agent| agent.key.clone());
    let mut names: Vec<_> = env
        .tools
        .schemas(key.as_ref(), None)
        .unwrap()
        .into_iter()
        .map(|schema| schema.name)
        .collect();
    names.sort();
    names
}

async fn run(env: &Env, name: &str, agent: Option<&Arc<TestAgent>>) -> String {
    let mut request = input("c1", name, json!({}));
    request.agent = agent.map(|agent| agent.clone() as Arc<dyn hoh_tools::Agent>);
    let result = execute_tool_fixture(&env.ctx, request).await;
    result
        .content()
        .first()
        .and_then(|block| block.text_value())
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(result.content()).unwrap())
}

fn matches_error(result: Result<hoh_kernel::Disposer, String>, needle: &str) {
    let message = result.err().expect("rejected");
    assert!(message.contains(needle), "{message}");
}

// ------------------------------------------------------- scoped tool registration

#[tokio::test]
async fn files_a_scoped_tool_in_its_layer_visible_and_executable_for_that_scope_only() {
    let env = setup().await;
    let a = mint(&env, None);
    let other = TestAgent::new();
    add(&env, tool("shared"));
    add_in(&env, &a, tool("mine"));

    assert_eq!(names(&env, Some(&a.agent)), vec!["mine", "shared"]);
    assert_eq!(names(&env, None), vec!["shared"]);
    assert_eq!(names(&env, Some(&other)), vec!["shared"]);

    assert_eq!(run(&env, "mine", Some(&a.agent)).await, "ran:mine");
    // Out-of-view execution is indistinguishable from a nonexistent tool.
    assert_eq!(
        run(&env, "mine", Some(&other)).await,
        "Error: unknown tool \"mine\""
    );
    assert_eq!(
        run(&env, "mine", None).await,
        "Error: unknown tool \"mine\""
    );
}

#[tokio::test]
async fn scoped_shadows_global_on_a_name_conflict_in_either_registration_order() {
    let env = setup().await;
    let a = mint(&env, None);
    add_in(&env, &a, tool_replying("bash", "restricted-bash"));
    add(&env, tool_replying("bash", "global-bash"));
    assert_eq!(run(&env, "bash", Some(&a.agent)).await, "restricted-bash");
    assert_eq!(run(&env, "bash", None).await, "global-bash");
    let shadows = env
        .tools
        .schemas(Some(&a.agent.key), None)
        .unwrap()
        .iter()
        .filter(|schema| schema.name == "bash")
        .count();
    assert_eq!(shadows, 1);
}

#[tokio::test]
async fn rejects_a_duplicate_name_within_one_layer_naming_agent_ctx_for_the_global_case() {
    let env = setup().await;
    let a = mint(&env, None);
    add(&env, tool("x"));
    let global = env.tools.register(&env.ctx, tool("x"), Default::default());
    assert!(
        global
            .err()
            .expect("duplicate")
            .to_string()
            .contains("agent.ctx")
    );
    add_in(&env, &a, tool("y"));
    let scoped = env
        .tools
        .register(&a.scope.ctx, tool("y"), Default::default());
    assert!(
        scoped
            .err()
            .expect("duplicate")
            .to_string()
            .contains("already registered in this scope")
    );
}

#[tokio::test]
async fn disposing_the_scope_unwinds_its_registrations_and_leaves_no_residue() {
    let env = setup().await;
    let a = mint(&env, None);
    add_in(&env, &a, tool("mine"));
    assert!(env.tools.get("mine", Some(&a.agent.key), None).is_some());
    a.scope.dispose().await;
    assert!(env.tools.get("mine", Some(&a.agent.key), None).is_none());
    assert!(names(&env, Some(&a.agent)).is_empty());
}

// -------------------------------------------------------------------- restrict()

#[tokio::test]
async fn restrict_masks_global_tools_merges_scope_local_tools_afterward_and_keeps_assembly_with_execution()
 {
    let env = setup().await;
    let a = mint(&env, None);
    add(&env, tool("read"));
    add(&env, tool("bash"));
    add_in(&env, &a, tool("capture"));
    restrict(&env, &a, Some(&["read"]), None).unwrap();

    assert_eq!(names(&env, Some(&a.agent)), vec!["capture", "read"]);
    assert_eq!(
        run(&env, "bash", Some(&a.agent)).await,
        "Error: unknown tool \"bash\""
    );
    assert_eq!(run(&env, "read", Some(&a.agent)).await, "ran:read");
    assert_eq!(run(&env, "capture", Some(&a.agent)).await, "ran:capture");
    assert_eq!(names(&env, None), vec!["bash", "read"]);
}

#[tokio::test]
async fn applies_snapshotted_filters_to_the_live_global_registry_before_merging_later_scope_local_tools()
 {
    let env = setup().await;
    let denied = mint(&env, None);
    let allowed = mint(&env, None);
    add(&env, tool("read"));
    add(&env, tool("bash"));
    restrict(&env, &denied, None, Some(&["bash"])).unwrap();
    restrict(&env, &allowed, Some(&["read"]), None).unwrap();

    add(&env, tool("web"));
    add_in(&env, &denied, tool("denied-local"));
    add_in(&env, &allowed, tool("allowed-local"));

    assert_eq!(
        names(&env, Some(&denied.agent)),
        vec!["denied-local", "read", "web"]
    );
    assert_eq!(
        names(&env, Some(&allowed.agent)),
        vec!["allowed-local", "read"]
    );
    assert_eq!(run(&env, "web", Some(&denied.agent)).await, "ran:web");
    assert_eq!(
        run(&env, "web", Some(&allowed.agent)).await,
        "Error: unknown tool \"web\""
    );
    assert_eq!(
        run(&env, "denied-local", Some(&denied.agent)).await,
        "ran:denied-local"
    );
    assert_eq!(
        run(&env, "allowed-local", Some(&allowed.agent)).await,
        "ran:allowed-local"
    );
}

#[tokio::test]
async fn composes_multiple_restrictions_by_intersection_and_lifts_each_independently() {
    let env = setup().await;
    let a = mint(&env, None);
    for name in ["a", "b", "c"] {
        add(&env, tool(name));
    }
    let lift_allow = restrict(&env, &a, Some(&["a", "b"]), None).unwrap();
    restrict(&env, &a, None, Some(&["b"])).unwrap();
    assert_eq!(names(&env, Some(&a.agent)), vec!["a"]);
    lift_allow.dispose().await;
    // The deny remains after the allow-list is lifted.
    assert_eq!(names(&env, Some(&a.agent)), vec!["a", "c"]);
}

#[tokio::test]
async fn fails_loud_on_an_unscoped_call_an_empty_filter_and_names_it_does_not_inherit() {
    let env = setup().await;
    let a = mint(&env, None);
    add(&env, tool("real"));
    add_in(&env, &a, tool("local"));
    let unscoped = env.tools.restrict(
        &env.ctx,
        ToolRestriction {
            allow: None,
            deny: Some(vec!["real".into()]),
        },
    );
    assert!(
        unscoped
            .err()
            .expect("rejected")
            .to_string()
            .contains("requires a scoped context")
    );
    matches_error(restrict(&env, &a, None, None), "no-op");
    // A scope's own registration is exempt from its own filter, so naming it is a caller error.
    matches_error(
        restrict(&env, &a, Some(&["local"]), None),
        "unknown global tool \"local\"",
    );
    let typo = restrict(&env, &a, Some(&["reall"]), None)
        .err()
        .expect("rejected");
    assert!(typo.contains("unknown global tool \"reall\""));
    assert!(typo.contains("known global tools: real"));
    matches_error(
        restrict(&env, &a, None, Some(&["ghost", "wraith"])),
        "unknown global tools \"ghost\", \"wraith\"",
    );

    let empty = setup().await;
    let empty_scope = mint(&empty, None);
    matches_error(
        restrict(&empty, &empty_scope, None, Some(&["ghost"])),
        "known global tools: (none)",
    );
}

// ------------------------------------------------ restrict() over an inherited layer

#[tokio::test]
async fn filters_tools_the_child_inherits_from_an_ancestor_scope_not_only_global_ones() {
    let env = setup().await;
    let parent = mint(&env, None);
    add_in(&env, &parent, tool("bash"));
    add_in(&env, &parent, tool("read"));
    let child = mint(&env, Some(&parent));

    assert_eq!(names(&env, Some(&child.agent)), vec!["bash", "read"]);
    restrict(&env, &child, None, Some(&["bash"])).unwrap();

    assert_eq!(names(&env, Some(&child.agent)), vec!["read"]);
    assert_eq!(
        run(&env, "bash", Some(&child.agent)).await,
        "Error: unknown tool \"bash\""
    );
    // The ancestor keeps its whole surface: a child's filter is its own.
    assert_eq!(names(&env, Some(&parent.agent)), vec!["bash", "read"]);
}

#[tokio::test]
async fn keeps_the_childs_own_registrations_outside_its_own_filter() {
    let env = setup().await;
    let parent = mint(&env, None);
    add_in(&env, &parent, tool("bash"));
    add_in(&env, &parent, tool("read"));
    let child = mint(&env, Some(&parent));
    add_in(&env, &child, tool("report"));

    restrict(&env, &child, Some(&["read"]), None).unwrap();

    assert_eq!(names(&env, Some(&child.agent)), vec!["read", "report"]);
    assert_eq!(run(&env, "report", Some(&child.agent)).await, "ran:report");
}

#[tokio::test]
async fn lets_an_ancestors_restriction_reach_every_scope_nested_inside_it() {
    let env = setup().await;
    add(&env, tool("web"));
    let parent = mint(&env, None);
    add_in(&env, &parent, tool("bash"));
    let child = mint(&env, Some(&parent));
    restrict(&env, &parent, None, Some(&["web"])).unwrap();

    assert_eq!(names(&env, Some(&child.agent)), vec!["bash"]);
    assert_eq!(names(&env, Some(&parent.agent)), vec!["bash"]);
}

// ------------------------------------------------------- scoped execution dispatch

#[tokio::test]
async fn an_agent_ctx_pre_execute_listener_gates_only_its_own_agent_and_never_subject_less_calls() {
    let env = setup().await;
    let a = mint(&env, None);
    let other = TestAgent::new();
    add(&env, tool("t"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = seen.clone();
        a.scope
            .ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, _next| {
                lock(&seen).push(event.exec.agent.as_ref().map(|agent| agent.scope_key()));
                async {
                    Ok(PreToolDecision::Deny {
                        reason: "scoped veto".into(),
                        info: None,
                    })
                }
            })
            .unwrap();
    }

    assert_eq!(run(&env, "t", Some(&a.agent)).await, "Error: scoped veto");
    assert_eq!(run(&env, "t", Some(&other)).await, "ran:t");
    assert_eq!(run(&env, "t", None).await, "ran:t");
    assert_eq!(*lock(&seen), vec![Some(a.agent.key.clone())]);
}

#[tokio::test]
async fn applies_scoped_guards_after_pre_execute_and_unwinds_duplicate_registrations_independently()
{
    let env = setup().await;
    let a = mint(&env, None);
    let other = TestAgent::new();
    let body_calls = Arc::new(AtomicUsize::new(0));
    let mut definition = tool("t");
    {
        let body_calls = body_calls.clone();
        definition.execute = Arc::new(move |_, _| {
            body_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!("ran:t")) })
        });
    }
    add(&env, definition);
    let guard: hoh_tools::ToolGuard =
        Arc::new(|_: &ToolExecution| Ok(Some("terminal policy".into())));
    let lift_first = env.tools.guard(&a.scope.ctx, guard.clone()).unwrap();
    env.tools.guard(&a.scope.ctx, guard).unwrap();
    // A later pre-execute listener may force the extensible decision to allow, but it cannot bypass
    // the owner-level monotonic guard that runs after the waterfall.
    a.scope
        .ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Allow)
        })
        .unwrap();

    assert_eq!(
        run(&env, "t", Some(&a.agent)).await,
        "Error: terminal policy"
    );
    assert_eq!(run(&env, "t", Some(&other)).await, "ran:t");
    assert_eq!(body_calls.load(Ordering::SeqCst), 1);

    lift_first.dispose().await;
    assert_eq!(
        run(&env, "t", Some(&a.agent)).await,
        "Error: terminal policy"
    );
    a.scope.dispose().await;
    assert_eq!(run(&env, "t", Some(&a.agent)).await, "ran:t");
    assert_eq!(body_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn composes_global_guards_monotonically_when_one_abstains_and_a_later_one_denies() {
    let env = setup().await;
    let body_calls = Arc::new(AtomicUsize::new(0));
    let mut definition = tool("t");
    {
        let body_calls = body_calls.clone();
        definition.execute = Arc::new(move |_, _| {
            body_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!("ran:t")) })
        });
    }
    add(&env, definition);
    env.tools.guard(&env.ctx, Arc::new(|_| Ok(None))).unwrap();
    env.tools
        .guard(&env.ctx, Arc::new(|_| Ok(Some("global denial".into()))))
        .unwrap();

    assert_eq!(run(&env, "t", None).await, "Error: global denial");
    assert_eq!(body_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn records_the_structured_identity_of_a_guard_denial() {
    let env = setup().await;
    add(&env, tool("t"));
    env.tools
        .guard(
            &env.ctx,
            Arc::new(|_| {
                Ok(Some(ToolGuardDenial {
                    reason: "outside the shared workspace".into(),
                    info: Some(ToolErrorInfo::new(
                        "ForkDeniedError",
                        "FORK_PATH_OUTSIDE_WORKSPACE",
                    )),
                }))
            }),
        )
        .unwrap();

    let result = execute_tool_fixture(&env.ctx, input("c1", "t", json!({}))).await;

    assert!(result.is_error());
    assert_eq!(
        result.content(),
        &[text("Error: outside the shared workspace")]
    );
    let error = result.error().unwrap();
    assert_eq!(error.message, "outside the shared workspace");
    assert_eq!(
        error.info,
        Some(ToolErrorInfo::new(
            "ForkDeniedError",
            "FORK_PATH_OUTSIDE_WORKSPACE"
        ))
    );
}

#[tokio::test]
async fn live_iterates_a_guard_registered_by_an_earlier_guard() {
    let env = setup().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let added = Arc::new(AtomicUsize::new(0));
    add(&env, tool("t"));
    {
        let (calls, added) = (calls.clone(), added.clone());
        let tools = env.tools.clone();
        let ctx = env.ctx.clone();
        env.tools
            .guard(
                &env.ctx,
                Arc::new(move |_| {
                    lock(&calls).push("first");
                    if added.fetch_add(1, Ordering::SeqCst) == 0 {
                        let calls = calls.clone();
                        tools.guard(
                            &ctx,
                            Arc::new(move |_| {
                                lock(&calls).push("late");
                                Ok(Some("late denial".into()))
                            }),
                        )?;
                    }
                    Ok(None)
                }),
            )
            .unwrap();
    }

    assert_eq!(run(&env, "t", None).await, "Error: late denial");
    assert_eq!(*lock(&calls), vec!["first", "late"]);
}

#[tokio::test]
async fn defers_a_scoped_guard_that_replaces_the_last_guard_in_its_generation() {
    let env = setup().await;
    let a = mint(&env, None);
    let calls = Arc::new(Mutex::new(Vec::new()));
    add(&env, tool("t"));
    add_in(&env, &a, tool("scope_sibling"));
    let lift = Arc::new(Mutex::new(None::<hoh_kernel::Disposer>));
    {
        let (calls, lift_slot) = (calls.clone(), lift.clone());
        let tools = env.tools.clone();
        let scope_ctx = a.scope.ctx.clone();
        let disposer = env
            .tools
            .guard(
                &a.scope.ctx,
                Arc::new(move |_| {
                    lock(&calls).push("first");
                    if let Some(disposer) = lock(&lift_slot).take() {
                        // Begins the disposal synchronously; the guard list is not awaited here.
                        drop(disposer.dispose());
                    }
                    let calls = calls.clone();
                    tools.guard(
                        &scope_ctx,
                        Arc::new(move |_| {
                            lock(&calls).push("replacement");
                            Ok(Some("replacement denial".into()))
                        }),
                    )?;
                    Ok(None)
                }),
            )
            .unwrap();
        *lock(&lift) = Some(disposer);
    }

    assert_eq!(run(&env, "t", Some(&a.agent)).await, "ran:t");
    assert_eq!(*lock(&calls), vec!["first"]);
    assert_eq!(
        run(&env, "t", Some(&a.agent)).await,
        "Error: replacement denial"
    );
    assert_eq!(*lock(&calls), vec!["first", "replacement"]);
}

#[tokio::test]
async fn shares_one_token_across_the_pipeline_and_runs_scoped_result_observers() {
    let env = setup().await;
    let a = mint(&env, None);
    let safe_calls = Arc::new(AtomicUsize::new(0));
    let danger_calls = Arc::new(AtomicUsize::new(0));
    let scoped_results = Arc::new(AtomicUsize::new(0));
    let safe_arguments = Arc::new(Mutex::new(None));
    let tokens = Arc::new(Mutex::new(Vec::new()));
    let mut safe = tool("safe");
    {
        let (safe_calls, safe_arguments) = (safe_calls.clone(), safe_arguments.clone());
        safe.execute = Arc::new(move |args, _| {
            safe_calls.fetch_add(1, Ordering::SeqCst);
            *lock(&safe_arguments) = Some(args);
            Box::pin(async { Ok(json!("safe")) })
        });
    }
    add(&env, safe);
    add(&env, counting_named("danger", &danger_calls));
    env.tools
        .guard(
            &a.scope.ctx,
            Arc::new(|exec| Ok((exec.name == "danger").then(|| "danger denied".into()))),
        )
        .unwrap();
    {
        let tokens = tokens.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
                lock(&tokens).push(event.exec.token);
                async move { next.run().await }
            })
            .unwrap();
    }
    {
        let tokens = tokens.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                lock(&tokens).push(event.exec.token);
                async move { next.run().await }
            })
            .unwrap();
    }
    {
        let tokens = tokens.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
                lock(&tokens).push(event.exec.token);
                async move { next.run().await }
            })
            .unwrap();
    }
    {
        let scoped_results = scoped_results.clone();
        a.scope
            .ctx
            .on_contained::<ToolsResult>(move |_| {
                scoped_results.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
    }

    assert_eq!(
        run(&env, "danger", Some(&a.agent)).await,
        "Error: danger denied"
    );
    let caller_arguments = json!({ "source": true });
    let mut request = input("safe-call", "safe", caller_arguments.clone());
    request.agent = Some(a.agent.clone());
    let safe_result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(safe_result.content(), &[text("safe")]);
    assert_eq!(*lock(&safe_arguments), Some(caller_arguments));
    // One token for danger and one shared by every phase of safe.
    let mut distinct: Vec<_> = lock(&tokens).clone();
    distinct.sort_by_key(|token| format!("{token:?}"));
    distinct.dedup();
    assert_eq!(distinct.len(), 2);
    assert_eq!(
        (
            safe_calls.load(Ordering::SeqCst),
            danger_calls.load(Ordering::SeqCst),
            scoped_results.load(Ordering::SeqCst)
        ),
        (1, 0, 2)
    );
}

fn counting_named(name: &str, counter: &Arc<AtomicUsize>) -> ToolDefinition {
    let mut definition = tool(name);
    let counter = counter.clone();
    definition.execute = Arc::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(json!("danger")) })
    });
    definition
}

#[tokio::test]
async fn normalizes_non_json_arguments_and_still_publishes_one_scoped_final_outcome() {
    let env = setup().await;
    let a = mint(&env, None);
    let policy_calls = Arc::new(AtomicUsize::new(0));
    let body_calls = Arc::new(AtomicUsize::new(0));
    let scoped_observed = Arc::new(AtomicUsize::new(0));
    let global_observed = Arc::new(AtomicUsize::new(0));
    add(&env, counting("t", &body_calls));
    {
        let policy_calls = policy_calls.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, next| {
                policy_calls.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            })
            .unwrap();
    }
    {
        let scoped_observed = scoped_observed.clone();
        a.scope
            .ctx
            .on_contained::<ToolsResult>(move |event| {
                scoped_observed.fetch_add(1, Ordering::SeqCst);
                assert_eq!(event.exec.arguments, Value::Null);
                assert!(event.result.is_error());
                Ok(())
            })
            .unwrap();
    }
    {
        let global_observed = global_observed.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |_| {
                global_observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
    }

    let mut scoped = input("non-json", "t", json!({ "invalid": -0.0 }));
    scoped.agent = Some(a.agent.clone());
    let scoped_result = execute_tool_fixture(&env.ctx, scoped).await;
    let subjectless_result = execute_tool_fixture(
        &env.ctx,
        input("non-json-subjectless", "t", json!({ "invalid": -0.0 })),
    )
    .await;
    assert!(scoped_result.is_error());
    assert!(
        scoped_result
            .text()
            .contains("losslessly JSON-serializable")
    );
    assert!(subjectless_result.is_error());
    assert_eq!(
        (
            policy_calls.load(Ordering::SeqCst),
            body_calls.load(Ordering::SeqCst),
            scoped_observed.load(Ordering::SeqCst),
            global_observed.load(Ordering::SeqCst)
        ),
        (0, 0, 1, 2)
    );
}

#[tokio::test]
async fn rejects_lossy_arguments_before_policy_or_dispatch_with_one_observation() {
    let env = setup().await;
    let (policy_calls, body_calls, observed) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    add(&env, counting("t", &body_calls));
    {
        let policy_calls = policy_calls.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, next| {
                policy_calls.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            })
            .unwrap();
    }
    {
        let observed = observed.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |event| {
                observed.fetch_add(1, Ordering::SeqCst);
                assert_eq!(event.exec.arguments, Value::Null);
                assert!(event.result.is_error());
                Ok(())
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("bad-arguments", "t", json!([-0.0]))).await;
    assert!(result.is_error());
    assert_eq!(
        result.content(),
        &[text(
            "Error: tool execution arguments must be losslessly JSON-serializable"
        )]
    );
    assert_eq!(
        (
            policy_calls.load(Ordering::SeqCst),
            body_calls.load(Ordering::SeqCst),
            observed.load(Ordering::SeqCst)
        ),
        (0, 0, 1)
    );
}

#[tokio::test]
async fn notifies_every_tools_result_observer_with_the_final_outcome_and_contains_failures() {
    let env = setup().await;
    let a = mint(&env, None);
    add(&env, tool("t"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
            next.run().await?;
            Ok(ToolExecutionResult::Failure(
                hoh_tools::ToolExecutionFailure::new(
                    hoh_tools::ToolFailure {
                        message: "outer failure".into(),
                        info: None,
                    },
                    vec![text("outer failure")],
                ),
            ))
        })
        .unwrap();
    {
        let seen = seen.clone();
        a.scope
            .ctx
            .on_contained::<ToolsResult>(move |event| {
                lock(&seen).push(event.result.is_error());
                Ok(())
            })
            .unwrap();
    }
    env.ctx
        .on_contained::<ToolsResult>(|_| Err("coercion trap".into()))
        .unwrap();
    env.ctx
        .on_contained::<ToolsResult>(|_| Err("async observer failure".into()))
        .unwrap();
    {
        let seen = seen.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |event| {
                lock(&seen).push(event.result.is_error());
                Ok(())
            })
            .unwrap();
    }

    let mut request = input("final", "t", json!({}));
    request.agent = Some(a.agent.clone());
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert!(result.is_error());
    assert_eq!(result.content(), &[text("outer failure")]);
    assert_eq!(*lock(&seen), vec![true, true]);
    let warnings: Vec<_> = env
        .kernel
        .root()
        .logger()
        .buffer()
        .into_iter()
        .map(|record| record.message)
        .collect();
    assert_eq!(
        warnings
            .iter()
            .filter(|message| message.contains("tools/result observer failed"))
            .count(),
        2
    );
    assert!(
        warnings
            .iter()
            .any(|message| message.contains("coercion trap"))
    );
    assert!(
        warnings
            .iter()
            .any(|message| message.contains("async observer failure"))
    );
}
