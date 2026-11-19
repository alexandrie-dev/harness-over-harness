use hoh_agent::{
    AgentInboxClaimed, AgentInboxDiscarded, AgentInboxInserted, AgentOptions, AgentSessionMeta,
    InboxTarget, ModelSelection,
};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, ToolMode, ToolOptions, create_inbox_stub,
    mount_agent_loop_test_dependencies, mount_agent_loop_test_harness,
    provide_working_directory_fixture, unsupported_inbox, unsupported_model,
};
use hoh_error::BoxError;
use hoh_fs_local::FsLocalPlugin;
use hoh_kernel::{Context, Kernel, Plugin};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_session::{Session, SessionEvent, SessionId};
use hoh_session_projection::SessionProjections;
use hoh_system_prompt::{
    AssembleContext, Config as SystemPromptConfig, SystemPromptKey, render_prompt,
};
use hoh_working_directory::WorkingDirectoryKey;
use serde_json::json;
use std::{
    fs,
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

async fn mount<P: Plugin>(ctx: &Context, config: serde_json::Value) -> Result<(), BoxError> {
    let fiber = ctx.plugin::<P>(config)?;
    fiber.settled().await?;
    Ok(())
}

fn message(text: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "hoh-agent-loop-testkit-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[tokio::test]
async fn rejects_an_invalid_program_concurrency_limit_during_prerequisite_mounting() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let error = mount_agent_loop_test_dependencies(
        &ctx,
        AgentLoopTestDependenciesOptions {
            tools: Some(ToolOptions {
                mode: ToolMode::Ptc,
                max_parallel_sub_calls: Some(0),
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("maxParallelSubCalls"));
    kernel.shutdown().await;
}

#[test]
fn rejects_reads_and_changes_through_an_unsupported_agent_stub_model() {
    let model = unsupported_model();
    let panic = std::panic::catch_unwind(AssertUnwindSafe(|| model.current()));
    assert!(panic.is_err());
    let error = model
        .select(&ModelSelection {
            provider: "mock".into(),
            model: "mock".into(),
            reasoning_effort: None,
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "this test Agent does not support model selection"
    );
}

#[test]
fn rejects_mutations_through_an_unsupported_agent_stub_inbox() {
    let inbox = unsupported_inbox();
    assert!(inbox.next_turn().is_empty());
    assert!(inbox.next_step().is_empty());
    assert_eq!(
        inbox.clear().unwrap_err().to_string(),
        "this test Agent does not support Inbox mutations"
    );
}

#[tokio::test]
async fn mounts_a_configurable_prerequisite_spine_and_the_production_agent_loop() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(
        &ctx,
        AgentLoopTestDependenciesOptions {
            system_prompt: Some(SystemPromptConfig {
                persona_prefix: "Test persona.".into(),
                ..Default::default()
            }),
            tools: Some(ToolOptions::default()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let assembly = ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(AssembleContext::new(None))
        .await
        .unwrap();
    assert!(render_prompt(&assembly).unwrap().contains("Test persona."));
    mount_agent_loop_test_harness(&ctx).await.unwrap();
    kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_an_existing_filesystem_when_enabling_working_directories() {
    let root = temp_root("filesystem");
    fs::create_dir_all(&root).unwrap();
    let canonical = fs::canonicalize(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount::<FsLocalPlugin>(&ctx, json!({"cwd":canonical}))
        .await
        .unwrap();
    mount_agent_loop_test_dependencies(
        &ctx,
        AgentLoopTestDependenciesOptions {
            working_directory: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
    let agent = harness
        .create_with(
            SessionId::from("existing-filesystem"),
            AgentOptions::default(),
            AgentSessionMeta {
                cwd: Some(canonical.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        ctx.get::<WorkingDirectoryKey>()
            .unwrap()
            .ensure(&agent, None)
            .await
            .unwrap(),
        canonical
    );
    kernel.shutdown().await;
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn provides_a_mutable_in_memory_inbox_stub_for_structural_agent_tests() {
    let inbox = create_inbox_stub();
    let first_turn = message("first turn");
    let second_turn = message("second turn");
    let first_step = message("first step");
    let edited_turn = message("edited turn");
    let edited_step = message("edited step");

    inbox
        .append(InboxTarget::NextTurn, first_turn.clone())
        .unwrap();
    inbox
        .prepend(InboxTarget::NextTurn, second_turn.clone())
        .unwrap();
    inbox
        .append(InboxTarget::NextStep, first_step.clone())
        .unwrap();
    assert_eq!(inbox.next_turn(), [second_turn, first_turn.clone()]);
    assert_eq!(
        inbox.next_step().as_slice(),
        std::slice::from_ref(&first_step)
    );

    assert!(inbox.replace(&first_turn.id, edited_turn.clone()).unwrap());
    assert!(inbox.replace(&first_step.id, edited_step.clone()).unwrap());
    assert!(!inbox.replace(&first_turn.id, message("missing")).unwrap());
    assert!(!inbox.remove(&first_turn.id).unwrap());
    assert_eq!(
        inbox
            .splice(InboxTarget::NextTurn, -1.0, 1.0, vec![])
            .unwrap(),
        [edited_turn]
    );
    assert!(inbox.remove(&edited_step.id).unwrap());
    inbox.clear().unwrap();
    assert!(inbox.next_turn().is_empty());
    assert!(inbox.next_step().is_empty());
}

#[tokio::test]
async fn drives_durable_inbox_behavior_through_a_production_agent() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, Default::default())
        .await
        .unwrap();
    let harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
    let agent = harness
        .create(SessionId::from("agent-loop-testkit-inbox"))
        .await
        .unwrap();
    let turn = message("turn");
    let step = message("step");
    let inserted = Arc::new(Mutex::new(Vec::new()));
    let claimed = Arc::new(Mutex::new(Vec::new()));
    let discarded = Arc::new(Mutex::new(Vec::new()));
    let subject = agent.clone();
    let sink = inserted.clone();
    ctx.on_contained::<AgentInboxInserted>(move |event| {
        if Arc::ptr_eq(&event.agent, &subject) {
            sink.lock().unwrap().push(event.message.id.to_string());
        }
        Ok(())
    })
    .unwrap();
    let subject = agent.clone();
    let sink = claimed.clone();
    ctx.on_contained::<AgentInboxClaimed>(move |event| {
        if Arc::ptr_eq(&event.agent, &subject) {
            sink.lock()
                .unwrap()
                .push((event.message.id.to_string(), event.turn));
        }
        Ok(())
    })
    .unwrap();
    let subject = agent.clone();
    let sink = discarded.clone();
    ctx.on_contained::<AgentInboxDiscarded>(move |event| {
        if Arc::ptr_eq(&event.agent, &subject) {
            sink.lock().unwrap().push(event.message.id.to_string());
        }
        Ok(())
    })
    .unwrap();

    agent
        .inbox()
        .append(InboxTarget::NextTurn, turn.clone())
        .unwrap();
    agent
        .inbox()
        .append(InboxTarget::NextStep, step.clone())
        .unwrap();
    assert_eq!(
        *inserted.lock().unwrap(),
        [turn.id.to_string(), step.id.to_string()]
    );
    assert!(
        agent
            .inbox()
            .append(InboxTarget::NextStep, turn.clone())
            .unwrap_err()
            .to_string()
            .contains("is already pending")
    );

    let invalid_event = Arc::new(SessionEvent::from_value(json!({
        "type":"agent/inbox/spliced","seq":0,"time":1,
        "data":{"target":"next-turn","start":99,"inserted":[]}
    })));
    let invalid = Session::create(
        SessionId::from("invalid-persisted-inbox"),
        Some(&[invalid_event]),
        None,
        None,
        None,
    )
    .unwrap();
    assert!(
        ctx.get::<SessionProjections>()
            .unwrap()
            .checkpoint(&invalid)
            .unwrap_err()
            .to_string()
            .contains("invalid persisted inbox splice")
    );
    assert_eq!(
        harness.claim(&agent, InboxTarget::NextTurn, 3).unwrap(),
        [step.clone(), turn.clone()]
    );
    assert_eq!(
        *claimed.lock().unwrap(),
        [(step.id.to_string(), 3), (turn.id.to_string(), 3)]
    );
    assert!(discarded.lock().unwrap().is_empty());
    assert_eq!(
        agent
            .session()
            .snapshot_events(None, None)
            .iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        [
            "agent/inbox/spliced",
            "agent/inbox/spliced",
            "agent/inbox/spliced",
            "agent/inbox/spliced"
        ]
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn working_directory_fixture_publishes_the_production_service_key() {
    let root = temp_root("working-directory");
    fs::create_dir_all(&root).unwrap();
    let canonical = fs::canonicalize(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    provide_working_directory_fixture(&ctx, Some(&canonical))
        .await
        .unwrap();
    let session =
        Session::create(SessionId::from("fixture-directory"), None, None, None, None).unwrap();
    assert_eq!(
        ctx.get::<WorkingDirectoryKey>().unwrap().get(&session),
        canonical
    );
    kernel.shutdown().await;
    fs::remove_dir_all(root).unwrap();
}
