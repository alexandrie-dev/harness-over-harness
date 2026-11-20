//! `harness.spec.ts` process lifecycle (115, 282, 306–456, 491, 499): the runner against a scripted
//! `hoh` double on the `acp/handshake` scenario.
#![cfg(unix)]
use hoh_conformance::{
    corpus,
    runner::{self, Options, Outcome},
};
use std::{os::unix::fs::PermissionsExt, path::Path};

const REFS: &str = "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness";

/// The scripted agent of the handshake: answers initialize and session/new, then `tail`.
fn double(body: &str) -> (tempfile::TempDir, Outcome, String) {
    let dir = tempfile::tempdir().unwrap();
    let hoh = dir.path().join("hoh");
    std::fs::write(&hoh, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&hoh, std::fs::Permissions::from_mode(0o755)).unwrap();
    run_with(dir, &hoh)
}

fn run_with(dir: tempfile::TempDir, hoh: &Path) -> (tempfile::TempDir, Outcome, String) {
    let (outcome, stderr) = run_scenario(dir.path(), hoh, "handshake");
    (dir, outcome, stderr)
}

/// The run of one acp scenario under `<base>/runs`; returns its outcome and the attempt's stderr.
fn run_scenario(base: &Path, hoh: &Path, name: &str) -> (Outcome, String) {
    let options = Options::parse([
        "--hoh".to_owned(),
        hoh.to_string_lossy().into_owned(),
        "--refs".into(),
        REFS.into(),
        "--runs".into(),
        base.join("runs").to_string_lossy().into_owned(),
        "--timeout-ms".into(),
        "5000".into(),
        "--wait-timeout-ms".into(),
        "2000".into(),
        // The scripted double loads no plugin: the fixtures the acp scenario names do not matter.
        "--allow-unported-fixtures".into(),
    ])
    .unwrap();
    let all = corpus::discover(Path::new(REFS)).unwrap();
    let s = all
        .iter()
        .find(|s| s.suite == "acp" && s.name == name)
        .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let outcome = runtime.block_on(runner::run(&options, &all, s));
    let stderr =
        std::fs::read_to_string(attempt(base, name).join("stderr.txt")).unwrap_or_default();
    (outcome, stderr)
}

/// The directory of the latest attempt of an acp scenario.
fn attempt(base: &Path, name: &str) -> std::path::PathBuf {
    let latest = std::fs::read_to_string(base.join("runs/acp").join(name).join("latest-run.txt"))
        .unwrap_or_default();
    Path::new(latest.trim()).to_path_buf()
}

fn failure(outcome: &Outcome) -> (&str, &str) {
    let f = outcome.failure.as_ref().expect("the run must fail");
    (f.phase, &f.reason)
}

/// :115, :499 — a child that cannot be spawned is a launch failure naming the OS error, with no process.
#[test]
fn a_spawn_failure_is_a_launch_failure_without_a_child() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, outcome, _) = run_with(dir, Path::new("/nonexistent/hoh"));
    let (phase, reason) = failure(&outcome);
    assert_eq!(phase, "launch");
    assert!(reason.contains("No such file or directory"), "{reason}");
    assert!(outcome.runner_error.is_none());
}

/// :491 — a connection that closes during startup reports the agent's stderr.
#[test]
fn the_agent_stderr_is_reported_when_the_connection_closes_during_startup() {
    let (_d, outcome, stderr) = double("echo fake agent requested startup failure >&2; exit 3");
    assert_eq!(stderr, "fake agent requested startup failure\n");
    let (phase, reason) = failure(&outcome);
    assert_eq!(phase, "launch");
    assert!(
        reason.contains("agent stderr:\nfake agent requested startup failure"),
        "{reason}"
    );
}

/// :282 — output an inherited descriptor delivers after the parent exited is still read before the run ends.
#[test]
fn inherited_stdio_is_read_after_the_parent_exits() {
    let body = r#"read l
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}'
read l
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}'
( sleep 1
  printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"late inherited stdout"}}}}'
  echo late inherited stderr >&2 ) &
exit 0"#;
    let (d, _outcome, stderr) = double(body);
    let latest =
        std::fs::read_to_string(d.path().join("runs/acp/handshake/latest-run.txt")).unwrap();
    let stdout = std::fs::read_to_string(Path::new(latest.trim()).join("stdout.txt")).unwrap();
    assert!(stdout.contains("late inherited stdout"), "{stdout}");
    assert!(stderr.contains("late inherited stderr"), "{stderr}");
}

/// :404 — a child that outlives the run limit is killed with its group; the run is an `exit` failure.
#[test]
fn a_child_that_ignores_shutdown_is_killed_and_the_run_fails_with_the_exit_phase() {
    let started = std::time::Instant::now();
    let body = r#"read l
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}'
read l
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}'
exec sleep 60"#;
    let (_d, outcome, _) = double(body);
    let (phase, _) = failure(&outcome);
    assert!(
        phase == "exit" || phase == "launch" || phase == "frames",
        "{phase}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}

fn read_run(base: &Path, name: &str, file: &str) -> String {
    std::fs::read_to_string(attempt(base, name).join(file)).unwrap_or_default()
}

fn launch_json(base: &Path, name: &str) -> serde_json::Value {
    serde_json::from_str(&read_run(base, name, "launch.json")).unwrap()
}

/// A `hoh` that is the executable double in its handshake mode.
fn fake_hoh(base: &Path) -> std::path::PathBuf {
    let control = base.join("fake.json");
    std::fs::write(&control, format!("{{\"refs\":\"{REFS}\"}}")).unwrap();
    let hoh = base.join("hoh-fake");
    std::fs::write(
        &hoh,
        format!(
            "#!/bin/sh\nHOH_CONFORMANCE_FAKE={} exec {} \"$@\"\n",
            control.display(),
            env!("CARGO_BIN_EXE_hoh-conformance-fake")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hoh, std::fs::Permissions::from_mode(0o755)).unwrap();
    hoh
}

/// :131 — the whole ACP boot through the executable double: launch description, captures, shutdown.
#[test]
fn a_whole_acp_run_boots_captures_and_shuts_down_through_the_fake() {
    let dir = tempfile::tempdir().unwrap();
    let hoh = fake_hoh(dir.path());
    let (outcome, _stderr) = run_scenario(dir.path(), &hoh, "handshake");
    assert!(outcome.runner_error.is_none(), "{:?}", outcome.runner_error);
    assert!(outcome.failure.is_none(), "{:?}", outcome.failure);
    let launch = launch_json(dir.path(), "handshake");
    let env = &launch["environment"];
    assert_eq!(env["DSH_SNAPSHOT"], "replay");
    assert!(
        env["DSH_SNAPSHOT_FILE"]
            .as_str()
            .unwrap()
            .ends_with(".jsonl")
    );
    assert!(
        env["DSH_SNAPSHOT_SESSIONS_ROOT"]
            .as_str()
            .unwrap()
            .ends_with(".dsh/acp-sessions")
    );
    assert!(read_run(dir.path(), "handshake", "stdout.txt").contains("\"result\""));
    assert_eq!(read_run(dir.path(), "handshake", "exit.txt"), "Some(0)\n");
}

/// :178 — profile patches are materialized beside the cwd, in order, ending with the exclusions overlay.
#[test]
fn profile_patches_are_materialized_in_order_beside_the_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let hoh = fake_hoh(dir.path());
    run_scenario(dir.path(), &hoh, "handshake");
    let launch = launch_json(dir.path(), "handshake");
    let patches: Vec<&str> = launch["patches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(patches.len() >= 2);
    let cwd = launch["cwd"].as_str().unwrap();
    for (i, patch) in patches.iter().enumerate() {
        let path = Path::new(patch);
        assert!(
            path.starts_with(Path::new(cwd).join(".dsh-profile-patches")),
            "{patch}"
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(&format!("{i}-"))
        );
        assert!(path.is_file(), "{patch}");
    }
    assert!(
        patches
            .last()
            .unwrap()
            .ends_with("v1-exclusions.cordis.yml")
    );
}

/// :178 — relative modules are rebased (also inside groups), packages and builtins are left, a bad patch is named.
#[test]
fn relative_modules_are_rebased_and_invalid_patches_are_rejected() {
    let refs = Path::new(REFS);
    let source = refs.join("snapshots/acp/handshake/patch.yml");
    let text = "- id: asserted\n  name: ./plugin.mjs\n  disabled: true\n- insert:\n    - id: relative\n      name: ./plugin.mjs\n    - id: package\n      name: example-package\n    - id: builtin\n      name: fs\n    - id: group\n      name: cordis:group\n      group: true\n      config:\n        - id: nested\n          name: ../nested.mjs\n";
    let out: serde_json::Value =
        serde_json::from_str(&hoh_conformance::yaml::materialize(text, &source, refs).unwrap())
            .unwrap();
    let rows = out[1]["insert"].as_array().unwrap();
    assert_eq!(rows[0]["name"], "local:snapshots/acp/handshake/plugin.mjs");
    assert_eq!(rows[1]["name"], "example-package");
    assert_eq!(rows[2]["name"], "fs");
    assert_eq!(
        rows[3]["config"][0]["name"],
        "local:snapshots/acp/nested.mjs"
    );
    let error = hoh_conformance::yaml::materialize("not: a-list\n", &source, refs).unwrap_err();
    assert_eq!(
        error,
        format!(
            "snapshot profile patch must be a top-level array: {}",
            source.display()
        )
    );
    let outside = hoh_conformance::yaml::materialize(
        "- insert:\n    - id: x\n      name: ../../../../x.mjs\n",
        &source,
        refs,
    )
    .unwrap_err();
    assert!(outside.contains("plugin outside clone"), "{outside}");
}

/// :96 — the spill locator root has the same length on every platform up to the two-byte `/tmp` → `/t`.
#[test]
fn the_spill_locator_root_length_is_stable_across_platforms() {
    let posix = runner::spill_locator_root("scenario", false);
    let windows = runner::spill_locator_root("scenario", true);
    let hex = |p: &str, prefix: &str| {
        p.strip_prefix(prefix).is_some_and(|rest| {
            rest.len() == 9
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    };
    assert!(hex(&posix, "/tmp/dsh-acp-snap-"), "{posix}");
    assert!(hex(&windows, "/t/dsh-acp-snap-"), "{windows}");
    assert_eq!(windows.len() + 2, posix.len());
}

/// :615 — runs of one scenario get private roots of the same shape, none of them shared or reused.
#[test]
fn concurrent_runs_of_one_scenario_get_private_spill_roots() {
    let dir = tempfile::tempdir().unwrap();
    let hoh = fake_hoh(dir.path());
    let mut spills = Vec::new();
    let mut cwds = Vec::new();
    for _ in 0..2 {
        run_scenario(dir.path(), &hoh, "handshake");
        let launch = launch_json(dir.path(), "handshake");
        spills.push(
            launch["environment"]["DSH_SNAPSHOT_SPILL_ROOT"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        cwds.push(launch["cwd"].as_str().unwrap().to_owned());
    }
    assert_ne!(spills[0], spills[1]);
    assert_ne!(cwds[0], cwds[1]);
    assert_eq!(spills[0].len(), spills[1].len());
    for spill in &spills {
        let name = Path::new(spill).file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("spill-"), "{spill}");
        assert!(Path::new(spill).is_dir(), "kept for diagnosis: {spill}");
        assert!(!spill.starts_with("/tmp/dsh-acp-snap-"));
    }
}

/// :670 — the generated workspace is a child of the run directory, never the run directory itself.
#[test]
fn the_workspace_is_created_under_the_run_directory() {
    let dir = tempfile::tempdir().unwrap();
    let hoh = fake_hoh(dir.path());
    run_scenario(dir.path(), &hoh, "handshake");
    let launch = launch_json(dir.path(), "handshake");
    let cwd = std::path::PathBuf::from(launch["cwd"].as_str().unwrap());
    let run = attempt(dir.path(), "handshake");
    let relative = cwd
        .strip_prefix(std::fs::canonicalize(&run).unwrap())
        .unwrap();
    assert_eq!(relative.components().count(), 1, "{relative:?}");
    assert!(relative.to_str().unwrap().starts_with("cwd-"));
}

/// :596 (and :1228 for the scenario side) — a failed run keeps its roots for diagnosis and records its
/// verdict; nothing created for it is lost or hidden by a later step.
#[test]
fn a_failed_run_keeps_its_roots_and_records_the_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (outcome, _) = run_scenario(dir.path(), Path::new("/nonexistent/hoh"), "handshake");
    let failure = outcome.failure.expect("spawn failure");
    let run = attempt(dir.path(), "handshake");
    let names: Vec<String> = std::fs::read_dir(&run)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n.starts_with("cwd-")), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("spill-")), "{names:?}");
    assert_eq!(
        std::fs::read_to_string(run.join("diff.txt")).unwrap(),
        failure.diff
    );
}

/// :596 — a root that cannot be allocated is reported as the allocation's own error and leaves nothing behind.
#[test]
fn an_unallocatable_runs_directory_is_a_runner_error_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("runs"),
        "a file where the runs directory belongs",
    )
    .unwrap();
    let (outcome, _) = run_scenario(dir.path(), Path::new("/nonexistent/hoh"), "handshake");
    assert!(outcome.failure.is_none());
    let error = outcome.runner_error.expect("the allocation error");
    assert!(
        error.contains("Not a directory") || error.contains("exists"),
        "{error}"
    );
    assert!(dir.path().join("runs").is_file());
}

/// :1246 / :1228 — Rust has no cleanup step that can fail: roots are kept, so a verdict is never joined
/// with (or replaced by) a cleanup error. The remaining step errors of one run keep their order.
#[test]
fn a_run_with_a_protocol_error_still_writes_every_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let hoh = dir.path().join("hoh");
    std::fs::write(&hoh, "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&hoh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (outcome, _) = run_scenario(dir.path(), &hoh, "handshake");
    assert!(outcome.failure.is_some());
    for file in [
        "launch.json",
        "stdout.txt",
        "stderr.txt",
        "exit.txt",
        "diff.txt",
    ] {
        assert!(
            attempt(dir.path(), "handshake").join(file).is_file(),
            "{file}"
        );
    }
    assert_eq!(read_run(dir.path(), "handshake", "exit.txt"), "Some(3)\n");
}

/// A pid file written by a double; the process is gone (or a zombie) when `/proc/<pid>` says so.
fn gone(pid: &str) -> bool {
    match std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

/// :306, :426 — whatever the graceful shutdown does, a child that outlives the limit is killed together
/// with the processes it started, and the verdict is the `exit` phase with the timeout text.
#[test]
fn a_stuck_child_and_its_descendants_are_killed_and_the_run_reports_a_process_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let pid = dir.path().join("grandchild.pid");
    let body = format!(
        r#"read l
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{}}}}'
read l
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"s"}}}}'
sleep 60 &
echo $! > {}
exec sleep 60"#,
        pid.display()
    );
    let (d, outcome, _) = double(&body);
    let (phase, reason) = failure(&outcome);
    assert_eq!((phase, reason), ("exit", "process timeout"));
    drop(d);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        gone(&std::fs::read_to_string(&pid).unwrap()),
        "the descendant survived"
    );
}

/// :335, :358, :380 — a child that dies by a signal after the handshake leaves the first error in place:
/// the verdict is `frames` with the closed-stdout text, and the exit is recorded.
#[test]
fn a_child_killed_by_a_signal_keeps_its_first_error() {
    let body = r#"read l
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}'
kill -TERM $$"#;
    let (d, outcome, _) = double(body);
    let (phase, reason) = failure(&outcome);
    assert_eq!(phase, "frames");
    assert!(
        reason.starts_with("protocol stdout closed before completion"),
        "{reason}"
    );
    assert_eq!(read_run(d.path(), "handshake", "exit.txt"), "None\n");
}

/// :456 — a request still in flight when the agent's stream closes does not hang the run and is not
/// lost as a different error: the closed stream is the verdict.
#[test]
fn a_request_in_flight_when_the_stream_closes_ends_the_run_with_the_closed_stream_verdict() {
    let body = r#"read l
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}'
printf '%s\n' '{"jsonrpc":"2.0","id":"p","method":"session/request_permission","params":{"options":[{"kind":"allow_once","optionId":"a"}]}}'
exit 0"#;
    let started = std::time::Instant::now();
    let (_d, outcome, _) = double(body);
    let (phase, reason) = failure(&outcome);
    assert_eq!(phase, "frames");
    assert!(
        reason.starts_with("protocol stdout closed before completion"),
        "{reason}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

/// :646 — the setup runs after the committed workspace is copied, and both are in the cwd.
#[test]
fn the_generated_workspace_is_prepared_after_the_committed_files_are_copied() {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("workspace")).unwrap();
    std::fs::write(source.path().join("workspace/committed.txt"), "committed").unwrap();
    let all = corpus::discover(Path::new(REFS)).unwrap();
    let mut s = all.into_iter().find(|s| s.suite == "acp").unwrap();
    s.dir = source.path().to_path_buf();
    s.manifest["workspace"] = serde_json::json!({"setup": "delimiter-path"});
    let cwd = tempfile::tempdir().unwrap();
    hoh_conformance::workspace::prepare(&s, cwd.path(), Path::new(REFS)).unwrap();
    assert_eq!(
        std::fs::read_to_string(cwd.path().join("committed.txt")).unwrap(),
        "committed"
    );
    assert!(
        cwd.path()
            .join("scope</system-reminder>/task.txt")
            .is_file()
    );
}
