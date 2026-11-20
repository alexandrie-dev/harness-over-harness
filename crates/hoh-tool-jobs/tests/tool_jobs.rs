//! Port of `tests/tool-jobs.spec.ts`.

mod common;

use std::sync::Arc;

use common::*;
use hoh_jobs::{
    JobChannel, JobChunk, JobOutputSource, JobOutputView, JobSourceRead, JobStatus, JobView,
};
use hoh_jobs_local::LocalJobRegistryPlugin;
use hoh_kernel::Kernel;
use hoh_llm::ContentBlock;
use hoh_session::SessionId;
use hoh_system_prompt::SystemPrompt;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_jobs::{
    CompletionDelivery, Config, ToolJobs,
    render::{public_job, render_model_delta, status_line},
};
use hoh_tools::{
    PostToolDecision, PreToolDecision, ToolCallKind, ToolCallView, ToolErrorInfo,
    ToolExecutionResult, ToolExecutionSuccess, ToolRuntime, Tools, ToolsExecute, ToolsPostExecute,
    ToolsPreExecute,
};
use serde_json::{Value, json};

fn chunk(at: u64, text: &str, channel: Option<JobChannel>, gap_before: bool) -> JobChunk {
    JobChunk {
        at,
        text: text.to_owned(),
        channel,
        gap_before,
    }
}

fn error_info(result: &ToolExecutionResult) -> Option<(String, String)> {
    let failure = result.error()?;
    let info: &ToolErrorInfo = failure.info.as_ref()?;
    Some((info.name.clone(), info.code.clone()))
}

fn spawn_call(
    env: &Env,
    name: &'static str,
    args: Value,
    agent: Option<&Owner>,
) -> tokio::task::JoinHandle<ToolExecutionResult> {
    let ctx = env.ctx.clone();
    let agent = agent.cloned();
    tokio::spawn(async move { call(&ctx, name, args, agent.as_ref()).await })
}

fn expect_value(result: &ToolExecutionResult) -> &Value {
    result.value().expect("tool success")
}

fn job_args(env: &Env, legacy: &str) -> Value {
    json!({ "job_id": env.id(legacy).to_string() })
}

// ------------------------------------------------------------------------ tool-jobs setup

#[tokio::test]
async fn attaches_the_job_controller_on_load_and_detaches_it_with_the_fiber() {
    let env = setup_default().await;
    env.jobs().start(producer().spec()).unwrap();
    env.tools_fiber.clone().unwrap().dispose().await;
    let error = env.jobs().start(producer().spec()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no job controller serves this agent")
    );
}

#[tokio::test]
async fn serves_unowned_jobs_in_a_composition_without_an_agent_registry() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount::<SystemPrompt>(&ctx, json!({})).await;
    mount::<ToolRuntime>(&ctx, json!({})).await;
    mount::<NativeToolExecutorPlugin>(&ctx, json!({})).await;
    mount::<LocalJobRegistryPlugin>(&ctx, json!({})).await;
    mount::<ToolJobs>(&ctx, json!({})).await;
    let mut p = producer();
    let id = ctx
        .get::<hoh_jobs::Jobs>()
        .unwrap()
        .start(p.spec())
        .unwrap();
    p.append("open\n");
    let result = call(
        &ctx,
        "job_output",
        json!({ "job_id": id.to_string() }),
        None,
    )
    .await;
    assert_eq!(text(&result), "open\n[status: running]");
    p.settle(completed());
    tick().await;
    let listed = call(&ctx, "job_list", json!({}), None).await;
    assert_eq!(text(&listed), format!("{id} [bash] completed — sleep 60"));
}

#[tokio::test]
async fn rejects_a_config_whose_default_wait_exceeds_the_cap() {
    let (_kernel, ctx, _) = base(json!({})).await;
    let fiber = ctx
        .plugin::<ToolJobs>(json!({ "waitTimeoutMs": 100, "maxWaitTimeoutMs": 50 }))
        .unwrap();
    let error = fiber.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("waitTimeoutMs (100) exceeds maxWaitTimeoutMs (50)"),
        "{error}"
    );
}

#[tokio::test]
async fn defaults_delivery_to_unbounded_wakeup_and_rejects_an_unknown_lane() {
    let config: Config = serde_json::from_value(json!({})).unwrap();
    assert_eq!(config.completion_delivery, CompletionDelivery::Wakeup);
    assert_eq!(config.max_consecutive_wakes, None);
    assert!(serde_json::from_value::<Config>(json!({ "completionDelivery": "loud" })).is_err());
    // `maxConsecutiveWakes: 0` is refused by the schema's `minimum: 1` when the plugin loads.
    let (_kernel, ctx, _) = base(json!({})).await;
    let rejected = match ctx.plugin::<ToolJobs>(json!({ "maxConsecutiveWakes": 0 })) {
        Err(_) => true,
        Ok(fiber) => fiber.settled().await.is_err(),
    };
    assert!(rejected);
}

/// Reports the load outcome as text.
async fn load_with(max_consecutive_wakes: f64) -> String {
    let (_kernel, ctx, _) = base(json!({})).await;
    match ctx.plugin::<ToolJobs>(json!({ "maxConsecutiveWakes": max_consecutive_wakes })) {
        Err(error) => error.to_string(),
        Ok(fiber) => match fiber.settled().await {
            Ok(()) => "loaded".to_owned(),
            Err(error) => error.to_string(),
        },
    }
}

#[tokio::test]
async fn rejects_a_wake_budget_that_cannot_bound_anything() {
    // The field exists to bound runaway waking; a fractional budget counts nothing, and an
    // infinite one is spelled by omitting the field (JSON cannot spell `Infinity`).
    assert!(load_with(2.5).await.contains("maxConsecutiveWakes"));
    assert_eq!(load_with(1.0).await, "loaded");
}

#[test]
fn renders_status_lines_with_and_without_producer_detail() {
    assert_eq!(status_line("running", None), "[status: running]");
    assert_eq!(
        status_line("completed", Some("exit code: 0")),
        "[status: completed, exit code: 0]"
    );
}

fn view(owner: Option<&str>, status: JobStatus) -> JobView {
    JobView {
        id: hoh_jobs::JobId::from("bash-1"),
        kind: "bash".to_owned(),
        label: "x".to_owned(),
        owner: owner.map(SessionId::from),
        output_limit_bytes: owner.map(|_| 8),
        status,
        progress: None,
        detail: None,
        started_at: 5,
        finished_at: None,
        output: JobOutputView {
            total: 12,
            earliest: 0,
            spill_paths: None,
        },
    }
}

#[test]
fn projects_the_public_job_live_progress_or_terminal_detail_never_ownership_or_offsets() {
    let mut live = view(Some("sess-1"), JobStatus::Running);
    live.progress = Some("3/10 files".to_owned());
    assert_eq!(
        Value::Object(public_job(&live).to_map()),
        json!({ "id": "bash-1", "kind": "bash", "label": "x", "status": "running", "detail": "3/10 files", "startedAt": 5 })
    );
    let mut settled = view(None, JobStatus::Completed);
    settled.detail = Some("exit code: 0".to_owned());
    settled.finished_at = Some(9);
    assert_eq!(
        Value::Object(public_job(&settled).to_map()),
        json!({ "id": "bash-1", "kind": "bash", "label": "x", "status": "completed", "detail": "exit code: 0", "startedAt": 5, "finishedAt": 9 })
    );
}

#[test]
fn renders_a_model_delta_as_stdout_one_stderr_section_and_a_dropped_output_notice() {
    assert_eq!(render_model_delta(&[], false, &[]), "");
    assert_eq!(
        render_model_delta(&[], true, &[]),
        "[some output was dropped from memory; full output: (unavailable)]"
    );
    assert_eq!(
        render_model_delta(
            &[
                chunk(0, "a", None, false),
                chunk(1, "e1\n", Some(JobChannel::Stderr), false),
                chunk(4, "narration", Some(JobChannel::Log), false),
                chunk(13, "b", Some(JobChannel::Stdout), false),
                chunk(14, "e2", Some(JobChannel::Stderr), false),
            ],
            false,
            &[]
        ),
        "ab\n[stderr]\ne1\ne2"
    );
    assert_eq!(
        render_model_delta(&[chunk(0, "tail", None, false)], true, &[]),
        "tail\n[some output was dropped from memory; full output: (unavailable)]"
    );
    assert_eq!(
        render_model_delta(&[chunk(0, "line\n", None, false)], true, &[]),
        "line\n[some output was dropped from memory; full output: (unavailable)]"
    );
    // Ring eviction names the files the job's sources keep, whether or not a retained chunk
    // carries a gap.
    assert_eq!(
        render_model_delta(
            &[chunk(8, "tail", None, false)],
            true,
            &["/spill/out.log".to_owned(), "/spill/err.log".to_owned()]
        ),
        "tail\n[some output was dropped from memory; full output: /spill/out.log, /spill/err.log]"
    );
}

#[test]
fn reports_a_producer_side_gap_as_dropped_output_and_names_the_spill_files_the_job_advertises() {
    // The ring itself lost nothing (`lossy: false`); the source did, between two pumps.
    assert_eq!(
        render_model_delta(
            &[chunk(0, "head", None, false), chunk(4, "tail", None, true)],
            false,
            &["/spill/out.log".to_owned()]
        ),
        "headtail\n[some output was dropped from memory; full output: /spill/out.log]"
    );
    assert_eq!(
        render_model_delta(
            &[
                chunk(0, "o", None, true),
                chunk(1, "e", Some(JobChannel::Stderr), true)
            ],
            false,
            &["/spill/out.log".to_owned(), "/spill/err.log".to_owned()]
        ),
        "o\n[stderr]\ne\n[some output was dropped from memory; full output: /spill/out.log, /spill/err.log]"
    );
    // A gap while no source keeps a file yields the generic notice.
    assert_eq!(
        render_model_delta(&[chunk(0, "tail", None, true)], false, &[]),
        "tail\n[some output was dropped from memory; full output: (unavailable)]"
    );
    // A gap on observer-only narration never reaches the model.
    assert_eq!(
        render_model_delta(
            &[chunk(0, "phase", Some(JobChannel::Log), true)],
            false,
            &["/spill/log".to_owned()]
        ),
        ""
    );
}

#[tokio::test]
async fn names_the_spill_file_when_the_ring_evicted_output_the_model_never_read() {
    let env = setup(json!({}), json!({ "retainBytes": 8 })).await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    // The source never reads lossy: the pump keeps up, and only the ring's live cap drops bytes.
    let source = JobOutputSource {
        channel: Some(JobChannel::Stdout),
        read: Box::new(|from| {
            Ok(if from == 0 {
                JobSourceRead {
                    text: "x".repeat(32),
                    next_offset: 32,
                    lossy: false,
                    spill_path: Some("/spill/out.log".to_owned()),
                }
            } else {
                JobSourceRead {
                    text: String::new(),
                    next_offset: from,
                    lossy: false,
                    spill_path: Some("/spill/out.log".to_owned()),
                }
            })
        }),
    };
    let mut p = producer_with(ProducerOptions {
        label: Some("noisy".to_owned()),
        owner: Some(owner.id()),
        ..ProducerOptions::default()
    });
    let mut spec = p.spec();
    spec.output = Some(vec![source]);
    let id = env.jobs().start(spec).unwrap();

    let result = call(
        &env.ctx,
        "job_output",
        json!({ "job_id": id.to_string() }),
        Some(&owner),
    )
    .await;
    assert_eq!(
        text(&result),
        format!(
            "{}\n[some output was dropped from memory; full output: /spill/out.log]\n[status: running]",
            "x".repeat(8)
        )
    );
    p.settle(completed());
    tick().await;
}

#[tokio::test]
async fn applies_the_built_in_wait_bounds_when_the_plugin_loads_with_a_bare_config() {
    // The source calls `apply(ctx, {})` without the schema defaults; the Rust `Config` always
    // carries them, so the bare row `{}` is the equivalent.
    let env = setup_default().await;
    let tools = env.ctx.get::<Tools>().unwrap();
    assert!(tools.get("job_output", None, None).is_some());
    env.jobs().start(producer().spec()).unwrap();
}

// ----------------------------------------------------------------------------- job_output

fn bytes(text: &str) -> usize {
    text.len()
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[tokio::test]
async fn reads_a_consuming_delta_with_a_trailing_status_line() {
    let env = setup_default().await;
    let mut p = producer();
    start(&env, &mut p);
    p.append("line one\n");

    // A body already ending in a newline gets no doubled separator.
    let first = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert!(!first.is_error(), "expected job_output success");
    let value = expect_value(&first);
    assert_eq!(value["text"], "line one\n");
    let job = &value["job"];
    assert_eq!(job["id"], env.id("bash-1").to_string());
    assert_eq!(
        (
            job["kind"].clone(),
            job["label"].clone(),
            job["status"].clone()
        ),
        (json!("bash"), json!("sleep 60"), json!("running"))
    );
    assert!(job.get("owner").is_none());
    assert!(job.get("output").is_none());
    assert_eq!(text(&first), "line one\n[status: running]");
    let second = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(text(&second), "(no new output)\n[status: running]");
}

#[tokio::test]
async fn renders_stderr_in_a_trailing_section_and_never_shows_log_narration() {
    let env = setup_default().await;
    let mut p = producer();
    start(&env, &mut p);
    p.append("out\n");
    p.append_on("warn\n", JobChannel::Stderr);
    p.append_on("observer-only narration", JobChannel::Log);
    let first = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(text(&first), "out\n[stderr]\nwarn\n[status: running]");
    let second = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(text(&second), "(no new output)\n[status: running]");
}

#[tokio::test]
async fn shows_live_progress_beside_the_status_until_settlement_replaces_it_with_the_terminal_detail()
 {
    let env = setup_default().await;
    let mut p = producer();
    start(&env, &mut p);
    p.progress("3/10 files");
    let live = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(
        text(&live),
        "(no new output)\n[status: running, 3/10 files]"
    );
    p.settle(completed_with(Some("exit code: 0"), None));
    tick().await;
    let done = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(
        text(&done),
        "(no new output)\n[status: completed, exit code: 0]"
    );
}

#[tokio::test]
async fn flags_a_read_whose_cursor_fell_behind_the_ring() {
    let env = setup(json!({}), json!({ "retainBytes": 16 })).await;
    let mut p = producer();
    let id = start(&env, &mut p);
    p.append(&"x".repeat(40));
    let first = text(&call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await);
    assert!(first.contains("[some output was dropped from memory; full output: (unavailable)]"));
    assert!(first.contains("[status: running]"));
    assert!(!first.contains(&"x".repeat(17)));
    // The cursor now sits at the ring's end: the next read is clean.
    let second = call(
        &env.ctx,
        "job_output",
        json!({ "job_id": id.to_string() }),
        None,
    )
    .await;
    assert_eq!(text(&second), "(no new output)\n[status: running]");
}

#[tokio::test]
async fn returns_the_result_of_a_settled_job_exactly_once_after_the_pending_delta() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        kind: Some("test-task"),
        label: Some("research".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);
    let first = call(&env.ctx, "job_output", job_args(&env, "test-task-1"), None).await;
    assert_eq!(text(&first), "(no new output)\n[status: running]");

    p.append("partial");
    p.settle(completed_with(Some("completed"), Some("the answer")));
    tick().await;
    let second = call(&env.ctx, "job_output", job_args(&env, "test-task-1"), None).await;
    assert_eq!(
        text(&second),
        "partial\nthe answer\n[status: completed, completed]"
    );
    let third = call(&env.ctx, "job_output", job_args(&env, "test-task-1"), None).await;
    assert_eq!(
        text(&third),
        "(no new output)\n[status: completed, completed]"
    );
}

#[tokio::test]
async fn keeps_a_newline_terminated_delta_and_the_result_on_separate_lines() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        kind: Some("test-task"),
        label: Some("research".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);
    p.append("progress log\n");
    p.settle(completed_with(None, Some("the answer")));
    tick().await;
    let result = call(&env.ctx, "job_output", job_args(&env, "test-task-1"), None).await;
    assert_eq!(
        text(&result),
        "progress log\nthe answer\n[status: completed]"
    );
}

#[tokio::test]
async fn applies_a_producer_limit_to_the_complete_body_and_status_result() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        output_limit_bytes: Some(48),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);
    p.append(&"界".repeat(100));

    let output = text(&call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await);
    assert!(bytes(&output) <= 48, "{output:?}");
    assert!(output.contains("[status: running]"));
}

#[tokio::test]
async fn preserves_empty_and_newline_terminated_output_under_a_producer_limit() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        output_limit_bytes: Some(64),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    let empty = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(text(&empty), "(no new output)\n[status: running]");
    p.append("line\n");
    let line = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(text(&line), "line\n[status: running]");
}

/// A `tools/post-execute` policy that answers `job_output`/`job_kill` calls with `decide`.
fn post_policy(
    env: &Env,
    decide: impl Fn(&hoh_tools::ToolExecution) -> Option<PostToolDecision> + Send + Sync + 'static,
) {
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
            let decision = decide(&event.exec);
            async move {
                match decision {
                    Some(decision) => Ok(decision),
                    None => next.run().await,
                }
            }
        })
        .unwrap();
}

fn accept_with(text: String) -> PostToolDecision {
    PostToolDecision::Accept {
        content: Some(vec![ContentBlock::text(text)]),
        value: None,
        additional_contexts: None,
    }
}

fn block_with(feedback: Vec<ContentBlock>) -> PostToolDecision {
    PostToolDecision::Block {
        feedback,
        info: None,
        additional_contexts: None,
    }
}

#[tokio::test]
async fn bounds_post_policy_output_without_restoring_the_canonical_status_rendering() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        output_limit_bytes: Some(64),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);
    p.append("canonical output");
    post_policy(&env, |exec| {
        (exec.name == "job_output").then(|| accept_with("p".repeat(1_000)))
    });

    let result = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert!(bytes(&text(&result)) <= 64);
    assert!(text(&result).contains("[result truncated]"));
    assert!(!text(&result).contains("[status: running]"));
}

#[tokio::test]
async fn does_not_double_a_truncation_marker_a_policy_already_appended() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        output_limit_bytes: Some(64),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);
    post_policy(&env, |exec| {
        (exec.name == "job_output")
            .then(|| accept_with(format!("{}\n[result truncated]", "z".repeat(1_000))))
    });

    let result = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert!(bytes(&text(&result)) <= 64);
    assert_eq!(count(&text(&result), "[result truncated]"), 1);
}

#[tokio::test]
async fn refuses_a_job_owned_by_another_session_as_an_errored_result() {
    let env = setup_default().await;
    let alice = env.owner("sess-alice", FakeDelivery::default()).await;
    env.jobs().start(owned_by(&alice).spec()).unwrap();
    let bob = env.owner("sess-bob", FakeDelivery::default()).await;

    let result = call(&env.ctx, "job_output", job_args(&env, "bash-1"), Some(&bob)).await;
    assert!(result.is_error());
    assert!(text(&result).contains("belongs to another session"));
    assert!(
        call(&env.ctx, "job_output", job_args(&env, "bash-1"), None)
            .await
            .is_error()
    );
    assert!(
        !call(
            &env.ctx,
            "job_output",
            job_args(&env, "bash-1"),
            Some(&alice)
        )
        .await
        .is_error()
    );
}

fn job_id_of(args: &Value) -> Option<String> {
    args.get("job_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[tokio::test]
async fn bounds_pre_around_and_post_execute_policy_outcomes_and_failures() {
    let env = setup_default().await;
    for _ in 0..5 {
        env.jobs()
            .start(
                producer_with(ProducerOptions {
                    output_limit_bytes: Some(64),
                    ..ProducerOptions::default()
                })
                .spec(),
            )
            .unwrap();
    }
    let ids: Vec<String> = (1..=5)
        .map(|n| env.id(&format!("bash-{n}")).to_string())
        .collect();
    let pre_ids = ids.clone();
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
            let job_id = job_id_of(&event.exec.arguments);
            let ids = pre_ids.clone();
            async move {
                if job_id.as_deref() == Some(ids[0].as_str()) {
                    return Ok(PreToolDecision::Deny {
                        reason: "d".repeat(1_000),
                        info: None,
                    });
                }
                if job_id.as_deref() == Some(ids[2].as_str()) {
                    return Err(format!("pre failed: {}", "p".repeat(1_000)).into());
                }
                next.run().await
            }
        })
        .unwrap();
    let around_ids = ids.clone();
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
            let job_id = job_id_of(&event.exec.arguments);
            let ids = around_ids.clone();
            async move {
                if job_id.as_deref() == Some(ids[1].as_str()) {
                    return Ok(ToolExecutionResult::Success(ToolExecutionSuccess::new(
                        json!({
                            "text": "a".repeat(1_000),
                            "job": {
                                "id": ids[1], "kind": "bash", "label": "sleep 60",
                                "status": "running", "startedAt": 0
                            }
                        }),
                        vec![],
                    )));
                }
                if job_id.as_deref() == Some(ids[3].as_str()) {
                    return Err(format!("around failed: {}", "e".repeat(1_000)).into());
                }
                next.run().await
            }
        })
        .unwrap();
    let post_ids = ids.clone();
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
            let job_id = job_id_of(&event.exec.arguments);
            let ids = post_ids.clone();
            async move {
                if job_id.as_deref() == Some(ids[4].as_str()) {
                    return Err(format!("post failed: {}", "o".repeat(1_000)).into());
                }
                next.run().await
            }
        })
        .unwrap();

    let denied = call(&env.ctx, "job_output", json!({ "job_id": ids[0] }), None).await;
    assert!(denied.is_error());
    assert!(bytes(&text(&denied)) <= 64);
    assert!(text(&denied).contains("[result truncated]"));

    let short_circuited = call(&env.ctx, "job_output", json!({ "job_id": ids[1] }), None).await;
    assert!(!short_circuited.is_error());
    assert!(bytes(&text(&short_circuited)) <= 64);
    assert!(text(&short_circuited).contains("[output truncated]"));

    for id in &ids[2..5] {
        let failure = call(&env.ctx, "job_output", json!({ "job_id": id }), None).await;
        assert!(failure.is_error());
        assert!(bytes(&text(&failure)) <= 64);
        assert!(text(&failure).contains("[result truncated]"));
    }
}

#[tokio::test]
async fn wait_true_blocks_until_settlement_and_reports_the_terminal_state() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        kind: Some("test-task"),
        label: Some("research".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    let mut args = job_args(&env, "test-task-1");
    args["wait"] = json!(true);
    let pending = spawn_call(&env, "job_output", args, None);
    tick().await;
    p.settle(completed_with(None, Some("done deal")));
    assert_eq!(
        text(&pending.await.unwrap()),
        "done deal\n[status: completed]"
    );
}

#[tokio::test]
async fn wait_true_times_out_against_the_configured_cap_and_leaves_the_job_alive() {
    let env = setup(
        json!({ "waitTimeoutMs": 10, "maxWaitTimeoutMs": 20 }),
        json!({}),
    )
    .await;
    env.jobs().start(producer().spec()).unwrap();

    // A model-supplied timeout far above the cap is clamped: this returns promptly (≤ the 20ms
    // cap), not after ten minutes.
    let mut args = job_args(&env, "bash-1");
    args["wait"] = json!(true);
    args["timeout_ms"] = json!(600_000);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        call(&env.ctx, "job_output", args, None),
    )
    .await
    .expect("the wait is clamped to the cap");
    assert_eq!(text(&result), "(no new output)\n[status: running]");
}

#[tokio::test]
async fn rejects_an_empty_or_unknown_job_id_as_an_errored_result() {
    let env = setup_default().await;
    let empty = call(&env.ctx, "job_output", json!({ "job_id": "" }), None).await;
    assert!(empty.is_error());
    assert_eq!(
        error_info(&empty),
        Some(("ToolArgsError".to_owned(), "INVALID_ARGS".to_owned()))
    );
    let unknown = call(&env.ctx, "job_output", json!({ "job_id": "bash-99" }), None).await;
    assert!(unknown.is_error());
    assert_eq!(
        error_info(&unknown),
        Some(("JobError".to_owned(), "JOB_NOT_FOUND".to_owned()))
    );
    assert!(text(&unknown).contains("unknown job bash-99"));
    let bad_timeout = call(
        &env.ctx,
        "job_output",
        json!({ "job_id": "bash-99", "wait": true, "timeout_ms": 0 }),
        None,
    )
    .await;
    assert!(bad_timeout.is_error());
    assert_eq!(
        error_info(&bad_timeout),
        Some(("ToolArgsError".to_owned(), "INVALID_ARGS".to_owned()))
    );
}

// -------------------------------------------------------------------------------- job_list

#[tokio::test]
async fn job_list_is_the_only_parallel_safe_job_tool() {
    let env = setup_default().await;
    let tools = env.ctx.get::<Tools>().unwrap();
    let list = tools.get("job_list", None, None).unwrap();
    assert_eq!(
        list.is_concurrency_safe.as_ref().map(|f| f(&json!({}))),
        Some(true)
    );
    for name in ["job_output", "job_kill"] {
        let definition = tools.get(name, None, None).expect("registered");
        assert!(definition.is_concurrency_safe.is_none());
    }
}

#[tokio::test]
async fn lists_caller_visible_jobs_and_renders_the_empty_case() {
    let env = setup_default().await;
    assert_eq!(
        text(&call(&env.ctx, "job_list", json!({}), None).await),
        "(no background jobs)"
    );

    let alice = env.owner("sess-alice", FakeDelivery::default()).await;
    let mut first = producer_with(ProducerOptions {
        owner: Some(alice.id()),
        label: Some("pnpm test".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut first);
    let mut open = producer_with(ProducerOptions {
        kind: Some("test-task"),
        label: Some("open research".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut open);
    let mut built = producer_with(ProducerOptions {
        owner: Some(alice.id()),
        label: Some("build".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut built);
    built.settle(completed_with(Some("exit code: 0"), None));
    tick().await;

    let listed = call(&env.ctx, "job_list", json!({}), Some(&alice)).await;
    assert!(!listed.is_error(), "expected job_list success");
    let values = expect_value(&listed).as_array().unwrap().clone();
    assert_eq!(values.len(), 3);
    assert_eq!(values[0]["id"], env.id("bash-1").to_string());
    assert_eq!(
        (
            &values[0]["kind"],
            &values[0]["label"],
            &values[0]["status"]
        ),
        (&json!("bash"), &json!("pnpm test"), &json!("running"))
    );
    assert_eq!(values[2]["id"], env.id("bash-2").to_string());
    assert_eq!(
        (
            &values[2]["label"],
            &values[2]["status"],
            &values[2]["detail"]
        ),
        (&json!("build"), &json!("completed"), &json!("exit code: 0"))
    );
    for job in &values {
        assert!(job.get("owner").is_none());
        assert!(job.get("output").is_none());
    }
    assert_eq!(
        text(&listed),
        [
            format!("{} [bash] running — pnpm test", env.id("bash-1")),
            format!(
                "{} [test-task] running — open research",
                env.id("test-task-1")
            ),
            format!("{} [bash] completed — build", env.id("bash-2")),
        ]
        .join("\n")
    );
    // A different caller sees only the unowned job.
    let bob = env.owner("sess-bob", FakeDelivery::default()).await;
    assert_eq!(
        text(&call(&env.ctx, "job_list", json!({}), Some(&bob)).await),
        format!(
            "{} [test-task] running — open research",
            env.id("test-task-1")
        )
    );
}

// -------------------------------------------------------------------------------- job_kill

#[tokio::test]
async fn requests_cancellation_with_the_forwarded_reason() {
    let env = setup_default().await;
    let mut p = producer();
    start(&env, &mut p);

    let mut args = job_args(&env, "bash-1");
    args["reason"] = json!("superseded");
    let result = call(&env.ctx, "job_kill", args, None).await;
    assert!(!result.is_error(), "expected job_kill success");
    let value = expect_value(&result);
    assert_eq!(value["outcome"], "cancellation-requested");
    assert_eq!(value["job"]["id"], env.id("bash-1").to_string());
    assert_eq!(value["job"]["status"], "stopping");
    assert!(value["job"].get("owner").is_none());
    assert!(value["job"].get("output").is_none());
    assert_eq!(
        text(&result),
        format!("requested cancellation of job {}", env.id("bash-1"))
    );
    assert_eq!(p.cancels(), vec![Some("superseded".to_owned())]);
}

#[tokio::test]
async fn applies_the_producer_output_limit_to_a_cancellation_acknowledgement() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        output_limit_bytes: Some(8),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    let result = call(&env.ctx, "job_kill", job_args(&env, "bash-1"), None).await;
    assert!(bytes(&text(&result)) <= 8);
    assert_eq!(p.cancels(), vec![None]);
}

#[tokio::test]
async fn applies_the_producer_output_limit_to_a_normalized_cancellation_failure() {
    let env = setup_default().await;
    let mut p = producer_with(ProducerOptions {
        output_limit_bytes: Some(64),
        cancel: Some(Box::new(|_| Err("cancel failed: ".repeat(100).into()))),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);

    let result = call(&env.ctx, "job_kill", job_args(&env, "bash-1"), None).await;
    assert!(result.is_error());
    assert!(bytes(&text(&result)) <= 64);
    assert!(text(&result).contains("[result truncated]"));
    assert_eq!(
        env.jobs().get(&id, None).unwrap().status,
        JobStatus::Running
    );
}

#[tokio::test]
async fn bounds_single_text_post_policy_while_preserving_structured_policy_results() {
    let env = setup_default().await;
    post_policy(&env, |exec| {
        if exec.name != "job_kill" {
            return None;
        }
        match exec.arguments.get("reason").and_then(Value::as_str) {
            Some("replace") => Some(accept_with("r".repeat(1_000))),
            Some("block") => Some(block_with(vec![ContentBlock::text("b".repeat(1_000))])),
            Some("multi") => Some(block_with(vec![
                ContentBlock::text("first"),
                ContentBlock::text("second"),
            ])),
            Some("reasoning") => Some(block_with(vec![ContentBlock::reasoning("policy detail")])),
            _ => None,
        }
    });
    for _ in 0..4 {
        env.jobs()
            .start(
                producer_with(ProducerOptions {
                    output_limit_bytes: Some(64),
                    ..ProducerOptions::default()
                })
                .spec(),
            )
            .unwrap();
    }
    let with_reason = |legacy: &str, reason: &str| {
        let mut args = job_args(&env, legacy);
        args["reason"] = json!(reason);
        args
    };

    let replaced = call(&env.ctx, "job_kill", with_reason("bash-1", "replace"), None).await;
    assert!(!replaced.is_error());
    assert!(bytes(&text(&replaced)) <= 64);
    assert!(text(&replaced).contains("[result truncated]"));

    let blocked = call(&env.ctx, "job_kill", with_reason("bash-2", "block"), None).await;
    assert!(blocked.is_error());
    assert!(bytes(&text(&blocked)) <= 64);
    assert!(text(&blocked).contains("[result truncated]"));

    let multi = call(&env.ctx, "job_kill", with_reason("bash-3", "multi"), None).await;
    assert_eq!(
        multi.content(),
        [ContentBlock::text("first"), ContentBlock::text("second")]
    );

    let reasoning = call(
        &env.ctx,
        "job_kill",
        with_reason("bash-4", "reasoning"),
        None,
    )
    .await;
    assert_eq!(
        reasoning.content(),
        [ContentBlock::reasoning("policy detail")]
    );
}

#[tokio::test]
async fn reports_an_already_finished_job_without_consuming_its_pending_delta() {
    let env = setup_default().await;
    let mut p = producer();
    start(&env, &mut p);
    p.append("unread tail");
    p.settle(completed_with(Some("exit code: 0"), None));
    tick().await;

    let killed = call(&env.ctx, "job_kill", job_args(&env, "bash-1"), None).await;
    assert!(!killed.is_error(), "expected job_kill success");
    let value = expect_value(&killed);
    assert_eq!(value["outcome"], "already-finished");
    assert_eq!(value["job"]["status"], "completed");
    assert_eq!(value["job"]["detail"], "exit code: 0");
    assert_eq!(
        text(&killed),
        format!(
            "job {} had already finished [status: completed, exit code: 0]",
            env.id("bash-1")
        )
    );
    // The kill described the job via a non-consuming projection: the delta is intact.
    let read = call(&env.ctx, "job_output", job_args(&env, "bash-1"), None).await;
    assert_eq!(
        text(&read),
        "unread tail\n[status: completed, exit code: 0]"
    );
}

#[tokio::test]
async fn job_kill_rejects_an_empty_job_id_as_an_errored_result() {
    let env = setup_default().await;
    assert!(
        call(&env.ctx, "job_kill", json!({ "job_id": "" }), None)
            .await
            .is_error()
    );
}

// ------------------------------------------------------ tool-owned UI presentation (presentCall)

#[tokio::test]
async fn renders_generic_cards_for_all_three_control_tools() {
    let env = setup_default().await;
    let tools = env.ctx.get::<Tools>().unwrap();
    let present = |name: &str, args: Value| {
        let definition = tools.get(name, None, None).unwrap();
        (definition.present_call.as_ref().unwrap())(&args)
    };
    let generic = |title: &str, kind: ToolCallKind, raw_input: Option<&str>| {
        Some(ToolCallView::Generic(hoh_tools::GenericCallView {
            title: title.to_owned(),
            kind: Some(kind),
            raw_input: raw_input.map(|input| json!(input)),
            content: None,
            locations: None,
        }))
    };
    assert_eq!(
        present("job_output", json!({ "job_id": "bash-1" })),
        generic(
            "Read output from background job bash-1",
            ToolCallKind::Read,
            Some("bash-1")
        )
    );
    assert_eq!(
        present("job_list", json!({})),
        generic("List background jobs", ToolCallKind::Read, None)
    );
    assert_eq!(
        present("job_kill", json!({ "job_id": "test-task-2" })),
        generic(
            "Kill background job test-task-2",
            ToolCallKind::Execute,
            Some("test-task-2")
        )
    );
}

// ------------------------------------------------------- completion notices across scoped mounts

/// Two agent presets mounting `tool-jobs` over ONE host registry: each mount subscribes with
/// `{ owners: 'scope' }`, which the registry files into the mount's own scope layer and reaches
/// along the owner's scope chain. Only the mount whose scope the owner belongs to sees the
/// settlement.
#[tokio::test]
async fn delivers_one_notice_from_the_owning_scope_when_two_mounts_share_the_registry() {
    use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope, scope_of};
    let env = setup_unmounted(json!({})).await;
    let standing_a =
        create_scope(&env.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let standing_b =
        create_scope(&env.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    mount::<ToolJobs>(&standing_a.ctx, json!({})).await;
    mount::<ToolJobs>(&standing_b.ctx, json!({})).await;

    // The agent joins preset A exactly as `agentPresets.compose` binds it.
    let agent_scope = create_scope(
        &env.ctx,
        ScopeKey::new(),
        CreateScopeOptions {
            parent: scope_of(&standing_a.ctx),
        },
    )
    .unwrap();
    let owner = env
        .owner_in(
            "sess-scoped",
            FakeDelivery::default(),
            Some(agent_scope.ctx.clone()),
        )
        .await;

    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("pnpm test".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);
    p.settle(completed_with(Some("exit code: 0"), None));
    tick().await;

    assert_eq!(owner.injected().len(), 1);
}

// --------------------------------------------------------------- completion notice delivery

#[tokio::test]
async fn opens_a_turn_on_an_idle_owner_when_a_job_settles() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("pnpm test".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    p.settle(completed_with(Some("exit code: 0"), None));
    tick().await;
    assert_eq!(owner.followed().len(), 1);
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn delivers_the_completion_notice_for_a_human_kill_reason_included() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("pnpm run watch".to_owned()),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);

    // A kill outside the model's own job_kill (the web client's stop button) claims nothing in
    // the ledger, so the notice stays due.
    env.jobs()
        .kill(&id, Some(&owner.id()), Some("cancelled by the user"))
        .unwrap();
    p.settle(killed_with("signal: SIGTERM"));
    tick().await;
    let injected = owner.injected();
    assert_eq!(injected.len(), 1);
    assert_eq!(
        notice_text(&injected[0]),
        format!(
            "background job {} (bash: pnpm run watch) finished [status: killed, signal: SIGTERM; cancelled by the user]. No new output.",
            env.id("bash-1")
        )
    );
}

#[tokio::test]
async fn never_wakes_an_idle_owner_under_quiet_delivery() {
    let env = setup(json!({ "completionDelivery": "quiet" }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);

    p.settle(completed());
    tick().await;
    assert_eq!(owner.injected().len(), 1);
    assert!(owner.followed().is_empty());
}

#[tokio::test]
async fn wakes_an_idle_owner_for_every_completion_when_no_wake_budget_is_set() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;

    // Four unattended completions in a row must each open a turn; no user input arrives in
    // between to refill anything.
    settle_tasks(&env, &owner, 4).await;
    assert_eq!(owner.followed().len(), 4);
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn degrades_to_injection_once_the_consecutive_wake_budget_is_spent() {
    let env = setup(json!({ "maxConsecutiveWakes": 2 }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;

    settle_tasks(&env, &owner, 3).await;
    // A woken turn that starts another job is the self-exciting case: the budget stops the
    // chain, and the notice still reaches the inbox.
    assert_eq!(owner.followed().len(), 2);
    assert_eq!(owner.injected().len(), 1);
}

#[tokio::test]
async fn restores_the_wake_budget_when_the_owner_claims_a_user_message() {
    let env = setup(json!({ "maxConsecutiveWakes": 1 }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;

    settle_tasks(&env, &owner, 2).await;
    assert_eq!(owner.followed().len(), 1);

    owner.claim(&env.ctx, user_message("carry on"));
    settle_tasks(&env, &owner, 1).await;
    assert_eq!(owner.followed().len(), 2);
}

#[tokio::test]
async fn neither_wakes_nor_injects_into_an_owner_its_own_teardown_is_draining() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        settle_on_cancel: Some(killed()),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    // Disposal cancels and settles the owned job with a teardown cause. Waking here would spend a
    // model request on an agent the host is destroying.
    owner.dispose_scope().await;
    tick().await;
    assert!(owner.followed().is_empty());
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn neither_wakes_nor_injects_when_the_teardown_cancel_itself_failed() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;
    let mut p = producer_with(ProducerOptions {
        label: Some("broken producer".to_owned()),
        owner: Some(owner.id()),
        cancel: Some(Box::new(|_| Err("cancel boom".into()))),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    // The registry force-fails the record instead of deadlocking. That settlement also announces
    // a teardown cause, so a throwing producer is not enough to spend a model request on an owner
    // being destroyed.
    owner.dispose_scope().await;
    tick().await;
    assert!(
        env.warnings()
            .iter()
            .any(|w| w.contains("work may be orphaned"))
    );
    assert!(owner.followed().is_empty());
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn keeps_the_budget_spent_when_the_owner_only_claims_plugin_notices() {
    let env = setup(json!({ "maxConsecutiveWakes": 1 }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::idle()).await;

    settle_tasks(&env, &owner, 1).await;
    owner.claim(
        &env.ctx,
        notice_message("background job bash-1 finished", "bash"),
    );
    settle_tasks(&env, &owner, 1).await;
    assert_eq!(owner.followed().len(), 1);
}

// ------------------------------------------------------------------------ completion notices

fn message_json(message: &hoh_llm::UserMessage) -> Value {
    serde_json::to_value(message).unwrap()
}

#[tokio::test]
async fn injects_a_notice_into_the_owning_agent_when_an_uncollected_job_settles() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("pnpm test".to_owned()),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    p.settle(completed_with(Some("exit code: 0"), None));
    tick().await;
    let injected = owner.injected();
    assert_eq!(injected.len(), 1);
    let mut actual = message_json(&injected[0]);
    assert!(actual["id"].is_string());
    actual.as_object_mut().unwrap().remove("id");
    assert_eq!(
        actual,
        json!({
            "role": "user",
            "content": [{ "type": "text", "text": format!("background job {} (bash: pnpm test) finished [status: completed, exit code: 0]. No new output.", env.id("bash-1")) }],
            "source": {
                "kind": "tool-jobs",
                "form": "notice",
                "summary": "bash pnpm test [status: completed, exit code: 0]"
            }
        })
    );
    assert_eq!(
        serde_json::to_string(&injected[0].source).unwrap(),
        r#"{"kind":"tool-jobs","form":"notice","summary":"bash pnpm test [status: completed, exit code: 0]"}"#
    );
}

#[tokio::test]
async fn keeps_collection_guidance_when_a_bounded_notice_cannot_carry_the_id() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut first = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("test-task"),
        label: Some("x".repeat(1_000)),
        output_limit_bytes: Some(55),
        ..ProducerOptions::default()
    });
    start(&env, &mut first);
    first.settle(completed_with(Some(&"d".repeat(1_000)), None));
    tick().await;

    let injected = owner.injected();
    assert_eq!(injected.len(), 1);
    assert_eq!(
        notice_text(&injected[0]),
        "background job ended; use job_list for id."
    );
    // The label and status detail are unbounded caller text, so the durable one-line account
    // caps itself rather than committing their full length.
    assert_eq!(
        injected[0].source.data["summary"],
        json!(format!("test-task {}…", "x".repeat(109)))
    );

    let mut second = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("test-task"),
        label: Some("x".repeat(1_000)),
        output_limit_bytes: Some(60),
        ..ProducerOptions::default()
    });
    start(&env, &mut second);
    second.settle(completed_with(Some(&"d".repeat(1_000)), None));
    tick().await;

    let notice = notice_text(&owner.injected()[1]);
    assert!(bytes(&notice) <= 80);
    assert_eq!(
        notice,
        format!(
            "background job {}\nDone; job_output.",
            env.id("test-task-2")
        )
    );
}

#[tokio::test]
async fn guides_collection_through_job_list_when_the_pty_id_exceeds_its_output_limit() {
    let env = setup_default().await;
    for _ in 0..99 {
        let mut prior = producer_with(ProducerOptions {
            kind: Some("pty-send"),
            ..ProducerOptions::default()
        });
        start(&env, &mut prior);
        prior.settle(completed());
        tick().await;
    }
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut target = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("pty-send"),
        label: Some("x".repeat(1_000)),
        output_limit_bytes: Some(56),
        ..ProducerOptions::default()
    });
    start(&env, &mut target);

    target.settle(completed_with(Some(&"d".repeat(1_000)), None));
    tick().await;

    let notice = notice_text(&owner.injected()[0]);
    assert!(bytes(&notice) <= 56);
    assert_eq!(notice, "background job ended; use job_list for id.");
}

#[tokio::test]
async fn keeps_a_complete_job_id_at_the_exact_notice_boundary_and_uses_spare_bytes_for_context() {
    let env = setup_default().await;
    let owner = env.owner("notice-boundary", FakeDelivery::default()).await;
    let fixed_bytes = bytes(&format!(
        "background job test-task-{}\n[notice truncated]\nDone; job_output.",
        "0".repeat(16)
    ));
    for (index, extra) in ["", " (t"].into_iter().enumerate() {
        let mut p = producer_with(ProducerOptions {
            owner: Some(owner.id()),
            kind: Some("test-task"),
            label: Some("x".repeat(1_000)),
            output_limit_bytes: Some((fixed_bytes + extra.len()) as u64),
            ..ProducerOptions::default()
        });
        let id = start(&env, &mut p);
        assert!(id.to_string().starts_with("test-task-") && id.to_string().len() == 26);
        p.settle(completed());
        tick().await;
        let notice = notice_text(&owner.injected()[index]);
        assert_eq!(
            notice,
            format!("background job {id}{extra}\n[notice truncated]\nDone; job_output.")
        );
        assert_eq!(bytes(&notice), fixed_bytes + extra.len());
    }
}

#[tokio::test]
async fn keeps_a_complete_job_list_action_when_the_id_exceeds_a_smaller_budget() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let make = |limit: u64| {
        producer_with(ProducerOptions {
            owner: Some(owner.id()),
            kind: Some("pty-send"),
            label: Some("x".repeat(100)),
            output_limit_bytes: Some(limit),
            ..ProducerOptions::default()
        })
    };
    let (mut tiny, mut short, mut minimal) = (make(8), make(32), make(3));
    start(&env, &mut tiny);
    start(&env, &mut short);
    start(&env, &mut minimal);

    tiny.settle(completed());
    short.settle(completed());
    minimal.settle(completed());
    tick().await;

    let injected = owner.injected();
    let tiny_notice = notice_text(&injected[0]);
    let short_notice = notice_text(&injected[1]);
    assert!(bytes(&tiny_notice) <= 8);
    assert_eq!(tiny_notice, "job_list");
    assert!(bytes(&short_notice) <= 32);
    assert_eq!(short_notice, "job_list");
    assert_eq!(notice_text(&injected[2]), "job");
}

#[tokio::test]
async fn suppresses_the_notice_for_a_job_the_model_already_killed() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);

    call(&env.ctx, "job_kill", job_args(&env, "bash-1"), Some(&owner)).await;
    p.settle(killed());
    tick().await;
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn suppresses_the_notice_when_a_wait_returned_the_terminal_state() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("test-task"),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    let mut args = job_args(&env, "test-task-1");
    args["wait"] = json!(true);
    let pending = spawn_call(&env, "job_output", args, Some(&owner));
    tick().await;
    p.settle(completed_with(None, Some("answer")));
    assert!(text(&pending.await.unwrap()).contains("answer"));
    tick().await;
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn a_timed_out_wait_withdraws_only_its_own_claim_a_concurrent_wait_keeps_the_settlement_covered()
 {
    let env = setup(
        json!({ "waitTimeoutMs": 10, "maxWaitTimeoutMs": 1000 }),
        json!({}),
    )
    .await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);

    let mut patient_args = job_args(&env, "bash-1");
    patient_args["wait"] = json!(true);
    patient_args["timeout_ms"] = json!(1000);
    let patient = spawn_call(&env, "job_output", patient_args, Some(&owner));
    tick().await;
    let mut short_args = job_args(&env, "bash-1");
    short_args["wait"] = json!(true);
    short_args["timeout_ms"] = json!(10);
    let short = call(&env.ctx, "job_output", short_args, Some(&owner)).await;
    assert_eq!(text(&short), "(no new output)\n[status: running]");
    p.settle(completed());
    assert_eq!(
        text(&patient.await.unwrap()),
        "(no new output)\n[status: completed]"
    );
    tick().await;
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn withdraws_the_wait_claim_when_the_wait_times_out_so_the_later_settlement_still_notifies() {
    let env = setup(
        json!({ "waitTimeoutMs": 10, "maxWaitTimeoutMs": 20 }),
        json!({}),
    )
    .await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);

    let mut args = job_args(&env, "bash-1");
    args["wait"] = json!(true);
    let timed_out = call(&env.ctx, "job_output", args, Some(&owner)).await;
    assert_eq!(text(&timed_out), "(no new output)\n[status: running]");
    p.settle(completed());
    tick().await;
    assert_eq!(owner.injected().len(), 1);
}

#[tokio::test]
async fn withdraws_the_wait_claim_when_the_caller_aborts_the_wait() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);

    let aborter = hoh_timeout::AbortController::new();
    let ctx = env.ctx.clone();
    let agent = owner.clone();
    let signal = aborter.signal();
    let mut args = job_args(&env, "bash-1");
    args["wait"] = json!(true);
    let pending = tokio::spawn(async move {
        call_with_signal(&ctx, "job_output", args, Some(&agent), signal).await
    });
    tick().await;
    aborter.abort();
    let result = pending.await.unwrap();
    assert!(result.is_error());
    assert_eq!(
        error_info(&result),
        Some(("AbortError".to_owned(), "ABORTED".to_owned()))
    );
    assert!(text(&result).contains("wait aborted"));

    p.settle(completed());
    tick().await;
    assert_eq!(owner.injected().len(), 1);
}

#[tokio::test]
async fn delivers_the_notice_when_the_wait_aborts_and_the_job_settles_before_the_rejection_lands() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("test-task"),
        ..ProducerOptions::default()
    });
    start(&env, &mut p);

    let aborter = hoh_timeout::AbortController::new();
    let ctx = env.ctx.clone();
    let agent = owner.clone();
    let signal = aborter.signal();
    let mut args = job_args(&env, "test-task-1");
    args["wait"] = json!(true);
    let pending = tokio::spawn(async move {
        call_with_signal(&ctx, "job_output", args, Some(&agent), signal).await
    });
    tick().await;
    // One synchronous span: a push producer's settlement lands before the abort rejection
    // reaches the tool. The tool result carries the abort, not the outcome, so the settlement
    // notice is the model's only completion record.
    aborter.abort();
    p.settle(completed_with(None, Some("answer")));
    let result = pending.await.unwrap();
    assert!(result.is_error());
    assert!(text(&result).contains("wait aborted"));
    tick().await;
    assert_eq!(owner.injected().len(), 1);
}

#[tokio::test]
async fn keeps_a_claim_on_one_job_while_another_job_settles() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut killed_job = owned_by(&owner);
    let mut other = owned_by(&owner);
    start(&env, &mut killed_job);
    start(&env, &mut other);

    call(&env.ctx, "job_kill", job_args(&env, "bash-1"), Some(&owner)).await;
    other.settle(completed());
    tick().await;
    assert_eq!(owner.injected().len(), 1);
    killed_job.settle(killed());
    tick().await;
    assert_eq!(owner.injected().len(), 1);
}

#[tokio::test]
async fn drops_the_notice_for_unowned_jobs_without_failing() {
    let env = setup_default().await;
    // Unowned: settles with nobody to notify — nothing fails.
    let mut unowned = producer();
    start(&env, &mut unowned);
    unowned.settle(completed());
    tick().await;
}

#[tokio::test]
async fn delivers_to_the_agent_currently_registered_for_the_owner_session() {
    let env = setup_default().await;
    // The settlement names the owner session; the notice goes to whichever agent holds that
    // session when the job settles, which can read the job through the same session id.
    let old_owner = env.owner("shared", FakeDelivery::default()).await;
    let mut p = owned_by(&old_owner);
    start(&env, &mut p);

    old_owner.detach().await;
    let replacement = env.owner("shared", FakeDelivery::default()).await;
    p.settle(completed());
    tick().await;

    assert!(old_owner.injected().is_empty());
    assert_eq!(replacement.injected().len(), 1);
    let read = call(
        &env.ctx,
        "job_output",
        job_args(&env, "bash-1"),
        Some(&replacement),
    )
    .await;
    assert_eq!(text(&read), "(no new output)\n[status: completed]");
}

#[tokio::test]
async fn drops_the_notice_when_the_agent_registry_left_before_settlement() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);

    env.agents_fiber.clone().unwrap().dispose().await;
    p.settle(completed());
    tick().await;
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn drops_the_notice_when_the_owner_session_has_no_live_agent_at_settlement() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);

    owner.detach().await;
    p.settle(completed());
    tick().await;
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn surfaces_an_inject_failure_through_listener_containment_a_real_bug_must_be_visible() {
    let env = setup_default().await;
    let owner = env
        .owner(
            "sess-1",
            FakeDelivery {
                inject_error: Some("unexpected inject bug"),
                ..FakeDelivery::default()
            },
        )
        .await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.settle(completed());
    tick().await;
    // The failure escapes the notice listener and is contained (logged) by the registry's
    // per-listener containment — visible, not swallowed.
    assert!(
        env.warnings()
            .iter()
            .any(|w| w.contains("unexpected inject bug"))
    );
}

// ------------------------------------------------------- released waits crossed by a settlement

async fn release_setup() -> (Env, Owner) {
    let env = setup(json!({ "completionDelivery": "quiet" }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    (env, owner)
}

fn wait_args(env: &Env, legacy: &str) -> Value {
    let mut args = job_args(env, legacy);
    args["wait"] = json!(true);
    args
}

#[tokio::test]
async fn delivers_a_settlement_through_the_released_read_that_is_about_to_carry_it() {
    let (env, owner) = release_setup().await;
    let mut p = owned_by(&owner);
    let id = start(&env, &mut p);
    p.append("partial\n");
    let pending = spawn_call(&env, "job_output", wait_args(&env, "bash-1"), Some(&owner));
    tick().await;
    p.settle(completed_with(Some("exit code: 0"), Some("final output")));
    owner.insert_next_step(&env.ctx, "RELEASE_INPUT");
    assert_eq!(
        text(&pending.await.unwrap()),
        "partial\nfinal output\n[status: completed, exit code: 0]"
    );
    assert!(
        env.jobs()
            .read(&id, Some(&owner.id()))
            .unwrap()
            .result
            .is_none()
    );
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn still_notifies_a_settlement_that_follows_a_released_read_reporting_running() {
    let (env, owner) = release_setup().await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.append("partial\n");
    let pending = spawn_call(&env, "job_output", wait_args(&env, "bash-1"), Some(&owner));
    tick().await;
    owner.insert_next_step(&env.ctx, "RELEASE_INPUT");
    assert_eq!(
        text(&pending.await.unwrap()),
        "partial\n[status: running, user steered]"
    );
    assert!(owner.injected().is_empty());
    p.settle(completed_with(Some("exit code: 0"), None));
    tick().await;
    assert_eq!(owner.injected().len(), 1);
}

#[tokio::test]
async fn ignores_another_agents_insertion_while_this_owner_waits() {
    let (env, owner) = release_setup().await;
    let other = env.owner("sess-2", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.append("partial\n");
    let pending = spawn_call(&env, "job_output", wait_args(&env, "bash-1"), Some(&owner));
    tick().await;
    other.insert_next_step(&env.ctx, "OTHER_AGENT_INPUT");
    p.settle(completed_with(Some("exit code: 0"), Some("final output")));
    assert_eq!(
        text(&pending.await.unwrap()),
        "partial\nfinal output\n[status: completed, exit code: 0]"
    );
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn releases_both_parallel_reads_and_notifies_the_settlement_once() {
    let (env, owner) = release_setup().await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.append("partial\n");
    let first = spawn_call(&env, "job_output", wait_args(&env, "bash-1"), Some(&owner));
    let second = spawn_call(&env, "job_output", wait_args(&env, "bash-1"), Some(&owner));
    tick().await;
    owner.insert_next_step(&env.ctx, "RELEASE_INPUT");
    let first = text(&first.await.unwrap());
    let second = text(&second.await.unwrap());
    // Which of the two parallel reads takes the buffered output depends on scheduling; one gets
    // it, the other reports nothing new.
    let mut texts = [first, second];
    texts.sort();
    assert_eq!(
        texts,
        [
            "(no new output)\n[status: running, user steered]".to_owned(),
            "partial\n[status: running, user steered]".to_owned()
        ]
    );
    assert!(owner.injected().is_empty());
    p.settle(completed());
    tick().await;
    assert_eq!(owner.injected().len(), 1);
}

#[tokio::test]
async fn delivers_the_withheld_notice_when_the_released_read_never_commits() {
    let (env, owner) = release_setup().await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.append("partial\n");
    let aborter = hoh_timeout::AbortController::new();
    let ctx = env.ctx.clone();
    let agent = owner.clone();
    let signal = aborter.signal();
    let args = wait_args(&env, "bash-1");
    let pending = tokio::spawn(async move {
        call_with_signal(&ctx, "job_output", args, Some(&agent), signal).await
    });
    tick().await;
    owner.insert_next_step(&env.ctx, "RELEASE_INPUT");
    // Settlement lands while the release claim is open, so its notice waits for that read; the
    // cancellation that follows discards the read, and the withheld notice is delivered.
    p.settle(completed_with(Some("exit code: 0"), Some("final output")));
    aborter.abort();
    let result = pending.await.unwrap();
    assert!(result.is_error());
    assert!(text(&result).contains("aborted"));
    tick().await;
    let injected = owner.injected();
    assert_eq!(injected.len(), 1);
    assert!(
        serde_json::to_string(&injected[0])
            .unwrap()
            .contains("finished")
    );
}

#[tokio::test]
async fn does_not_repeat_a_completion_the_model_already_collected_by_killing_the_job() {
    let (env, owner) = release_setup().await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.append("partial\n");
    let aborter = hoh_timeout::AbortController::new();
    let ctx = env.ctx.clone();
    let agent = owner.clone();
    let signal = aborter.signal();
    let args = wait_args(&env, "bash-1");
    let pending = tokio::spawn(async move {
        call_with_signal(&ctx, "job_output", args, Some(&agent), signal).await
    });
    tick().await;
    // The model killed the job it was waiting on; that result is its delivery.
    let killed_result = call(&env.ctx, "job_kill", job_args(&env, "bash-1"), Some(&owner)).await;
    assert!(text(&killed_result).contains("requested"));
    owner.insert_next_step(&env.ctx, "RELEASE_INPUT");
    p.settle(killed_with("cancelled by the user"));
    aborter.abort();
    let result = pending.await.unwrap();
    assert!(result.is_error());
    tick().await;
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn keeps_waiting_through_a_plugin_notice_and_carries_the_settlement_in_its_own_read() {
    let (env, owner) = release_setup().await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.append("partial\n");
    let mut pending = spawn_call(&env, "job_output", wait_args(&env, "bash-1"), Some(&owner));
    tick().await;
    p.progress("3/10 files");
    // A plugin notice is not user input: it stays queued for the next step instead of
    // interrupting the wait.
    owner.insert_notice(&env.ctx, "another job finished");
    tick().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), &mut pending)
            .await
            .is_err()
    );
    p.settle(completed_with(Some("exit code: 0"), Some("final output")));
    assert_eq!(
        text(&pending.await.unwrap()),
        "partial\nfinal output\n[status: completed, exit code: 0]"
    );
    assert!(owner.injected().is_empty());
}

#[tokio::test]
async fn releases_for_a_user_steer_even_while_a_plugin_notice_is_queued() {
    let (env, owner) = release_setup().await;
    let mut p = owned_by(&owner);
    start(&env, &mut p);
    p.append("partial\n");
    let pending = spawn_call(&env, "job_output", wait_args(&env, "bash-1"), Some(&owner));
    tick().await;
    p.progress("3/10 files");
    owner.insert_notice(&env.ctx, "another job finished");
    owner.insert_next_step(&env.ctx, "RELEASE_INPUT");
    assert_eq!(
        text(&pending.await.unwrap()),
        "partial\n[status: running, 3/10 files; user steered]"
    );
    assert!(owner.injected().is_empty());
    p.settle(completed());
}

// ------------------------------------------------------------------ completion notice output

fn first_notice(owner: &Owner) -> hoh_llm::UserMessage {
    owner.injected().remove(0)
}

#[tokio::test]
async fn carries_unread_output_that_fits_and_the_read_is_spent_once_the_notice_is_logged() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("pnpm test".to_owned()),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);
    p.append_on("ok 3 tests\n", JobChannel::Stdout);
    p.settle(completed_with(Some("exit code: 0"), None));
    tick().await;

    let notice = first_notice(&owner);
    assert_eq!(
        notice_text(&notice),
        format!(
            "background job {id} (bash: pnpm test) finished [status: completed, exit code: 0]. Output:\nok 3 tests"
        )
    );
    owner.log_user_message(&notice);
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "(no new output)\n[status: completed, exit code: 0]"
    );
}

fn exact_bound() -> u64 {
    bytes(&format!(
        "background job bash-{} (bash: exact) finished [status: completed]. Output:\n日本語",
        "0".repeat(16)
    )) as u64
}

#[tokio::test]
async fn inlines_a_notice_of_exactly_inline_output_bytes_counted_in_utf8_bytes() {
    let bound = exact_bound();
    let env = setup(json!({ "inlineOutputBytes": bound }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let make = || {
        producer_with(ProducerOptions {
            owner: Some(owner.id()),
            label: Some("exact".to_owned()),
            ..ProducerOptions::default()
        })
    };
    let (mut fits, mut over) = (make(), make());
    let fits_id = start(&env, &mut fits);
    fits.append("日本語");
    fits.settle(completed());
    let over_id = start(&env, &mut over);
    over.append("日本語x");
    over.settle(completed());
    tick().await;

    let injected = owner.injected();
    assert_eq!(
        notice_text(&injected[0]),
        format!(
            "background job {fits_id} (bash: exact) finished [status: completed]. Output:\n日本語"
        )
    );
    assert_eq!(bytes(&notice_text(&injected[0])) as u64, bound);
    assert_eq!(
        notice_text(&injected[1]),
        format!(
            "background job {over_id} (bash: exact) finished [status: completed]. Read its output with job_output."
        )
    );
    owner.log_user_message(&injected[1]);
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": over_id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "日本語x\n[status: completed]"
    );
}

#[tokio::test]
async fn points_a_notice_one_byte_over_inline_output_bytes_at_job_output_even_when_the_stream_fits()
{
    let env = setup(json!({ "inlineOutputBytes": exact_bound() - 1 }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("exact".to_owned()),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);
    p.append("日本語");
    p.settle(completed());
    tick().await;

    let notice = first_notice(&owner);
    assert_eq!(
        notice_text(&notice),
        format!(
            "background job {id} (bash: exact) finished [status: completed]. Read its output with job_output."
        )
    );
    owner.log_user_message(&notice);
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "日本語\n[status: completed]"
    );
}

#[tokio::test]
async fn leaves_a_large_final_result_for_job_output_instead_of_inlining_it() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("test-task"),
        label: Some("answer".to_owned()),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);
    p.settle(completed_with(None, Some(&"r".repeat(5_000))));
    tick().await;

    let notice = first_notice(&owner);
    assert_eq!(
        notice_text(&notice),
        format!(
            "background job {id} (test-task: answer) finished [status: completed]. Read its output with job_output."
        )
    );
    // A logged pointer notice keeps the read: the model collects it through job_output.
    owner.log_user_message(&notice);
    let args = json!({ "job_id": id.to_string() });
    assert_eq!(
        text(&call(&env.ctx, "job_output", args.clone(), Some(&owner)).await),
        format!("{}\n[status: completed]", "r".repeat(5_000))
    );
    assert_eq!(
        text(&call(&env.ctx, "job_output", args, Some(&owner)).await),
        "(no new output)\n[status: completed]"
    );
}

#[tokio::test]
async fn never_inlines_when_inline_output_bytes_is_zero() {
    let env = setup(json!({ "inlineOutputBytes": 0 }), json!({})).await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("quiet".to_owned()),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);
    p.append("kept in the registry\n");
    p.settle(completed());
    tick().await;

    assert_eq!(
        notice_text(&first_notice(&owner)),
        format!(
            "background job {id} (bash: quiet) finished [status: completed]. Read its output with job_output."
        )
    );
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "kept in the registry\n[status: completed]"
    );
}

#[tokio::test]
async fn leaves_a_notice_over_the_producer_cap_for_job_output() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("test-task"),
        label: Some("capped".to_owned()),
        output_limit_bytes: Some(140),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);
    p.settle(completed_with(None, Some(&format!("{}z", "r".repeat(499)))));
    tick().await;

    assert_eq!(
        notice_text(&first_notice(&owner)),
        format!(
            "background job {id} (test-task: capped) finished [status: completed]. Read its output with job_output."
        )
    );
    let read = text(
        &call(
            &env.ctx,
            "job_output",
            json!({ "job_id": id.to_string() }),
            Some(&owner),
        )
        .await,
    );
    assert!(bytes(&read) <= 140);
    assert!(read.ends_with("z\n[output truncated]\n[status: completed]"));
    assert!(
        read.trim_end_matches("z\n[output truncated]\n[status: completed]")
            .chars()
            .all(|c| c == 'r')
    );
}

#[tokio::test]
async fn withdraws_a_pending_notice_when_job_output_returns_the_same_output() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        kind: Some("test-task"),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);
    p.append("partial\n");
    p.settle(completed_with(None, Some("answer")));
    tick().await;

    let mut args = json!({ "job_id": id.to_string() });
    args["wait"] = json!(true);
    assert_eq!(
        text(&call(&env.ctx, "job_output", args, Some(&owner)).await),
        "partial\nanswer\n[status: completed]"
    );
    assert_eq!(owner.removed(), vec![first_notice(&owner).id.clone()]);
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "(no new output)\n[status: completed]"
    );
    assert_eq!(owner.removed().len(), 1);
}

#[tokio::test]
async fn keeps_the_output_of_an_inline_notice_that_was_claimed_but_never_logged() {
    let env = setup_default().await;
    let owner = env
        .owner(
            "sess-1",
            FakeDelivery {
                remove_result: false,
                ..FakeDelivery::default()
            },
        )
        .await;
    let mut p = owned_by(&owner);
    let id = start(&env, &mut p);
    p.append("lost line\n");
    p.settle(completed());
    tick().await;

    // A rejected or cancelled step claims the notice without logging it.
    let notice = first_notice(&owner);
    owner.claim(&env.ctx, notice.clone());
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "lost line\n[status: completed]"
    );
    assert_eq!(owner.removed(), vec![notice.id.clone()]);
}

#[tokio::test]
async fn spends_only_the_inline_read_of_the_logged_notice_in_its_own_session() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let other = env.owner("sess-2", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    let id = start(&env, &mut p);
    p.append("kept\n");
    p.settle(completed());
    tick().await;

    let notice = first_notice(&owner);
    other.log_user_message(&notice);
    owner.log_user_message(&notice_message("x", "x"));
    owner.log_user_message(&user_message("hi"));
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "kept\n[status: completed]"
    );
}

#[tokio::test]
async fn withdraws_the_notice_from_the_agent_now_registered_for_the_owner_session() {
    let env = setup_default().await;
    let old_owner = env.owner("shared", FakeDelivery::default()).await;
    let mut p = owned_by(&old_owner);
    let id = start(&env, &mut p);
    p.settle(completed());
    tick().await;
    let notice = first_notice(&old_owner);

    old_owner.detach().await;
    let replacement = env.owner("shared", FakeDelivery::default()).await;
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&replacement)
            )
            .await
        ),
        "(no new output)\n[status: completed]"
    );
    assert!(old_owner.removed().is_empty());
    assert_eq!(replacement.removed(), vec![notice.id.clone()]);
}

#[tokio::test]
async fn withdraws_nothing_when_the_owner_session_has_no_live_agent() {
    let env = setup_default().await;
    let owner = env.owner("gone", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    let id = start(&env, &mut p);
    p.settle(completed());
    tick().await;

    owner.detach().await;
    assert_eq!(
        text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner)
            )
            .await
        ),
        "(no new output)\n[status: completed]"
    );
}

#[tokio::test]
async fn names_a_multi_line_label_by_its_first_line() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = producer_with(ProducerOptions {
        owner: Some(owner.id()),
        label: Some("cat > a.mjs <<'EOF'\nconsole.log(1)\nEOF".to_owned()),
        ..ProducerOptions::default()
    });
    let id = start(&env, &mut p);
    p.settle(completed());
    tick().await;

    assert_eq!(
        notice_text(&first_notice(&owner)),
        format!(
            "background job {id} (bash: cat > a.mjs <<'EOF'…) finished [status: completed]. No new output."
        )
    );
}

// ------------------------------------------------------------- review round 1: threaded races

/// `job_kill` whose producer settles inside `cancel`: the settlement event may run on another
/// worker before `job_kill` returns, so the model's claim must precede the kill.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_notifies_a_job_the_model_killed_even_when_cancel_settles_at_once() {
    let env = setup_default().await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    for round in 1..=50 {
        let mut p = producer_with(ProducerOptions {
            owner: Some(owner.id()),
            settle_on_cancel: Some(killed()),
            cancel_stall: Some(std::time::Duration::from_millis(5)),
            ..ProducerOptions::default()
        });
        let id = start(&env, &mut p);
        let result = call(
            &env.ctx,
            "job_kill",
            json!({ "job_id": id.to_string() }),
            Some(&owner),
        )
        .await;
        assert!(!result.is_error(), "round {round}");
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    tick().await;
    assert!(
        owner.injected().is_empty(),
        "duplicate notices: {}",
        owner.injected().len()
    );
}

/// A user input releases a wait while the job settles in the same instant: the completion
/// reaches the model once (through the released read), never also as a notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivers_a_settlement_racing_a_release_exactly_once() {
    let (env, owner) = release_setup().await;
    for round in 1..=100 {
        let mut p = owned_by(&owner);
        let id = start(&env, &mut p);
        let mut args = json!({ "job_id": id.to_string() });
        args["wait"] = json!(true);
        let ctx = env.ctx.clone();
        let agent = owner.clone();
        let pending =
            tokio::spawn(async move { call(&ctx, "job_output", args, Some(&agent)).await });
        tick().await;
        let settle =
            tokio::task::spawn_blocking(move || p.settle(completed_with(None, Some("done"))));
        owner.insert_next_step(&env.ctx, "RELEASE_INPUT");
        settle.await.unwrap();
        let read = text(&pending.await.unwrap());
        tick().await;
        let again = text(
            &call(
                &env.ctx,
                "job_output",
                json!({ "job_id": id.to_string() }),
                Some(&owner),
            )
            .await,
        );
        // A notice the later read withdrew from the inbox was never seen by the model.
        let live_notices = owner.injected().len() - owner.removed().len();
        let seen = read.matches("done").count() + again.matches("done").count() + live_notices;
        assert_eq!(
            seen,
            1,
            "round {round}: read={read:?} again={again:?} injected={:?} removed={}",
            owner.injected().iter().map(notice_text).collect::<Vec<_>>(),
            owner.removed().len()
        );
        // Drain: the pending next-step input is consumed between rounds.
        owner.clear_injected();
    }
}

/// The notice builder's consuming read takes the result; the model's own `job_output` then runs
/// before the notice is filed and finds nothing. The result must survive in the notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keeps_the_result_when_the_model_reads_between_the_notice_read_and_its_filing() {
    let gate = Arc::new(ReadGate::default());
    let env = setup_gated(json!({}), json!({}), Some(gate.clone())).await;
    let owner = env.owner("sess-1", FakeDelivery::default()).await;
    let mut p = owned_by(&owner);
    let id = start(&env, &mut p);
    gate.arm();
    p.settle(completed_with(None, Some("done")));
    let parked = gate.clone();
    tokio::task::spawn_blocking(move || parked.wait_parked())
        .await
        .unwrap();

    // The notice builder is parked right after its consuming read.
    let read = text(
        &call(
            &env.ctx,
            "job_output",
            json!({ "job_id": id.to_string() }),
            Some(&owner),
        )
        .await,
    );
    gate.release();
    tick().await;
    tick().await;

    let notices: Vec<String> = owner.injected().iter().map(notice_text).collect();
    let seen = read.matches("done").count()
        + notices
            .iter()
            .map(|n| n.matches("done").count())
            .sum::<usize>();
    assert_eq!(seen, 1, "read={read:?} notices={notices:?}");
}
