//! `waitForTreeWork` joins each child's result promise, which settles only after the parent notice
//! (`manager.ts` settlement observer); the external-run notice reads «Background subagent … finished». The Rust join must not return while the notice is pending.

use futures_util::future::BoxFuture;
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_error::BoxError;
use hoh_kernel::Kernel;
use hoh_llm::ContentBlock;
use hoh_session::SessionId;
use hoh_subagent::{
    ResolvedSubagentStartRequest, SubagentActivationSpec, SubagentCapabilities,
    SubagentDeliveryTarget, SubagentPlugin, SubagentProvider, SubagentResult, SubagentRun,
    SubagentStartRequest, Subagents,
};
use hoh_timeout::AbortController;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// A child whose teardown (`dispose`) stays pending until the test opens the gate.
struct SlowTeardownRun {
    id: SessionId,
    gate: Arc<Notify>,
    open: Arc<std::sync::atomic::AtomicBool>,
}

impl SubagentRun for SlowTeardownRun {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn result(&self) -> BoxFuture<'static, Result<SubagentResult, BoxError>> {
        Box::pin(async {
            Ok(SubagentResult::completed(vec![ContentBlock::text(
                "closing words",
            )]))
        })
    }
    fn dispose(&self) -> BoxFuture<'static, Result<(), BoxError>> {
        let gate = self.gate.clone();
        let open = self.open.clone();
        Box::pin(async move {
            loop {
                let notified = gate.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if open.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(());
                }
                notified.await;
            }
        })
    }
}

struct Provider {
    gate: Arc<Notify>,
    open: Arc<std::sync::atomic::AtomicBool>,
}

impl SubagentProvider for Provider {
    fn name(&self) -> &str {
        "slow"
    }
    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }
    fn inherits_parent_context(&self) -> bool {
        false
    }
    fn supports_start(&self) -> bool {
        true
    }
    fn start(
        &self,
        _request: ResolvedSubagentStartRequest,
    ) -> Option<BoxFuture<'static, Result<Arc<dyn SubagentRun>, BoxError>>> {
        let run: Arc<dyn SubagentRun> = Arc::new(SlowTeardownRun {
            id: SessionId::from("slow-child"),
            gate: self.gate.clone(),
            open: self.open.clone(),
        });
        Some(Box::pin(async move { Ok(run) }))
    }
}

fn notice_count(parent: &Arc<dyn hoh_agent::Agent>) -> usize {
    parent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.data().to_string().contains("Background subagent"))
        .count()
}

#[tokio::test]
async fn wait_for_tree_work_returns_only_after_the_parent_received_the_notice() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
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
    ctx.plugin::<SubagentPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let subagents = ctx.get::<Subagents>().unwrap();
    let gate = Arc::new(Notify::new());
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _registration = subagents
        .register_provider(Arc::new(Provider {
            gate: gate.clone(),
            open: open.clone(),
        }))
        .unwrap();
    let parent = harness.create(SessionId::from("parent")).await.unwrap();
    let activation = subagents
        .start_activation(SubagentActivationSpec {
            provider: "slow".into(),
            label: "task".into(),
            name: None,
            child_id: None,
            delivery: SubagentDeliveryTarget::Parent,
            request: SubagentStartRequest {
                cwd: None,
                agent_preset: None,
                label: None,
                prompt: vec![ContentBlock::text("work")],
                parent: parent.clone(),
                signal: AbortController::new().signal(),
                agent_options: None,
                output_schema: None,
                max_depth: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(notice_count(&parent), 0);
    // The teardown finishes a little later; the join must wait for the notice that follows it.
    let release = tokio::spawn({
        let (gate, open) = (gate.clone(), open.clone());
        async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            open.store(true, std::sync::atomic::Ordering::SeqCst);
            gate.notify_waiters();
        }
    });
    let joined = tokio::time::timeout(
        Duration::from_secs(10),
        subagents.wait_for_tree_work(&parent),
    )
    .await
    .expect("wait_for_tree_work never returned")
    .unwrap();
    assert!(joined);
    assert_eq!(
        notice_count(&parent),
        1,
        "the join returned before the parent received the notice"
    );
    release.await.unwrap();
    drop(activation);
    kernel.shutdown().await;
}
