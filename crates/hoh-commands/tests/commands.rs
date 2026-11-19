mod common;

use common::{agent, agent_with_session, definition, event_data, harness};
use futures_util::future::BoxFuture;
use hoh_attachment::{
    AttachmentId, AttachmentStore, Attachments, FileAttachmentRef, ImageAttachmentLimits,
    ImageAttachmentRef, ImageMediaType, SaveImageAttachment, StoredImageAttachment,
};
use hoh_commands::{
    CommandDefinition, CommandDefinitionId, CommandInputDescriptor, CommandInvocation,
    CommandResult, CommandSubmitAttachment, CommandsChange, parse_command,
};
use hoh_error::BoxError;
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use hoh_session::{CreateSessionOptions, SessionEventEmitted, SessionSeq, SessionStore};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::json;
use std::{
    io,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn signal() -> AbortSignal {
    AbortController::new().signal()
}

#[test]
fn parses_supported_command_lines_without_normalizing_input() {
    for (line, name, input) in [
        ("/a", "a", ""),
        ("/abc-12_x tail", "abc-12_x", " tail"),
        ("/goal\t keep\nspacing", "goal", "\t keep\nspacing"),
        ("/x\r\n", "x", "\r\n"),
    ] {
        let parsed = parse_command(line).unwrap();
        assert_eq!(parsed.name, name);
        assert_eq!(parsed.raw_input, input);
    }
}

#[test]
fn rejects_lines_outside_the_ascii_command_grammar() {
    for line in [
        "",
        "goal",
        "/",
        "/Goal",
        "/1x",
        "/é",
        "/ok!",
        "/ok\u{00a0}x",
        " /goal",
        "/goal/path",
        "/goal🔥",
    ] {
        assert_eq!(parse_command(line), None, "{line:?}");
    }
}

#[tokio::test]
async fn lists_normalized_descriptors_without_handlers() {
    let h = harness();
    let mut command = definition("beta");
    command.definition_id = Some(CommandDefinitionId::from("owner/beta"));
    command.input = Some(CommandInputDescriptor {
        hint: " value ".into(),
        attachments: Some(false),
    });
    h.commands.register(&h.ctx, command).unwrap();
    h.commands.register(&h.ctx, definition("alpha")).unwrap();
    let listed = h.commands.list(&h.agent);
    assert_eq!(
        listed
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert_eq!(
        listed[1].definition_id.as_ref().unwrap().as_str(),
        "owner/beta"
    );
    assert_eq!(listed[1].input.as_ref().unwrap().hint, " value ");
    assert_eq!(listed[1].input.as_ref().unwrap().attachments, None);
}

#[tokio::test]
async fn finds_registered_definitions_by_exact_name() {
    let h = harness();
    h.commands.register(&h.ctx, definition("alpha")).unwrap();
    assert_eq!(h.commands.find(&h.agent, "alpha").unwrap().name, "alpha");
    assert!(h.commands.find(&h.agent, "Alpha").is_none());
}

#[tokio::test]
async fn scoped_commands_shadow_globals_only_for_the_matching_agent() {
    let h = harness();
    let key = ScopeKey::new();
    let scope = create_scope(&h.ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let scoped_agent = agent(&scope.ctx, key, "scoped");
    let other = agent(&h.ctx, ScopeKey::new(), "other");
    let mut global = CommandDefinition::new("plan", "global", |_| async {
        Ok(CommandResult::success(Some("global")))
    });
    global.definition_id = Some(CommandDefinitionId::from("global/plan"));
    global.description = "global".into();
    let mut local = CommandDefinition::new("plan", "local", |_| async {
        Ok(CommandResult::success(Some("scoped")))
    });
    local.description = "local".into();
    h.commands.register(&h.ctx, global).unwrap();
    h.commands.register(&scope.ctx, local).unwrap();
    assert_eq!(
        h.commands.find(&scoped_agent, "plan").unwrap().description,
        "local"
    );
    assert_eq!(
        h.commands.find(&other, "plan").unwrap().description,
        "global"
    );
    assert!(
        h.commands
            .list(&scoped_agent)
            .into_iter()
            .find(|descriptor| descriptor.name == "plan")
            .unwrap()
            .definition_id
            .is_none()
    );
    assert_eq!(
        h.commands
            .execute(scoped_agent.clone(), "/plan", vec![], signal())
            .await
            .unwrap()
            .unwrap()
            .result,
        CommandResult::success(Some("scoped"))
    );
    assert_eq!(
        h.commands
            .execute(other, "/plan", vec![], signal())
            .await
            .unwrap()
            .unwrap()
            .result,
        CommandResult::success(Some("global"))
    );
    scope.dispose().await;
    assert_eq!(
        h.commands
            .execute(scoped_agent, "/plan", vec![], signal())
            .await
            .unwrap()
            .unwrap()
            .result,
        CommandResult::success(Some("global"))
    );
}

#[tokio::test]
async fn disposing_a_scope_removes_its_commands() {
    let h = harness();
    let key = ScopeKey::new();
    let scope = create_scope(&h.ctx, key.clone(), Default::default()).unwrap();
    let scoped_agent = agent(&scope.ctx, key, "scoped");
    h.commands
        .register(&scope.ctx, definition("only-here"))
        .unwrap();
    assert!(h.commands.find(&scoped_agent, "only-here").is_some());
    scope.dispose().await;
    assert!(h.commands.find(&scoped_agent, "only-here").is_none());
}

#[tokio::test]
async fn registration_disposer_removes_the_command_from_find() {
    let h = harness();
    let dispose = h
        .commands
        .register(&h.ctx, definition("temporary"))
        .unwrap();
    assert!(h.commands.find(&h.agent, "temporary").is_some());
    dispose.dispose().await;
    assert!(h.commands.find(&h.agent, "temporary").is_none());
}

#[tokio::test]
async fn rejects_duplicate_names_with_scope_specific_diagnostics() {
    let h = harness();
    h.commands.register(&h.ctx, definition("same")).unwrap();
    let global = h
        .commands
        .register(&h.ctx, definition("same"))
        .err()
        .unwrap();
    assert_eq!(
        global.to_string(),
        "command \"same\" is already registered (for a per-agent variant, mount a command-injected plugin under that agent's `agent.ctx`)"
    );

    let scope = create_scope(&h.ctx, ScopeKey::new(), Default::default()).unwrap();
    h.commands.register(&scope.ctx, definition("same")).unwrap();
    let scoped = h
        .commands
        .register(&scope.ctx, definition("same"))
        .err()
        .unwrap();
    assert_eq!(
        scoped.to_string(),
        "command \"same\" is already registered in this scope"
    );
}

#[tokio::test]
async fn notifies_changes_and_contains_hostile_observers() {
    let h = harness();
    h.ctx
        .on_contained::<CommandsChange>(|_| panic!("observer failed"))
        .unwrap();
    h.ctx
        .on_contained_async::<CommandsChange, _>(|_| async {
            Err::<(), BoxError>(io::Error::other("observer rejected").into())
        })
        .unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();
    h.ctx
        .on_contained::<CommandsChange>(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    let dispose = h.commands.register(&h.ctx, definition("notify")).unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    dispose.dispose().await;
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    assert!(h.ctx.logger().buffer().iter().any(|record| {
        record.message.contains(
            "commands/change listener threw: Error: event listener panicked: observer failed",
        )
    }));
    assert!(h.ctx.logger().buffer().iter().any(|record| {
        record
            .message
            .contains("commands/change listener rejected: Error: observer rejected")
    }));
}

#[tokio::test]
async fn rejects_invalid_command_names() {
    let h = harness();
    for name in ["", "A", "two words", "é", "x/y"] {
        let failure = h.commands.register(&h.ctx, definition(name)).err().unwrap();
        assert_eq!(
            failure.to_string(),
            format!("command name \"{name}\" must match /^[a-z][a-z0-9_-]*$/u")
        );
    }
}

#[tokio::test]
async fn rejects_empty_descriptions() {
    let h = harness();
    let mut command = definition("empty");
    command.description = " \t\n".into();
    assert_eq!(
        h.commands
            .register(&h.ctx, command)
            .err()
            .unwrap()
            .to_string(),
        "command \"empty\" description must not be empty"
    );
}

#[tokio::test]
async fn rejects_empty_input_hints() {
    let h = harness();
    let mut command = definition("input");
    command.input = Some(CommandInputDescriptor::new(" \n"));
    assert_eq!(
        h.commands
            .register(&h.ctx, command)
            .err()
            .unwrap()
            .to_string(),
        "command \"input\" input hint must not be empty"
    );
}

#[tokio::test]
async fn passes_the_exact_invocation_context_to_the_handler() {
    let h = harness();
    let seen = Arc::new(Mutex::new(None::<CommandInvocation>));
    let capture = seen.clone();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("inspect", "inspect", move |invocation| {
                *lock(&capture) = Some(invocation);
                async { Ok(CommandResult::success(Some("done"))) }
            }),
        )
        .unwrap();
    let execution = h
        .commands
        .execute(h.agent.clone(), "/inspect  raw", vec![], signal())
        .await
        .unwrap()
        .unwrap();
    let invocation = lock(&seen).clone().unwrap();
    assert_eq!(invocation.command_id, execution.command_id);
    assert!(Arc::ptr_eq(&invocation.agent, &h.agent));
    assert_eq!(invocation.raw_input, "  raw");
    assert!(invocation.attachments.is_empty());
}

#[tokio::test]
async fn rejects_pre_aborted_execution_without_logging_or_invoking() {
    let h = harness();
    let invoked = Arc::new(AtomicBool::new(false));
    let observed = invoked.clone();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("stop", "stop", move |_| {
                observed.store(true, Ordering::SeqCst);
                async { Ok(CommandResult::success(Some("bad"))) }
            }),
        )
        .unwrap();
    let controller = AbortController::new();
    controller.abort_with("already stopped");
    let failure = h
        .commands
        .execute(h.agent.clone(), "/stop", vec![], controller.signal())
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "already stopped");
    assert!(!invoked.load(Ordering::SeqCst));
    assert!(event_data(&h.agent).is_empty());
}

#[tokio::test]
async fn propagates_handler_errors_and_settles_the_lifecycle() {
    let h = harness();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("fail", "fail", |_| async {
                Err::<CommandResult, BoxError>(io::Error::other("handler failed").into())
            }),
        )
        .unwrap();
    let failure = h
        .commands
        .execute(h.agent.clone(), "/fail", vec![], signal())
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "handler failed");
    let events = event_data(&h.agent);
    assert_eq!(
        events
            .iter()
            .map(|event| event.0.as_str())
            .collect::<Vec<_>>(),
        ["command/run", "command/done"]
    );
    assert_eq!(events[1].1["kind"], "error");
    assert_eq!(events[1].1["text"], "handler failed");
}

#[tokio::test]
async fn abort_while_handler_pending_settles_pair() {
    let h = harness();
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("wait", "wait", move |_| {
                notify.notify_one();
                std::future::pending::<Result<CommandResult, BoxError>>()
            }),
        )
        .unwrap();
    let controller = Arc::new(AbortController::new());
    let pending =
        tokio::spawn(
            h.commands
                .execute(h.agent.clone(), "/wait", vec![], controller.signal()),
        );
    entered.notified().await;
    controller.abort_with("operator cancelled command");
    let failure = pending.await.unwrap().unwrap_err();
    assert_eq!(failure.to_string(), "operator cancelled command");
    assert_error_pair(&h.agent, "operator cancelled command");
}

#[tokio::test]
async fn handler_panic_is_rethrown_as_error_and_settles_pair() {
    let h = harness();
    let mut command = definition("panic-handler");
    command.handler = Arc::new(|_| panic!("handler panicked"));
    h.commands.register(&h.ctx, command).unwrap();
    let failure = h
        .commands
        .execute(h.agent.clone(), "/panic-handler", vec![], signal())
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "handler panicked");
    assert_error_pair(&h.agent, "handler panicked");
}

#[tokio::test]
async fn handler_future_panic_is_rethrown_as_error_and_settles_pair() {
    let h = harness();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("panic-future", "panic future", |_| async {
                panic!("handler future panicked")
            }),
        )
        .unwrap();
    let failure = h
        .commands
        .execute(h.agent.clone(), "/panic-future", vec![], signal())
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "handler future panicked");
    assert_error_pair(&h.agent, "handler future panicked");
}

#[tokio::test]
async fn failed_done_append_warning_preserves_error_prefix() {
    let h = harness();
    let store = SessionStore::new();
    let session = store
        .create(
            &h.ctx,
            Some("append-failure".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    let attached = agent_with_session(&h.ctx, ScopeKey::new(), session);
    let event_count = Arc::new(AtomicUsize::new(0));
    let observed = event_count.clone();
    h.ctx
        .on_dispatch(move |_, name, event| {
            if name == "session/event" {
                let emitted = event.downcast_ref::<SessionEventEmitted>().unwrap();
                if observed.fetch_add(1, Ordering::SeqCst) == 1 {
                    emitted
                        .validation
                        .reject(io::Error::other("done rejected").into());
                }
            }
        })
        .unwrap();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("warn", "warn", |_| async {
                Err::<CommandResult, BoxError>(io::Error::other("handler failed").into())
            }),
        )
        .unwrap();
    let failure = h
        .commands
        .execute(attached, "/warn", vec![], signal())
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "handler failed");
    assert_eq!(event_count.load(Ordering::SeqCst), 2);
    let messages = h
        .ctx
        .logger()
        .buffer()
        .into_iter()
        .map(|record| record.message)
        .collect::<Vec<_>>();
    assert!(
        messages.iter().any(|message| {
            message == "command \"warn\": command/done append failed: Error: done rejected"
        }),
        "{messages:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_listener_can_start_another_command_without_sequence_deadlock() {
    let h = harness();
    let store = SessionStore::new();
    let first = agent_with_session(
        &h.ctx,
        ScopeKey::new(),
        store
            .create(
                &h.ctx,
                Some("first-agent".into()),
                CreateSessionOptions::default(),
            )
            .unwrap(),
    );
    let second = agent_with_session(
        &h.ctx,
        ScopeKey::new(),
        store
            .create(
                &h.ctx,
                Some("second-agent".into()),
                CreateSessionOptions::default(),
            )
            .unwrap(),
    );
    h.commands.register(&h.ctx, definition("nested")).unwrap();
    let nested = Arc::new(Mutex::new(
        None::<BoxFuture<'static, Result<Option<hoh_commands::CommandExecution>, BoxError>>>,
    ));
    let nested_out = nested.clone();
    let commands = h.commands.clone();
    let second_agent = second.clone();
    h.ctx
        .on_contained::<SessionEventEmitted>(move |emitted| {
            if emitted.session.id().as_str() == "first-agent"
                && emitted.event.event_type() == "command/run"
            {
                *lock(&nested_out) =
                    Some(commands.execute(second_agent.clone(), "/nested", vec![], signal()));
            }
            Ok(())
        })
        .unwrap();
    let first_execution = h.commands.execute(first, "/nested", vec![], signal());
    let second_execution = lock(&nested).take().expect("listener started command");
    let first_execution = first_execution.await.unwrap().unwrap();
    let second_execution = second_execution.await.unwrap().unwrap();
    assert!(first_execution.command_id.as_str().ends_with("-1"));
    assert!(second_execution.command_id.as_str().ends_with("-2"));
}

#[tokio::test]
async fn honors_a_signal_aborted_synchronously_by_the_handler() {
    let h = harness();
    let controller = Arc::new(AbortController::new());
    let abort = controller.clone();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("self-stop", "self stop", move |_| {
                abort.abort_with("self stopped");
                async { Ok(CommandResult::success(Some("ignored"))) }
            }),
        )
        .unwrap();
    let failure = h
        .commands
        .execute(h.agent.clone(), "/self-stop", vec![], controller.signal())
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "self stopped");
    assert_eq!(event_data(&h.agent)[1].1["text"], "self stopped");
}

#[tokio::test]
async fn returns_expected_error_results_without_throwing() {
    let h = harness();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("decline", "decline", |_| async {
                Ok(CommandResult::error("not now"))
            }),
        )
        .unwrap();
    let execution = h
        .commands
        .execute(h.agent.clone(), "/decline", vec![], signal())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(execution.result, CommandResult::error("not now"));
}

#[tokio::test]
async fn appends_success_lifecycle_events_in_order() {
    let h = harness();
    h.commands.register(&h.ctx, definition("go")).unwrap();
    let execution = h
        .commands
        .execute(h.agent.clone(), "/go x", vec![], signal())
        .await
        .unwrap()
        .unwrap();
    let events = event_data(&h.agent);
    assert_eq!(
        events,
        vec![
            (
                "command/run".into(),
                json!({"commandId": execution.command_id, "name": "go", "args": " x", "source": {"kind": "user"}})
            ),
            (
                "command/done".into(),
                json!({"commandId": execution.command_id, "kind": "success", "text": "ok"})
            ),
        ]
    );
}

#[tokio::test]
async fn preserves_success_source_event_sequence() {
    let h = harness();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("source", "source", |_| async {
                Ok(CommandResult::success_from(
                    SessionSeq::new(7).unwrap(),
                    Some("from event"),
                ))
            }),
        )
        .unwrap();
    let result = h
        .commands
        .execute(h.agent.clone(), "/source", vec![], signal())
        .await
        .unwrap()
        .unwrap()
        .result;
    assert_eq!(result.source_event_seq(), Some(SessionSeq::new(7).unwrap()));
    assert_eq!(event_data(&h.agent)[1].1["sourceEventSeq"], 7);
}

#[tokio::test]
async fn omits_recorded_arguments_when_record_input_is_false() {
    let h = harness();
    let mut command = definition("secret");
    command.record_input = Some(false);
    h.commands.register(&h.ctx, command).unwrap();
    h.commands
        .execute(h.agent.clone(), "/secret private", vec![], signal())
        .await
        .unwrap();
    assert!(event_data(&h.agent)[0].1.get("args").is_none());
}

#[tokio::test]
async fn mints_unique_monotonic_command_ids() {
    let h = harness();
    h.commands.register(&h.ctx, definition("id")).unwrap();
    let first = h
        .commands
        .execute(h.agent.clone(), "/id", vec![], signal())
        .await
        .unwrap()
        .unwrap()
        .command_id;
    let second = h
        .commands
        .execute(h.agent.clone(), "/id", vec![], signal())
        .await
        .unwrap()
        .unwrap()
        .command_id;
    assert!(first.as_str().starts_with("cmd-"));
    assert!(first.as_str().ends_with("-1"));
    assert!(second.as_str().ends_with("-2"));
    assert_ne!(first, second);
}

#[tokio::test]
async fn syntax_and_resolution_misses_append_nothing() {
    let h = harness();
    h.commands.register(&h.ctx, definition("known")).unwrap();
    assert!(
        h.commands
            .execute(h.agent.clone(), "known", vec![], signal())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        h.commands
            .execute(h.agent.clone(), "/unknown", vec![], signal())
            .await
            .unwrap()
            .is_none()
    );
    assert!(event_data(&h.agent).is_empty());
}

#[tokio::test]
async fn command_events_do_not_open_a_turn() {
    let h = harness();
    h.commands
        .register(&h.ctx, definition("standalone"))
        .unwrap();
    h.commands
        .execute(h.agent.clone(), "/standalone", vec![], signal())
        .await
        .unwrap();
    assert_eq!(
        event_data(&h.agent)
            .iter()
            .map(|event| event.0.as_str())
            .collect::<Vec<_>>(),
        ["command/run", "command/done"]
    );
}

#[tokio::test]
async fn command_events_are_appended_inside_an_open_turn() {
    let h = harness();
    h.agent
        .session()
        .append("turn/start", json!({"turn": 1}), None)
        .unwrap();
    h.commands.register(&h.ctx, definition("inside")).unwrap();
    h.commands
        .execute(h.agent.clone(), "/inside", vec![], signal())
        .await
        .unwrap();
    assert_eq!(
        event_data(&h.agent)
            .iter()
            .map(|event| event.0.as_str())
            .collect::<Vec<_>>(),
        ["turn/start", "command/run", "command/done"]
    );
}

#[tokio::test]
async fn rejects_empty_handler_error_text_and_closes_the_pair() {
    let h = harness();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("invalid", "invalid", |_| async {
                Ok(CommandResult::error(" \n"))
            }),
        )
        .unwrap();
    let failure = h
        .commands
        .execute(h.agent.clone(), "/invalid", vec![], signal())
        .await
        .unwrap_err();
    assert_eq!(
        failure.to_string(),
        "command \"invalid\" error text must be a non-empty string"
    );
    let events = event_data(&h.agent);
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].1["kind"], "error");
    assert_eq!(events[1].1["text"], failure.to_string());
}

#[test]
fn attachment_descriptor_requires_an_explicit_true_flag() {
    let plain = CommandInputDescriptor::new("input");
    let enabled = plain.clone().with_attachments();
    assert!(!plain.accepts_attachments());
    assert!(enabled.accepts_attachments());
}

fn image(data: &str, name: &str) -> CommandSubmitAttachment {
    CommandSubmitAttachment::Image {
        media_type: ImageMediaType::Png,
        data: data.into(),
        name: Some(name.into()),
    }
}

fn assert_error_pair(agent: &Arc<dyn hoh_agent::Agent>, text: &str) {
    let events = event_data(agent);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "command/run");
    assert_eq!(events[1].0, "command/done");
    assert_eq!(events[1].1["kind"], "error");
    assert_eq!(events[1].1["text"], text);
    assert_eq!(events[0].1["commandId"], events[1].1["commandId"]);
}

#[derive(Debug)]
struct TestStore {
    limits: ImageAttachmentLimits,
    saved: Mutex<Vec<String>>,
    fail_validation: AtomicBool,
    gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    entered: tokio::sync::Notify,
}

impl TestStore {
    fn new(max_images: u64) -> Arc<Self> {
        Arc::new(Self {
            limits: ImageAttachmentLimits {
                max_image_bytes: 1024,
                max_images_per_message: max_images,
                max_message_image_bytes: 4096,
                max_image_pixels: 1024,
                max_image_dimension: 1024,
                media_types: vec![ImageMediaType::Png],
            },
            saved: Mutex::new(vec![]),
            fail_validation: AtomicBool::new(false),
            gate: Mutex::new(None),
            entered: tokio::sync::Notify::new(),
        })
    }
}

impl AttachmentStore for TestStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }

    fn validate_image<'a>(
        &'a self,
        _: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            if self.fail_validation.load(Ordering::SeqCst) {
                return Err(io::Error::other("validation backend failed").into());
            }
            Ok(())
        })
    }

    fn save_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async move {
            self.entered.notify_one();
            let gate = lock(&self.gate).take();
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            let name = input.name.clone().unwrap_or_default();
            lock(&self.saved).push(name.clone());
            Ok(ImageAttachmentRef {
                attachment_id: AttachmentId::from(format!("sha256:{name}")),
                media_type: input.media_type,
                bytes: input.data.len() as u64,
                width: 1,
                height: 1,
                name: input.name.clone(),
                original_dimensions: None,
            })
        })
    }

    fn read_image<'a>(
        &'a self,
        _: &'a ImageAttachmentRef,
        _: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async { Err(io::Error::other("unused").into()) })
    }
}

fn attachment_command(
    name: &str,
    invoked: Arc<AtomicBool>,
    captured: Arc<Mutex<Vec<String>>>,
) -> CommandDefinition {
    let mut command = CommandDefinition::new(name, "attachments", move |invocation| {
        invoked.store(true, Ordering::SeqCst);
        *lock(&captured) = invocation
            .attachments
            .iter()
            .map(|block| block.r#type.clone())
            .collect();
        async { Ok(CommandResult::success(Some("accepted"))) }
    });
    command.input = Some(CommandInputDescriptor::new("files").with_attachments());
    command
}

#[tokio::test]
async fn rejects_attachments_for_commands_that_do_not_declare_them() {
    let h = harness();
    let invoked = Arc::new(AtomicBool::new(false));
    let observed = invoked.clone();
    h.commands
        .register(
            &h.ctx,
            CommandDefinition::new("plain", "plain", move |_| {
                observed.store(true, Ordering::SeqCst);
                async { Ok(CommandResult::success(Some("bad"))) }
            }),
        )
        .unwrap();
    let result = h
        .commands
        .execute(
            h.agent.clone(),
            "/plain",
            vec![image("AQ==", "one.png")],
            signal(),
        )
        .await
        .unwrap()
        .unwrap()
        .result;
    assert_eq!(
        result,
        CommandResult::error("/plain does not accept attachments")
    );
    assert!(!invoked.load(Ordering::SeqCst));
    assert_error_pair(&h.agent, "/plain does not accept attachments");
}

#[tokio::test]
async fn rejects_declared_attachments_when_no_store_is_composed() {
    let h = harness();
    let invoked = Arc::new(AtomicBool::new(false));
    h.commands
        .register(
            &h.ctx,
            attachment_command("upload", invoked.clone(), Arc::new(Mutex::new(vec![]))),
        )
        .unwrap();
    let result = h
        .commands
        .execute(
            h.agent.clone(),
            "/upload",
            vec![image("AQ==", "one.png")],
            signal(),
        )
        .await
        .unwrap()
        .unwrap()
        .result;
    assert_eq!(
        result,
        CommandResult::error(
            "/upload: attachments are unavailable because no attachment store is composed"
        )
    );
    assert!(!invoked.load(Ordering::SeqCst));
    assert_error_pair(
        &h.agent,
        "/upload: attachments are unavailable because no attachment store is composed",
    );
}

#[tokio::test]
async fn unknown_file_receipt_settles_error_before_any_image_save() {
    for attachments in [
        vec![CommandSubmitAttachment::File {
            receipt_id: "missing".into(),
        }],
        vec![
            image("AQ==", "one.png"),
            CommandSubmitAttachment::File {
                receipt_id: "missing".into(),
            },
        ],
    ] {
        let h = harness();
        let store = TestStore::new(2);
        h.ctx.provide::<Attachments>(store.clone()).await.unwrap();
        let invoked = Arc::new(AtomicBool::new(false));
        h.commands
            .register(
                &h.ctx,
                attachment_command("receipt", invoked.clone(), Arc::new(Mutex::new(vec![]))),
            )
            .unwrap();
        let result = h
            .commands
            .execute(h.agent.clone(), "/receipt", attachments, signal())
            .await
            .unwrap()
            .unwrap()
            .result;
        assert_eq!(
            result,
            CommandResult::error("File upload receipt is unknown for this session.")
        );
        assert!(!invoked.load(Ordering::SeqCst));
        assert!(lock(&store.saved).is_empty());
        assert_error_pair(&h.agent, "File upload receipt is unknown for this session.");
    }
}

#[tokio::test]
async fn resolves_files_before_saving_images_and_preserves_mixed_order() {
    let h = harness();
    let store = TestStore::new(4);
    h.ctx.provide::<Attachments>(store.clone()).await.unwrap();
    let resolution_order = Arc::new(Mutex::new(Vec::new()));
    let order = resolution_order.clone();
    h.commands
        .register_file_receipt_resolver(Arc::new(move |_, receipt| {
            lock(&order).push(receipt.to_owned());
            Some(FileAttachmentRef {
                attachment_id: AttachmentId::from(format!("sha256:{receipt}")),
                name: format!("{receipt}.txt"),
                bytes: 1,
            })
        }))
        .unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(Mutex::new(vec![]));
    h.commands
        .register(
            &h.ctx,
            attachment_command("mixed", invoked.clone(), captured.clone()),
        )
        .unwrap();
    let result = h
        .commands
        .execute(
            h.agent.clone(),
            "/mixed",
            vec![
                image("AQ==", "one.png"),
                CommandSubmitAttachment::File {
                    receipt_id: "file-a".into(),
                },
                image("Ag==", "two.png"),
                CommandSubmitAttachment::File {
                    receipt_id: "file-a".into(),
                },
            ],
            signal(),
        )
        .await
        .unwrap()
        .unwrap()
        .result;
    assert_eq!(result, CommandResult::success(Some("accepted")));
    assert!(invoked.load(Ordering::SeqCst));
    assert_eq!(*lock(&resolution_order), ["file-a"]);
    assert_eq!(*lock(&store.saved), ["one.png", "two.png"]);
    assert_eq!(*lock(&captured), ["image", "file", "image", "file"]);
}

#[tokio::test]
async fn file_receipt_resolver_is_single_owner_and_undo_is_idempotent() {
    let h = harness();
    let resolver = || {
        Arc::new(|_: &Arc<dyn hoh_agent::Agent>, _: &str| None)
            as Arc<hoh_commands::CommandFileReceiptResolver>
    };
    let undo = h
        .commands
        .register_file_receipt_resolver(resolver())
        .unwrap();
    assert_eq!(
        h.commands
            .register_file_receipt_resolver(resolver())
            .err()
            .unwrap()
            .to_string(),
        "commands: a file receipt resolver is already registered"
    );
    undo.undo();
    let current = h
        .commands
        .register_file_receipt_resolver(resolver())
        .unwrap();
    undo.undo();
    assert_eq!(
        h.commands
            .register_file_receipt_resolver(resolver())
            .err()
            .unwrap()
            .to_string(),
        "commands: a file receipt resolver is already registered"
    );
    current.undo();
}

#[tokio::test]
async fn resolver_panic_is_rethrown_as_error_and_settles_pair() {
    let h = harness();
    h.ctx
        .provide::<Attachments>(TestStore::new(1))
        .await
        .unwrap();
    h.commands
        .register_file_receipt_resolver(Arc::new(|_, _| -> Option<FileAttachmentRef> {
            panic!("resolver panicked")
        }))
        .unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    h.commands
        .register(
            &h.ctx,
            attachment_command(
                "panic-resolver",
                invoked.clone(),
                Arc::new(Mutex::new(vec![])),
            ),
        )
        .unwrap();
    let failure = h
        .commands
        .execute(
            h.agent.clone(),
            "/panic-resolver",
            vec![CommandSubmitAttachment::File {
                receipt_id: "one".into(),
            }],
            signal(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "resolver panicked");
    assert!(!invoked.load(Ordering::SeqCst));
    assert_error_pair(&h.agent, "resolver panicked");
}

#[tokio::test]
async fn attachment_policy_failures_settle_as_expected_errors() {
    let h = harness();
    let store = TestStore::new(1);
    h.ctx.provide::<Attachments>(store).await.unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    h.commands
        .register(
            &h.ctx,
            attachment_command("limited", invoked.clone(), Arc::new(Mutex::new(vec![]))),
        )
        .unwrap();
    let result = h
        .commands
        .execute(
            h.agent.clone(),
            "/limited",
            vec![image("AQ==", "one.png"), image("Ag==", "two.png")],
            signal(),
        )
        .await
        .unwrap()
        .unwrap()
        .result;
    assert_eq!(result.kind(), "error");
    assert_eq!(
        result.text(),
        Some("Image batch exceeds the configured image-count limit.")
    );
    assert!(!invoked.load(Ordering::SeqCst));
    assert_error_pair(
        &h.agent,
        "Image batch exceeds the configured image-count limit.",
    );
}

#[tokio::test]
async fn cancellation_during_admission_prevents_handler_entry() {
    let h = harness();
    let store = TestStore::new(2);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&store.gate) = Some(gate.clone());
    h.ctx.provide::<Attachments>(store.clone()).await.unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    h.commands
        .register(
            &h.ctx,
            attachment_command("slow", invoked.clone(), Arc::new(Mutex::new(vec![]))),
        )
        .unwrap();
    let controller = Arc::new(AbortController::new());
    let future = h.commands.execute(
        h.agent.clone(),
        "/slow",
        vec![image("AQ==", "one.png")],
        controller.signal(),
    );
    let pending = tokio::spawn(future);
    store.entered.notified().await;
    controller.abort_with("cancelled during upload");
    gate.add_permits(1);
    let failure = pending.await.unwrap().unwrap_err();
    assert_eq!(failure.to_string(), "cancelled during upload");
    assert!(!invoked.load(Ordering::SeqCst));
    assert_eq!(event_data(&h.agent)[1].1["kind"], "error");
}

#[tokio::test]
async fn non_attachment_admission_failures_are_rethrown_and_logged() {
    let h = harness();
    let store = TestStore::new(2);
    store.fail_validation.store(true, Ordering::SeqCst);
    h.ctx.provide::<Attachments>(store).await.unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    h.commands
        .register(
            &h.ctx,
            attachment_command("broken", invoked.clone(), Arc::new(Mutex::new(vec![]))),
        )
        .unwrap();
    let failure = h
        .commands
        .execute(
            h.agent.clone(),
            "/broken",
            vec![image("AQ==", "one.png")],
            signal(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.to_string(), "validation backend failed");
    assert!(!invoked.load(Ordering::SeqCst));
    assert_eq!(
        event_data(&h.agent)[1].1["text"],
        "validation backend failed"
    );
}
