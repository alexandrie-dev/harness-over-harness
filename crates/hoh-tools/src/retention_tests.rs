//! White-box retention tests for the executor table (`tools/src/index.ts:107,344–347`: a `WeakMap`
//! keyed by scope, so a dead scope's registration is collectable).

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_kernel::Kernel;
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use hoh_system_prompt::SystemPrompt;
use serde_json::json;

use crate::{
    ToolBatchInput, ToolBatchResult, ToolDefinition, ToolExecutor, ToolModelInput, ToolRuntime,
    Tools, ToolsService, lock, shim::BoxFuture,
};

struct Plain;

impl ToolExecutor for Plain {
    fn describe(&self, _: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
        Err("not used".into())
    }
    fn execute(&self, _: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        Box::pin(async { Err("not used".into()) })
    }
}

fn scoped_entries(service: &ToolsService) -> usize {
    lock(&service.0.executors).scoped.len()
}

async fn setup() -> (Kernel, hoh_kernel::Context, ToolsService) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .expect("system prompt mounts")
        .settled()
        .await
        .expect("system prompt settles");
    ctx.plugin::<ToolRuntime>(json!({}))
        .expect("tools mount")
        .settled()
        .await
        .expect("tools settle");
    let tools = ctx.get::<Tools>().expect("tools service");
    (kernel, ctx, (*tools).clone())
}

/// Many cycles of mint a scope, register its executor, dispose and drop the scope: the table
/// does not grow, whichever cycle's registration prunes the previous one.
#[tokio::test]
async fn dead_scope_entries_do_not_accumulate() {
    let (kernel, root, service) = setup().await;
    for cycle in 0..200 {
        let key = ScopeKey::new();
        let scope = create_scope(&root, key.clone(), CreateScopeOptions::default()).unwrap();
        let disposer = service.set_executor(&scope.ctx, Arc::new(Plain)).unwrap();
        assert!(
            scoped_entries(&service) <= 2,
            "cycle {cycle}: {} entries",
            scoped_entries(&service)
        );
        disposer.dispose().await;
        scope.dispose().await;
        drop(scope);
        drop(key);
    }
    // A selection prunes too: nothing is left of the 200 dead scopes.
    assert!(service.select_executor(None).is_err());
    assert_eq!(scoped_entries(&service), 0);
    kernel.shutdown().await;
}

/// A live scope keeps its unavailable selection after the registration is disposed: it cannot
/// fall through to a deployment default, and the entry survives pruning.
#[tokio::test]
async fn a_live_scope_keeps_its_unavailable_tombstone() {
    let (kernel, root, service) = setup().await;
    let default = service.set_executor(&root, Arc::new(Plain)).unwrap();
    let key = ScopeKey::new();
    let scope = create_scope(&root, key.clone(), CreateScopeOptions::default()).unwrap();
    let disposer = service.set_executor(&scope.ctx, Arc::new(Plain)).unwrap();
    assert!(service.select_executor(Some(&key)).is_ok());
    disposer.dispose().await;
    // Pruning runs on both of these; the live scope's entry must survive them.
    let other = ScopeKey::new();
    assert!(service.select_executor(Some(&other)).is_ok());
    assert!(service.select_executor(Some(&key)).is_err());
    assert_eq!(scoped_entries(&service), 1);
    default.dispose().await;
    scope.dispose().await;
    kernel.shutdown().await;
}

/// A fresh key never lands on a dead key's entry, even while the dead key's weak handle is still
/// in the table (no pruning has happened in between).
#[tokio::test]
async fn a_fresh_key_does_not_alias_a_dead_entry() {
    let (kernel, root, service) = setup().await;
    let dead = ScopeKey::new();
    let scope = create_scope(&root, dead.clone(), CreateScopeOptions::default()).unwrap();
    let disposer = service.set_executor(&scope.ctx, Arc::new(Plain)).unwrap();
    disposer.dispose().await;
    scope.dispose().await;
    drop(scope);
    drop(dead);
    for _ in 0..100 {
        let fresh = ScopeKey::new();
        // Lookup alone: the dead entry (an unavailable tombstone) must not answer for `fresh`,
        // and the missing registration is the plain "unavailable" error either way.
        assert!(service.select_executor(Some(&fresh)).is_err());
        assert_eq!(scoped_entries(&service), 0);
    }
    kernel.shutdown().await;
}
