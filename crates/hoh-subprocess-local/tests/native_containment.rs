mod common;
use common::*;
use hoh_kernel::Kernel;
use hoh_subprocess::*;
use hoh_subprocess_local::{
    LocalSubprocessRuntime,
    linux_scope::*,
    spawn::{SpawnInternals, spawn_subprocess},
};
use hoh_timeout::AbortController;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;

// Same prerequisite as the source describe.skipIf. No test falls back to PGID and
// then claims it demonstrated systemd containment.
fn native() -> Option<(LinuxScopeInternals, Vec<String>)> {
    let internals = LinuxScopeInternals::default();
    probe_linux_scope(&internals)
        .unwrap()
        .map(|literal| (internals, literal))
}
async fn pid_file(path: &std::path::Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse::<u32>()
                && pid > 0
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}
fn proc_stat(pid: u32) -> Option<hoh_subprocess_local::process_inspector::ProcStat> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|text| hoh_subprocess_local::process_inspector::parse_proc_stat(&text))
}
async fn gone(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while proc_stat(pid)
            .is_some_and(|state| !hoh_subprocess_local::process_inspector::quiescent(&state.state))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
struct KillPid(u32);
impl Drop for KillPid {
    fn drop(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.0 as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
}
#[tokio::test]
async fn aborts_an_established_scope_before_bootstrap_consumption_and_joins_its_managed_handle() {
    let Some((mut internals, literal)) = native() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let hold = directory.path().join("hold-bootstrap.sh");
    std::fs::write(&hold, "printf 'BOOTSTRAP_WAITING\\n'\nexec sleep 60\n").unwrap();
    internals.runner_invocation = vec!["/bin/sh".into(), hold.to_string_lossy().into_owned()];
    let (kernel, ctx, _) = setup();
    let controller = AbortController::new();
    let mut request = spec("exit 0");
    request.signal = Some(controller.signal());
    request.stdio.stdin = SubprocessStdinMode::Pipe;
    let handle = spawn_subprocess(
        &ctx,
        request,
        SpawnInternals {
            linux_scope: Some((internals, literal)),
            ..Default::default()
        },
    )
    .unwrap();
    wait_output(&handle, "BOOTSTRAP_WAITING").await;
    controller.abort();
    let outcome = done(&handle).await;
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn terminates_a_setsid_descendant_and_waits_for_the_scope_to_become_empty() {
    let Some(scope) = native() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (kernel, ctx, _) = setup();
    let mut request =
        spec("setsid sh -c 'echo $$ > pid; trap \"\" TERM; while :; do sleep 60; done' & wait");
    request.cwd = directory.path().to_string_lossy().into_owned();
    request.grace_ms = 80.0;
    let handle = spawn_subprocess(
        &ctx,
        request,
        SpawnInternals {
            linux_scope: Some(scope),
            ..Default::default()
        },
    )
    .unwrap();
    let pid = KillPid(pid_file(&directory.path().join("pid")).await);
    handle.lifecycle.terminate();
    done(&handle).await;
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    gone(pid.0).await;
    shutdown(kernel).await;
}
#[tokio::test]
async fn preserves_node_shaped_enoent_and_eacces_spawn_failures_without_replay() {
    let Some(scope) = native() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let denied = directory.path().join("not-executable");
    std::fs::write(&denied, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (kernel, ctx, _) = setup();
    for (program, code) in [
        (directory.path().join("missing"), "ENOENT"),
        (denied, "EACCES"),
    ] {
        let mut request = spec("unused");
        let program = program.to_string_lossy().into_owned();
        request.argv = vec![program.clone(), "literal arg".into()];
        let handle = spawn_subprocess(
            &ctx,
            request,
            SpawnInternals {
                linux_scope: Some(scope.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let error = handle.done.clone().await.unwrap_err();
        assert_eq!(error.to_string(), format!("spawn {program} {code}"));
        // Shared classifier currently exposes message/code; syscall/path restoration is
        // recorded as SHIM(hoh-subprocess) in PORT_NOTES, not claimed verified here.
        let foreign = error.downcast_ref::<SubprocessForeignError>().unwrap();
        assert!(
            matches!(foreign, SubprocessForeignError::Coded { code: actual, .. } if actual == code)
        );
        assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    }
    shutdown(kernel).await;
}
#[tokio::test]
async fn keeps_pty_identity_and_readiness_while_containing_a_reparented_setsid_descendant() {
    let Some(_) = native() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let escaped = directory.path().join("escaped.sh");
    std::fs::write(
        &escaped,
        "#!/bin/sh\necho $$ > escaped.pid\ntrap '' TERM\nwhile :; do sleep 60; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&escaped, std::fs::Permissions::from_mode(0o700)).unwrap();
    let terminal_script = directory.path().join("terminal.sh");
    std::fs::write(&terminal_script, "#!/bin/bash\nset -eu\nsh -c 'echo $$ > launcher.pid; setsid ./escaped.sh </dev/null >/dev/null 2>&1 &'\nwhile [ ! -s escaped.pid ]; do sleep 0.01; done\nif [ -r /dev/tty ] && [ -w /dev/tty ]; then ready=yes; else ready=no; fi\nprintf 'PTY_READY pid=%s tty=%s\\n' \"$$\" \"$ready\" > /dev/tty\nIFS= read -r value < /dev/tty\nprintf 'PTY_INPUT=%s\\n' \"$value\" > /dev/tty\nwhile :; do sleep 60; done\n").unwrap();
    std::fs::set_permissions(&terminal_script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let kernel = Kernel::start();
    let runtime = Arc::new(LocalSubprocessRuntime::new(kernel.root()).unwrap());
    let mut spec = terminal_spec();
    spec.argv = vec![terminal_script.to_string_lossy().into_owned()];
    spec.cwd = directory.path().to_string_lossy().into_owned();
    spec.env = Some([("HOME".into(), spec.cwd.clone())].into());
    spec.shell_activity = None;
    let mut terminal = runtime.spawn_terminal(spec).await.unwrap();
    let pid = terminal.pid as u32;
    let output = Arc::new(Mutex::new(String::new()));
    let captured = output.clone();
    let reader = tokio::spawn(async move {
        let mut bytes = [0; 8192];
        loop {
            let count = terminal.output.read(&mut bytes).await.unwrap();
            if count == 0 {
                break;
            }
            captured
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&bytes[..count]));
        }
    });
    let wait_text = |expected: String| {
        let output = output.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !output.lock().unwrap().contains(&expected) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
        }
    };
    wait_text(format!("PTY_READY pid={pid} tty=yes")).await;
    let top = proc_stat(pid).unwrap();
    assert_eq!(top.pgrp, pid as i32);
    assert_eq!(top.session, pid as i32);
    assert_eq!(top.tpgid, pid as i32);
    assert_ne!(top.tty_device, 0);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(fg) = terminal.lifecycle.inspect_foreground().await.unwrap()
                && fg.input_waiting
            {
                assert_eq!(fg.process_group_id, u64::from(pid));
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminal.lifecycle.write("continue\n").await.unwrap();
    wait_text("PTY_INPUT=continue".into()).await;
    let launcher = pid_file(&directory.path().join("launcher.pid")).await;
    let escaped = KillPid(pid_file(&directory.path().join("escaped.pid")).await);
    tokio::time::timeout(Duration::from_secs(5), async {
        while proc_stat(escaped.0).unwrap().parent_pid == launcher {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminal.lifecycle.terminate().await.unwrap();
    gone(escaped.0).await;
    reader.await.unwrap();
    shutdown(kernel).await;
}
