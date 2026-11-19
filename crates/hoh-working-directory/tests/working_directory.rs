//! Port of `tests/working-directory.spec.ts` (the tool cases belong to `hoh-tool-working-directory`).

mod support;

use std::sync::Arc;

use hoh_agent::AssembleAgent;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::Level;
use hoh_session::{CreateSessionOptions, Session};
use hoh_session_projection::SessionProjections;
use hoh_system_prompt::{
    AssembleContext, AssembledContext, PromptPresentation, SystemPromptAssemble, SystemPromptKey,
    render_context_snapshot, render_prompt,
};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::json;
use support::{Fixture, Park, make_agent, texts};
use tokio::sync::oneshot;

fn message(error: &BoxError) -> String {
    error.to_string()
}

fn harness(error: &BoxError) -> &HarnessError {
    HarnessError::of(error.as_ref()).expect("a HarnessError")
}

fn change_events(session: &Session) -> Vec<String> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "working-directory/change")
        .map(|event| event.data()["cwd"].as_str().unwrap().to_owned())
        .collect()
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

async fn assemble_for(
    f: &Fixture,
    agent: &support::TestAgent,
) -> hoh_system_prompt::PromptAssembly {
    let prompt = f.ctx.get::<SystemPromptKey>().expect("system prompt");
    prompt
        .assemble(AssembleContext::new(None).with_extension(AssembleAgent(agent.agent.clone())))
        .await
        .expect("assembly")
}

fn park(f: &Fixture) -> (oneshot::Receiver<Option<AbortSignal>>, oneshot::Sender<()>) {
    let (entered, entered_rx) = oneshot::channel();
    let (release_tx, release) = oneshot::channel();
    *f.fs.park.lock().unwrap() = Some(Park { entered, release });
    (entered_rx, release_tx)
}

#[tokio::test]
async fn keeps_origin_metadata_and_other_sessions_independent_while_logging_the_changed_directory()
{
    let f = Fixture::new().await;
    let child = f.path(&["child"]);
    std::fs::create_dir(&child).unwrap();
    let sibling = f.create("sibling", Some(&f.root_text)).await;
    assert_eq!(
        f.service.set(&f.agent.agent, "child", None).await.unwrap(),
        child
    );
    assert_eq!(f.service.get(&f.agent.session()), child);
    assert_eq!(f.service.get(&sibling.session()), f.root_text);
    assert_eq!(f.agent.session().header()["cwd"], json!(f.root_text));
    assert_eq!(
        change_events(&f.agent.session()),
        std::slice::from_ref(&child)
    );
    let messages = f.agent.claim();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].source.kind, "working-directory");
    assert_eq!(
        texts(&messages),
        [format!(
            "The working directory changed from {} to {}.",
            json_string(&f.root_text),
            json_string(&child)
        )]
    );
    f.shutdown().await;
}

#[tokio::test]
async fn restores_directory_state_from_the_log() {
    let f = Fixture::new().await;
    let child = f.path(&["child"]);
    std::fs::create_dir(&child).unwrap();
    f.service.set(&f.agent.agent, &child, None).await.unwrap();
    let restored = f
        .store
        .create(
            &f.ctx,
            Some("restored".into()),
            CreateSessionOptions {
                meta: Some(json!({ "cwd": f.root_text })),
                seed: Some(f.agent.session().snapshot_events(None, None).to_vec()),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    assert_eq!(f.service.get(&restored), child);
    f.shutdown().await;
}

#[tokio::test]
async fn exposes_the_current_directory_in_wire_observations_after_replay_without_an_agent() {
    let f = Fixture::new().await;
    let directory = f.path(&["moved"]);
    std::fs::create_dir(&directory).unwrap();
    let without_origin = f
        .store
        .create(
            &f.ctx,
            Some("wire-without-origin".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    assert_eq!(f.service.recorded(&without_origin), None);
    assert_eq!(
        f.service.recorded(&f.agent.session()),
        Some(f.root_text.clone())
    );
    f.service
        .set(&f.agent.agent, &directory, None)
        .await
        .unwrap();
    let restored = f
        .store
        .create(
            &f.ctx,
            Some("wire-restored".into()),
            CreateSessionOptions {
                meta: Some(json!({ "cwd": f.root_text })),
                seed: Some(f.agent.session().snapshot_events(None, None).to_vec()),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    assert_eq!(f.service.recorded(&restored), Some(directory));
    assert_eq!(restored.header()["cwd"], json!(f.root_text));
    f.shutdown().await;
}

#[tokio::test]
async fn serializes_concurrent_relative_changes_and_recovers_after_a_rejected_change() {
    let f = Fixture::new().await;
    std::fs::create_dir_all(f.path(&["a", "b"])).unwrap();
    let first = tokio::spawn(f.service.set(&f.agent.agent, "a", None));
    let second = tokio::spawn(f.service.set(&f.agent.agent, "b", None));
    assert_eq!(first.await.unwrap().unwrap(), f.path(&["a"]));
    assert_eq!(second.await.unwrap().unwrap(), f.path(&["a", "b"]));
    let error = f
        .service
        .set(&f.agent.agent, "missing", None)
        .await
        .unwrap_err();
    assert!(message(&error).contains("directory does not exist"));
    assert_eq!(
        f.service.set(&f.agent.agent, "..", None).await.unwrap(),
        f.path(&["a"])
    );
    f.shutdown().await;
}

#[tokio::test]
async fn returns_to_the_original_project_when_the_selected_directory_disappears_and_injects_the_recovery()
 {
    let f = Fixture::new().await;
    let child = f.path(&["temporary"]);
    std::fs::create_dir(&child).unwrap();
    f.service.set(&f.agent.agent, &child, None).await.unwrap();
    assert_eq!(f.agent.claim().len(), 1);
    std::fs::remove_dir_all(&child).unwrap();
    assert_eq!(
        f.service.ensure(&f.agent.agent, None).await.unwrap(),
        f.root_text
    );
    assert_eq!(
        texts(&f.agent.claim()),
        [format!(
            "The working directory {} is unavailable. The working directory is now {}.",
            json_string(&child),
            json_string(&f.root_text)
        )]
    );
    f.shutdown().await;
}

#[tokio::test]
async fn reports_a_missing_original_project_and_permits_an_explicit_directory_change_to_recover() {
    let f = Fixture::new().await;
    let origin = f.path(&["original"]);
    std::fs::create_dir(&origin).unwrap();
    let agent = f.create("removed-origin", Some(&origin)).await;
    std::fs::remove_dir_all(&origin).unwrap();
    let error = f.service.ensure(&agent.agent, None).await.unwrap_err();
    assert!(message(&error).contains("directory does not exist"));
    let error = f.service.ensure(&agent.agent, None).await.unwrap_err();
    let cause = harness(&error);
    assert_eq!(cause.name(), "WorkingDirectoryError");
    assert_eq!(cause.code(), "WORKING_DIRECTORY_UNAVAILABLE");
    assert!(cause.message().contains("is also unavailable"));
    assert_eq!(
        f.service
            .set(&agent.agent, &f.root_text, None)
            .await
            .unwrap(),
        f.root_text
    );
    f.shutdown().await;
}

#[tokio::test]
async fn rejects_files_empty_paths_and_cancelled_changes_without_changing_state() {
    let f = Fixture::new().await;
    std::fs::write(f.path(&["file"]), "").unwrap();
    let error = f
        .service
        .set(&f.agent.agent, "file", None)
        .await
        .unwrap_err();
    assert!(message(&error).contains("directory does not exist"));
    let error = f
        .service
        .set(&f.agent.agent, "file", None)
        .await
        .unwrap_err();
    assert_eq!(harness(&error).name(), "WorkingDirectoryError");
    assert_eq!(harness(&error).code(), "WORKING_DIRECTORY_NOT_FOUND");
    let error = f.service.set(&f.agent.agent, "", None).await.unwrap_err();
    assert!(message(&error).contains("cd must not be empty"));
    let error = f.service.set(&f.agent.agent, "", None).await.unwrap_err();
    assert_eq!(harness(&error).name(), "WorkingDirectoryError");
    assert_eq!(harness(&error).code(), "WORKING_DIRECTORY_INVALID");
    let controller = AbortController::new();
    controller.abort_with("cancelled directory change");
    let signal = controller.signal();
    let error = f
        .service
        .set(&f.agent.agent, &f.root_text, Some(&signal))
        .await
        .unwrap_err();
    assert!(message(&error).contains("cancelled directory change"));
    assert_eq!(f.service.get(&f.agent.session()), f.root_text);
    assert!(change_events(&f.agent.session()).is_empty());
    f.shutdown().await;
}

#[tokio::test]
async fn keeps_unchanged_reads_and_changes_free_of_transition_notices() {
    let f = Fixture::new().await;
    f.service.ensure(&f.agent.agent, None).await.unwrap();
    f.service
        .set(&f.agent.agent, &f.root_text, None)
        .await
        .unwrap();
    assert!(f.agent.claim().is_empty());
    f.shutdown().await;
}

#[tokio::test]
async fn provides_required_user_context_instead_of_a_system_prompt_directory_including_recovery_during_assembly()
 {
    let f = Fixture::new().await;
    let child = f.path(&["temporary"]);
    std::fs::create_dir(&child).unwrap();
    f.service.set(&f.agent.agent, &child, None).await.unwrap();
    std::fs::remove_dir_all(&child).unwrap();
    let assembly = assemble_for(&f, &f.agent).await;
    assert!(
        render_context_snapshot(&assembly)
            .unwrap()
            .contains(&format!(
                "Current working directory: {}.",
                json_string(&f.root_text)
            ))
    );
    assert!(!render_prompt(&assembly).unwrap().contains(&f.root_text));
    let prompt = f.ctx.get::<SystemPromptKey>().unwrap();
    let bare = prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(render_context_snapshot(&bare).unwrap(), "");
    f.shutdown().await;
}

#[tokio::test]
async fn uses_and_records_the_configured_launch_fallback_for_a_session_without_origin_metadata() {
    let f = Fixture::new().await;
    let agent = f.create("without-origin", None).await;
    let fallback = f.service.default_directory().to_owned();
    assert_eq!(
        f.service.ensure(&agent.agent, None).await.unwrap(),
        fallback
    );
    assert_eq!(change_events(&agent.session()), [fallback]);
    f.shutdown().await;
}

#[tokio::test]
async fn rejects_a_relative_deployment_default_before_accepting_sessions() {
    let error =
        match Fixture::try_with_config(|_| json!({ "defaultDirectory": "relative/project" })).await
        {
            Err(error) => error,
            Ok(_) => panic!("a relative default must not start"),
        };
    assert!(
        error
            .to_string()
            .contains("defaultDirectory must be absolute")
    );
}

#[tokio::test]
async fn recovers_a_session_without_origin_metadata_to_its_configured_default() {
    let f = Fixture::with_config(|root| json!({ "defaultDirectory": root })).await;
    let agent = f.create("fallback-recovery", None).await;
    let temporary = f.path(&["temporary"]);
    std::fs::create_dir(&temporary).unwrap();
    f.service.set(&agent.agent, &temporary, None).await.unwrap();
    assert_eq!(agent.claim().len(), 1);
    std::fs::remove_dir_all(&temporary).unwrap();
    assert_eq!(
        f.service.ensure(&agent.agent, None).await.unwrap(),
        f.root_text
    );
    assert!(agent.session().header().get("cwd").is_none());
    assert_eq!(
        texts(&agent.claim()),
        [format!(
            "The working directory {} is unavailable. The working directory is now {}.",
            json_string(&temporary),
            json_string(&f.root_text)
        )]
    );
    f.shutdown().await;
}

async fn retains_required_context(remove: bool) {
    let f = Fixture::new().await;
    f.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(
            move |_event, next| async move {
                let assembled = next.run().await?;
                assembled.update(|data| {
                    data.contexts = if remove {
                        Vec::new()
                    } else {
                        vec![AssembledContext {
                            name: "working-directory:current".into(),
                            text: "outdated directory".into(),
                            interpolate: None,
                        }]
                    };
                });
                Ok(assembled)
            },
        )
        .unwrap();
    let assembled = assemble_for(&f, &f.agent).await;
    assert_eq!(
        assembled.contexts,
        [AssembledContext {
            name: "working-directory:current".into(),
            text: format!("Current working directory: {}.", json_string(&f.root_text)),
            interpolate: Some(false),
        }]
    );
    assert!(!render_prompt(&assembled).unwrap().contains(&f.root_text));
    f.shutdown().await;
}

#[tokio::test]
async fn retains_required_current_directory_context_after_an_assembly_transform_can_remove_it() {
    retains_required_context(true).await;
}

#[tokio::test]
async fn retains_required_current_directory_context_after_an_assembly_transform_can_replace_it() {
    retains_required_context(false).await;
}

async fn renders_variable_looking_names(remove_context: bool) {
    let f = Fixture::new().await;
    let directory = f.path(&["{{project}}-{{unknown}}-{{malformed-name}}"]);
    std::fs::create_dir(&directory).unwrap();
    let prompt = f.ctx.get::<SystemPromptKey>().unwrap();
    prompt
        .variable(&f.ctx, "project", |_| {
            Ok(Some("wrong-directory".to_owned()))
        })
        .unwrap();
    if remove_context {
        f.ctx
            .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(
                |_event, next| async move {
                    let assembled = next.run().await?;
                    assembled.update(|data| data.contexts.clear());
                    Ok(assembled)
                },
            )
            .unwrap();
    }
    f.service
        .set(&f.agent.agent, &directory, None)
        .await
        .unwrap();
    let assembly = assemble_for(&f, &f.agent).await;
    let snapshot = render_context_snapshot(&assembly).unwrap();
    assert!(snapshot.contains(&format!(
        "Current working directory: {}.",
        json_string(&directory)
    )));
    assert!(!snapshot.contains("wrong-directory"));
    f.shutdown().await;
}

#[tokio::test]
async fn renders_variable_looking_directory_names_literally_after_context_removal_is_false() {
    renders_variable_looking_names(false).await;
}

#[tokio::test]
async fn renders_variable_looking_directory_names_literally_after_context_removal_is_true() {
    renders_variable_looking_names(true).await;
}

#[tokio::test]
async fn does_not_commit_a_cancelled_filesystem_lookup_and_lets_the_next_queued_change_proceed() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.path(&["first"])).unwrap();
    std::fs::create_dir(f.path(&["second"])).unwrap();
    let (entered, release) = park(&f);
    let controller = AbortController::new();
    let cancelled = tokio::spawn(f.service.set(
        &f.agent.agent,
        "first",
        Some(&controller.signal()),
    ));
    entered.await.unwrap();
    let queued = tokio::spawn(f.service.set(&f.agent.agent, "second", None));
    controller.abort_with("cancelled while checking directory");
    release.send(()).unwrap();
    let error = cancelled.await.unwrap().unwrap_err();
    assert!(message(&error).contains("cancelled while checking directory"));
    assert_eq!(queued.await.unwrap().unwrap(), f.path(&["second"]));
    assert_eq!(change_events(&f.agent.session()), [f.path(&["second"])]);
    assert_eq!(f.agent.claim().len(), 1);
    f.shutdown().await;
}

async fn aborts_and_drains(operation: &str) {
    let f = Fixture::new().await;
    let (entered, release) = park(&f);
    let pending = if operation == "ensure" {
        tokio::spawn(f.service.ensure(&f.agent.agent, None))
    } else {
        tokio::spawn(f.service.set(&f.agent.agent, &f.root_text, None))
    };
    let signal = entered
        .await
        .unwrap()
        .expect("directory probe requires a cancellation signal");
    let disposal = tokio::spawn(f.directory_fiber.dispose());
    signal.cancelled().await;
    // Flush disposal's reactions while the provider remains parked.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(signal.aborted());
    assert!(!disposal.is_finished());
    release.send(()).unwrap();
    let error = pending.await.unwrap().unwrap_err();
    assert!(message(&error).contains("working-directory service disposed"));
    disposal.await.unwrap();
    assert!(change_events(&f.agent.session()).is_empty());
    assert!(f.agent.claim().is_empty());
    let error = f.service.ensure(&f.agent.agent, None).await.unwrap_err();
    assert!(message(&error).contains("working-directory service disposed"));
    let error = f.service.ensure(&f.agent.agent, None).await.unwrap_err();
    assert_eq!(harness(&error).name(), "WorkingDirectoryError");
    assert_eq!(harness(&error).code(), "WORKING_DIRECTORY_UNAVAILABLE");
    assert_eq!(*f.fs.stats.lock().unwrap(), 1);
    f.shutdown().await;
}

#[tokio::test]
async fn aborts_and_drains_pending_ensure_before_disposal_completes() {
    aborts_and_drains("ensure").await;
}

#[tokio::test]
async fn aborts_and_drains_pending_set_before_disposal_completes() {
    aborts_and_drains("set").await;
}

#[tokio::test]
async fn refuses_an_agent_disposed_during_directory_inspection_without_publishing_a_change() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.path(&["child"])).unwrap();
    let (entered, release) = park(&f);
    let pending = tokio::spawn(f.service.set(&f.agent.agent, "child", None));
    entered.await.unwrap();
    f.agent.fiber.dispose().await;
    release.send(()).unwrap();
    let error = pending.await.unwrap().unwrap_err();
    assert!(message(&error).contains("inactive context"));
    assert!(change_events(&f.agent.session()).is_empty());
    let error = f.service.ensure(&f.agent.agent, None).await.unwrap_err();
    assert!(message(&error).contains("inactive context"));
    assert_eq!(*f.fs.stats.lock().unwrap(), 1);
    f.shutdown().await;
}

#[tokio::test]
async fn returns_committed_state_and_warns_when_the_notice_cannot_be_queued() {
    let f = Fixture::new().await;
    let child = f.path(&["child"]);
    std::fs::create_dir(&child).unwrap();
    f.agent
        .fake
        .fail_inject
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.service.set(&f.agent.agent, &child, None).await.unwrap(),
        child
    );
    assert_eq!(f.service.get(&f.agent.session()), child);
    let expected = format!(
        "working-directory: committed {}, but could not queue its notice: Error: inbox unavailable",
        json_string(&child)
    );
    assert!(
        f.ctx
            .logger()
            .buffer()
            .iter()
            .any(|record| record.level == Level::Warn && record.message == expected),
        "missing warning: {expected}"
    );
    let restored = make_agent(
        &f.ctx,
        &f.store,
        &f.inbox,
        "notice-failure-restored",
        Some(&f.root_text),
    )
    .await;
    drop(restored);
    let session = f
        .store
        .create(
            &f.ctx,
            Some("notice-failure-seeded".into()),
            CreateSessionOptions {
                meta: Some(json!({ "cwd": f.root_text })),
                seed: Some(f.agent.session().snapshot_events(None, None).to_vec()),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    let seeded = f.agent_for(session, f.ctx.clone(), f.agent.fiber.clone());
    let assembly = assemble_for(&f, &seeded).await;
    assert!(
        render_context_snapshot(&assembly)
            .unwrap()
            .contains(&format!(
                "Current working directory: {}.",
                json_string(&child)
            ))
    );
    let _ = Arc::new(());
    f.shutdown().await;
}

#[tokio::test]
async fn the_shared_projection_registry_carries_the_directory_in_snapshots_after_set() {
    let f = Fixture::new().await;
    let registry = f.ctx.get::<SessionProjections>().expect("registry mounted");
    assert_eq!(f.directory_fiber.state(), hoh_kernel::FiberState::Active);
    let snapshot = |session: &Session| {
        registry
            .snapshot(session, None)
            .expect("snapshot")
            .values
            .get("workingDirectory")
            .map(|value| (**value).clone())
    };
    // The Session header records the original project: that is the projected state.
    assert_eq!(snapshot(&f.agent.session()), Some(json!(f.root_text)));
    let child = f.path(&["child"]);
    std::fs::create_dir(&child).unwrap();
    f.service.set(&f.agent.agent, "child", None).await.unwrap();
    assert_eq!(snapshot(&f.agent.session()), Some(json!(child)));
    // A Session with no recorded directory projects `null` until one is recorded.
    let bare = f.create("snapshot-without-origin", None).await;
    assert_eq!(snapshot(&bare.session()), Some(json!(null)));
    f.service.set(&bare.agent, &child, None).await.unwrap();
    assert_eq!(snapshot(&bare.session()), Some(json!(child)));
    f.shutdown().await;
}

mod disposal_with_unpolled_futures {
    use super::*;
    use std::time::Duration;

    async fn disposed_within_a_bound(f: &Fixture) {
        tokio::time::timeout(Duration::from_secs(5), f.directory_fiber.dispose())
            .await
            .expect("disposal must not wait for a future nobody polls");
    }

    /// TS: the promise is already running, the abort rejects it and disposal finishes.
    #[tokio::test]
    async fn a_held_unpolled_future_does_not_stall_disposal() {
        let f = Fixture::new().await;
        let held = f.service.set(&f.agent.agent, "child", None);
        disposed_within_a_bound(&f).await;
        let error = held.await.unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).map(HarnessError::code),
            Some("WORKING_DIRECTORY_UNAVAILABLE")
        );
        assert!(change_events(&f.agent.session()).is_empty());
        f.shutdown().await;
    }

    /// A polled operation queued behind a held, unpolled one is released by the disposal too.
    /// The link left by a change cancelled in the queue does not outlive the service.
    #[tokio::test]
    async fn relay_tasks_do_not_survive_dispose() {
        let f = Fixture::new().await;
        let held = f.service.set(&f.agent.agent, "first", None);
        let controller = AbortController::new();
        let second = tokio::spawn(f.service.set(
            &f.agent.agent,
            "second",
            Some(&controller.signal()),
        ));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        controller.abort_with("cancelled in the queue");
        second.await.unwrap().unwrap_err();
        assert_eq!(
            f.service.live_relay_tasks(),
            1,
            "the cancelled link is relayed"
        );
        disposed_within_a_bound(&f).await;
        assert_eq!(
            f.service.live_relay_tasks(),
            0,
            "no relay outlives disposal"
        );
        drop(held);
        f.shutdown().await;
    }

    #[tokio::test]
    async fn a_polled_operation_behind_an_unpolled_one_is_released_by_disposal() {
        let f = Fixture::new().await;
        let held = f.service.set(&f.agent.agent, "first", None);
        let behind = tokio::spawn(f.service.ensure(&f.agent.agent, None));
        tokio::task::yield_now().await;
        disposed_within_a_bound(&f).await;
        let error = behind.await.unwrap().unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).map(HarnessError::code),
            Some("WORKING_DIRECTORY_UNAVAILABLE")
        );
        drop(held);
        f.shutdown().await;
    }
}

/// working-directory/src/index.ts:239-252: a cancelled queued change rejects at once, but it stays a
/// link of the chain until its predecessor settles, so the next change still waits for the first.
#[tokio::test]
async fn cancelled_queued_change_preserves_predecessor_barrier() {
    let f = Fixture::new().await;
    std::fs::create_dir_all(f.path(&["a", "c"])).unwrap();
    let (entered, release) = park(&f);
    let first = tokio::spawn(f.service.set(&f.agent.agent, "a", None));
    entered.await.unwrap();
    assert_eq!(*f.fs.stats.lock().unwrap(), 1);

    let controller = AbortController::new();
    let second = tokio::spawn(f.service.set(
        &f.agent.agent,
        "unrelated",
        Some(&controller.signal()),
    ));
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    controller.abort_with("cancelled in the queue");
    let error = second.await.unwrap().unwrap_err();
    assert!(message(&error).contains("cancelled in the queue"));

    // The third change is queued after the cancelled one; it must not reach the filesystem while the
    // first is still checking its directory.
    let third = tokio::spawn(f.service.set(&f.agent.agent, "c", None));
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(*f.fs.stats.lock().unwrap(), 1, "the barrier holds");
    assert!(!third.is_finished());

    release.send(()).unwrap();
    assert_eq!(first.await.unwrap().unwrap(), f.path(&["a"]));
    assert_eq!(third.await.unwrap().unwrap(), f.path(&["a", "c"]));
    assert_eq!(
        change_events(&f.agent.session()),
        [f.path(&["a"]), f.path(&["a", "c"])]
    );
    assert_eq!(f.service.get(&f.agent.session()), f.path(&["a", "c"]));
    f.shutdown().await;
}
