//! Port of `tests/service.spec.ts`.

mod support;

use std::time::Duration;

use hoh_agent::{AgentCreated, AgentDisposed, SessionStartSource};
use hoh_file_reference::{
    FILE_REFERENCE_PROMPT, FileReferenceCandidate, FileReferenceKind, FileReferences,
};
use hoh_file_reference_local::LocalFileReferenceService;
use hoh_session::SurfaceIntent;
use hoh_system_prompt::{AssembleContext, SystemPromptKey, render_prompt};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_testkit::{ContentToolFixtureOptions, define_content_tool_fixture};
use hoh_tools::{ToolRegistrationOptions, Tools};
use serde_json::{Value, json};
use support::{Fixture, TestAgent};

fn file(path: &str) -> FileReferenceCandidate {
    FileReferenceCandidate {
        path: path.to_owned(),
        kind: FileReferenceKind::File,
    }
}

fn signal() -> AbortSignal {
    AbortController::new().signal()
}

async fn list(f: &Fixture, agent: &TestAgent, query: &str) -> Vec<FileReferenceCandidate> {
    let service = f.ctx.get::<FileReferences>().expect("fileReferences");
    service.list(&agent.agent, query, &signal()).await.unwrap()
}

async fn prompt(f: &Fixture, agent: &TestAgent) -> String {
    let system_prompt = f.ctx.get::<SystemPromptKey>().expect("system prompt");
    let assembly = system_prompt
        .assemble(AssembleContext::new(Some(agent.agent.scope_key())))
        .await
        .unwrap();
    render_prompt(&assembly).unwrap()
}

fn register_read_tool(f: &Fixture) -> hoh_kernel::Disposer {
    let tools = f.ctx.get::<Tools>().expect("tools");
    let definition = define_content_tool_fixture(ContentToolFixtureOptions::new(
        "read",
        "read a file",
        json!({}),
        |_, _| async { Ok(Vec::new()) },
    ))
    .unwrap();
    tools
        .register(&f.ctx, definition, ToolRegistrationOptions::default())
        .unwrap()
}

async fn eventually<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..1_000 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition did not hold within the wait");
}

/// The text of the failure that mounting the plugin with `config` produces.
async fn mount_failure(f: &Fixture, config: Value) -> String {
    match f.ctx.plugin::<LocalFileReferenceService>(config) {
        Err(error) => error.to_string(),
        Ok(fiber) => fiber.settled().await.unwrap_err().to_string(),
    }
}

#[tokio::test]
async fn defers_guidance_until_optional_prompt_and_tool_services_become_available() {
    // The working directory the service depends on needs the system prompt, so only the tool
    // registry, the other optional service of the guidance scope, can arrive late here.
    let f = Fixture::new().await;
    f.ctx
        .plugin::<LocalFileReferenceService>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(f.ctx.get::<FileReferences>().is_some());
    let root = f.workspace("dsh-file-reference-service-", &["README.md"]);
    let agent = f
        .register_agent("deferred-prompt", Some(&root.to_string_lossy()))
        .await;
    assert!(
        f.ctx
            .get::<hoh_agent::Agents>()
            .unwrap()
            .get(agent.agent.id())
            .is_some()
    );
    f.mount_tools().await;
    let _read = register_read_tool(&f);
    assert!(prompt(&f, &agent).await.contains(FILE_REFERENCE_PROMPT));
    f.shutdown().await;
}

#[tokio::test]
async fn serves_the_addressed_workspace_and_installs_read_tool_guidance_for_existing_agents() {
    let f = Fixture::new().await;
    f.mount_tools().await;
    let root = f.workspace("dsh-file-reference-service-", &["README.md"]);
    let agent = f
        .register_agent("file-reference-agent", Some(&root.to_string_lossy()))
        .await;
    let fiber = f
        .ctx
        .plugin::<LocalFileReferenceService>(json!({
            "maxResults": 5,
            "maxEntries": 100,
            "excludedDirectories": [".git"],
        }))
        .unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(list(&f, &agent, "README").await, vec![file("README.md")]);
    assert!(!prompt(&f, &agent).await.contains(FILE_REFERENCE_PROMPT));

    let _read = register_read_tool(&f);
    assert!(prompt(&f, &agent).await.contains(FILE_REFERENCE_PROMPT));
    fiber.dispose().await;
    assert!(!prompt(&f, &agent).await.contains(FILE_REFERENCE_PROMPT));
    f.shutdown().await;
}

#[tokio::test]
async fn rebuilds_completion_from_the_current_directory_after_a_session_directory_change() {
    let f = Fixture::new().await;
    let root = f.workspace("dsh-file-reference-service-", &["README.md"]);
    let agent = f
        .register_agent("file-reference-agent", Some(&root.to_string_lossy()))
        .await;
    let next_root = f.workspace("dsh-file-reference-next-", &["NEXT.md"]);
    f.ctx
        .plugin::<LocalFileReferenceService>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert_eq!(list(&f, &agent, "").await, vec![file("README.md")]);
    let original = agent.session().header()["cwd"].clone();
    f.service
        .set(&agent.agent, &next_root.to_string_lossy(), None)
        .await
        .unwrap();
    assert_eq!(list(&f, &agent, "").await, vec![file("NEXT.md")]);
    assert_eq!(agent.session().header()["cwd"], original);
    f.shutdown().await;
}

#[tokio::test]
async fn invalidates_cached_searches_after_tool_results_and_disposes_them_with_the_agent() {
    let f = Fixture::new().await;
    let root = f.workspace("dsh-file-reference-service-", &["README.md"]);
    let agent = f
        .register_agent("file-reference-agent", Some(&root.to_string_lossy()))
        .await;
    f.ctx
        .plugin::<LocalFileReferenceService>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert_eq!(list(&f, &agent, "README").await, vec![file("README.md")]);
    std::fs::write(root.join("later.md"), "later").unwrap();

    // Another event type does not invalidate: the settled index is still the answer.
    agent
        .session()
        .append(
            "assistant/message",
            json!({}),
            Some(SurfaceIntent::append()),
        )
        .expect("other event");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(list(&f, &agent, "later").await, vec![]);
    // Neither does the event of a Session that no agent owns.
    let orphan = f
        .store
        .create(
            &f.ctx,
            Some("file-reference-orphan".into()),
            Default::default(),
        )
        .unwrap();
    orphan
        .append("tool/result", json!({}), Some(SurfaceIntent::append()))
        .expect("other event");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(list(&f, &agent, "later").await, vec![]);

    // A tool result of the agent's own Session does.
    agent
        .session()
        .append("tool/result", json!({}), Some(SurfaceIntent::append()))
        .expect("tool/result event");
    eventually(|| async { list(&f, &agent, "later").await == vec![file("later.md")] }).await;
    f.shutdown().await;
}

#[tokio::test]
async fn installs_guidance_for_agents_announced_after_the_service_and_validates_deployment_tunables()
 {
    let f = Fixture::new().await;
    f.mount_tools().await;
    f.ctx
        .plugin::<LocalFileReferenceService>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let root = f.workspace("dsh-file-reference-service-", &["README.md"]);
    let agent = f
        .register_agent("file-reference-agent", Some(&root.to_string_lossy()))
        .await;
    assert_eq!(list(&f, &agent, "").await, vec![file("README.md")]);
    let _read = register_read_tool(&f);
    assert!(prompt(&f, &agent).await.contains(FILE_REFERENCE_PROMPT));
    f.shutdown().await;

    for (config, name) in [
        (json!({ "maxResults": 0 }), "maxResults"),
        (json!({ "maxEntries": 1.5 }), "maxEntries"),
        (
            json!({ "excludedDirectories": ["nested/name"] }),
            "excludedDirectories",
        ),
        (json!({ "maxResults": 1.5 }), "maxResults"),
        (json!({ "maxEntries": 0 }), "maxEntries"),
        (
            json!({ "excludedDirectories": [""] }),
            "excludedDirectories",
        ),
        (
            json!({ "excludedDirectories": ["nested\\name"] }),
            "excludedDirectories",
        ),
    ] {
        let f = Fixture::new().await;
        let message = mount_failure(&f, config.clone()).await;
        assert!(message.contains(name), "{config}: {message}");
        f.shutdown().await;
    }
}

#[tokio::test]
async fn deduplicates_lifecycle_announcements_and_falls_back_to_the_process_cwd() {
    let f = Fixture::new().await;
    f.ctx
        .plugin::<LocalFileReferenceService>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let agent = f.register_agent("cwd-fallback", None).await;
    let before = f.kernel.snapshot().await.fibers.len();
    f.ctx
        .serial::<AgentCreated, ()>(AgentCreated {
            agent: agent.agent.clone(),
            source: SessionStartSource::Startup,
            signal: None,
        })
        .await
        .unwrap();
    assert_eq!(f.kernel.snapshot().await.fibers.len(), before);
    // The package directory is the process cwd of `cargo test`.
    assert_eq!(
        list(&f, &agent, "Cargo.toml").await,
        vec![file("Cargo.toml")]
    );
    f.shutdown().await;
}

#[tokio::test]
async fn disposes_a_search_with_its_agent_and_ignores_a_repeated_announcement() {
    let f = Fixture::new().await;
    let root = f.workspace("dsh-file-reference-service-", &["README.md"]);
    f.ctx
        .plugin::<LocalFileReferenceService>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let (agent, disposer) = f
        .register_disposable_agent("dispose-agent", Some(&root.to_string_lossy()))
        .await;
    assert_eq!(list(&f, &agent, "README").await, vec![file("README.md")]);
    disposer.dispose().await;
    assert!(
        f.ctx
            .get::<hoh_agent::Agents>()
            .unwrap()
            .get(agent.agent.id())
            .is_none()
    );

    // The announcement is idempotent: a second `agent/disposed` finds nothing to release.
    f.ctx
        .dispatch(AgentDisposed {
            agent: agent.agent.clone(),
        })
        .invoke(|_| panic!("listener failed"));
    f.shutdown().await;
}
