use hoh_error::BoxError;
use hoh_loader_smoke::{
    ChildEnvironment, ExampleMode, LoaderSmokeHook, LoaderSmokeOptions, run_loader_smoke,
};
use serde_json::Value;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_loader-smoke-fixture");
const CONFIG_PATH: &str = "/tmp/fixture.cordis.yml";

fn options(label: &str, prefix: &str, kind: &str) -> LoaderSmokeOptions {
    let mut env = ChildEnvironment::new();
    env.insert("LOADER_SMOKE_FIXTURE".into(), Some(kind.into()));
    LoaderSmokeOptions {
        label: label.into(),
        bin_script: "/unused/src/bin.ts".into(),
        lib_bin_script: Some(FIXTURE.into()),
        config_path: CONFIG_PATH.into(),
        bin_args: None,
        tsconfig_path: "/unused/tsconfig.json".into(),
        mode: Some(ExampleMode::Lib),
        source_import: None,
        env,
        process_timeout_ms: None,
        prepare: None,
        inspect: None,
        expected_exit_code: None,
        temp_dir_prefix: Some(prefix.into()),
        temp_dir_parent: None,
        cwd: None,
    }
}

fn parse(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap()
}

#[tokio::test]
async fn isolates_closes_stdin_captures_output_and_removes_the_cwd() {
    let mut options = options("success fixture", "loader-smoke-success-", "success");
    options
        .env
        .insert("LOADER_SMOKE_MARKER".into(), Some("present".into()));
    let result = run_loader_smoke(options).await.unwrap();
    let output = parse(&result.stdout);
    assert_eq!(output["args"], serde_json::json!([CONFIG_PATH]));
    assert_eq!(output["marker"], "present");
    assert_eq!(output["input"], "");
    assert_eq!(
        output["dshHome"],
        format!("{}/.dsh", output["cwd"].as_str().unwrap())
    );
    assert_eq!(
        output["agentsHome"],
        format!("{}/.agents", output["cwd"].as_str().unwrap())
    );
    assert!(result.stderr.contains("fixture stderr"));
    assert!(!PathBuf::from(output["cwd"].as_str().unwrap()).exists());
}

#[tokio::test]
async fn reuses_a_caller_provided_cwd_and_leaves_it_in_place() {
    let cwd = tempfile::tempdir().unwrap();
    let mut options = options("shared cwd fixture", "unused-", "success");
    options.cwd = Some(cwd.path().to_path_buf());
    options.temp_dir_prefix = None;
    let result = run_loader_smoke(options).await.unwrap();
    assert_eq!(
        parse(&result.stdout)["cwd"],
        cwd.path().to_string_lossy().as_ref()
    );
    assert!(cwd.path().exists());
}

#[tokio::test]
async fn passes_arbitrary_argv_and_inspects_world_state_before_cleanup() {
    let inspected = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
    let marker = Arc::new(std::sync::Mutex::new(String::new()));
    let mut options = options("argv fixture", "loader-smoke-argv-", "success");
    options.bin_args = Some(vec![
        "--config".into(),
        CONFIG_PATH.into(),
        "--output-format".into(),
        "json".into(),
        "task with spaces".into(),
    ]);
    options.prepare = Some(Arc::new(
        |cwd: PathBuf| -> Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>> {
            Box::pin(async move {
                tokio::fs::write(cwd.join("marker.txt"), "prepared").await?;
                Ok(())
            })
        },
    ) as LoaderSmokeHook);
    let inspected_out = inspected.clone();
    let marker_out = marker.clone();
    options.inspect = Some(Arc::new(
        move |cwd: PathBuf| -> Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>> {
            let inspected = inspected_out.clone();
            let marker = marker_out.clone();
            Box::pin(async move {
                *inspected.lock().unwrap() = Some(cwd.clone());
                *marker.lock().unwrap() = tokio::fs::read_to_string(cwd.join("marker.txt")).await?;
                Ok(())
            })
        },
    ) as LoaderSmokeHook);
    let result = run_loader_smoke(options).await.unwrap();
    assert_eq!(
        parse(&result.stdout)["args"],
        serde_json::json!([
            "--config",
            CONFIG_PATH,
            "--output-format",
            "json",
            "task with spaces"
        ])
    );
    assert_eq!(&*marker.lock().unwrap(), "prepared");
    assert!(!inspected.lock().unwrap().as_ref().unwrap().exists());
}

#[tokio::test]
async fn rejects_a_nonzero_exit_with_captured_diagnostics() {
    let error = run_loader_smoke(options("failure fixture", "loader-smoke-fail-", "fail"))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "failure fixture exited 7 (expected 0). stdout:\n\nstderr:\nfixture failed\n"
    );
}

#[tokio::test]
async fn reports_a_spawn_failure_with_the_source_label_and_undefined_exit() {
    let mut missing = options("missing fixture", "loader-smoke-missing-", "success");
    missing.lib_bin_script = Some("/definitely/missing/loader-smoke-fixture".into());
    let error = run_loader_smoke(missing).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "missing fixture exited undefined (expected 0). stdout:\n\nstderr:\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn reports_a_signal_exit_as_undefined() {
    let error = run_loader_smoke(options("signal fixture", "loader-smoke-signal-", "signal"))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "signal fixture exited undefined (expected 0). stdout:\n\nstderr:\n"
    );
}

#[tokio::test]
async fn accepts_a_declared_failure_and_rejects_any_other_outcome() {
    let mut declared = options(
        "declared failure fixture",
        "loader-smoke-declared-fail-",
        "fail",
    );
    declared.expected_exit_code = Some(7);
    assert_eq!(
        run_loader_smoke(declared).await.unwrap().stderr,
        "fixture failed\n"
    );

    let mut clean = options(
        "unexpectedly clean fixture",
        "loader-smoke-clean-",
        "success",
    );
    clean.expected_exit_code = Some(7);
    assert!(
        run_loader_smoke(clean)
            .await
            .unwrap_err()
            .to_string()
            .contains("exited 0 (expected 7)")
    );
}

#[tokio::test]
async fn kills_a_process_at_its_deadline_and_reports_captured_output() {
    let mut options = options("hanging fixture", "loader-smoke-hang-", "hang");
    options.process_timeout_ms = Some(100);
    let error = run_loader_smoke(options).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "hanging fixture did not exit within 0.1s. stdout:\nfixture started\n\nstderr:\n"
    );
}

#[tokio::test]
async fn zero_process_timeout_disables_the_deadline() {
    let mut options = options("delayed fixture", "loader-smoke-delay-", "delay");
    options.process_timeout_ms = Some(0);
    let result = run_loader_smoke(options).await.unwrap();
    assert_eq!(result.stdout, "fixture delayed\n");
}

#[tokio::test(flavor = "current_thread")]
async fn owned_temp_cleanup_yields_the_async_runtime() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let mut options = options("cleanup fixture", "loader-smoke-cleanup-", "success");
    options.prepare = Some(Arc::new(
        |cwd: PathBuf| -> Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>> {
            Box::pin(async move {
                let tree = cwd.join("large-tree");
                tokio::fs::create_dir(&tree).await?;
                for index in 0..512 {
                    tokio::fs::write(tree.join(format!("{index}.txt")), b"fixture").await?;
                }
                Ok(())
            })
        },
    ) as LoaderSmokeHook);
    let ticks_out = ticks.clone();
    options.inspect = Some(Arc::new(
        move |_| -> Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>> {
            let ticks = ticks_out.clone();
            Box::pin(async move {
                drop(tokio::spawn(async move {
                    tokio::task::yield_now().await;
                    ticks.fetch_add(1, Ordering::SeqCst);
                }));
                Ok(())
            })
        },
    ) as LoaderSmokeHook);
    run_loader_smoke(options).await.unwrap();
    assert_eq!(ticks.load(Ordering::SeqCst), 1);
}
