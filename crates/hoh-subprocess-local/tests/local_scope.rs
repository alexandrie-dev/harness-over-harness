mod common;
use common::*;
use futures_util::FutureExt;
use hoh_kernel::Kernel;
use hoh_subprocess::*;
use hoh_subprocess_local::{
    LocalSubprocessRuntime, linux_scope::*, linux_scope_selector::LinuxScopeObservations,
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Observations(Arc<AtomicUsize>);
impl LinuxScopeObservations for Observations {
    fn probe(&self) -> Result<Option<Vec<String>>, hoh_error::BoxError> {
        self.0.fetch_add(1, Ordering::AcqRel);
        Ok(Some(vec!["--expand-environment=no".into()]))
    }
    fn manager_listener(&self) -> Option<String> {
        None
    }
    fn confirm_manager(&self) -> bool {
        true
    }
    fn launcher_identity(&self) -> Option<String> {
        Some("same-launcher".into())
    }
}
fn fixture(body: &str) -> (tempfile::TempDir, LinuxScopeInternals) {
    let dir = tempfile::tempdir().unwrap();
    let launcher = dir.path().join("systemd-run");
    std::fs::write(&launcher, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    let internals = LinuxScopeInternals {
        systemd_run: launcher.to_string_lossy().into_owned(),
        run_sync: Arc::new(|_, _, _| SystemctlResult {
            status: Some(0),
            stdout: "LoadState=loaded\nActiveState=inactive\n".into(),
            stderr: String::new(),
            error: None,
        }),
        systemctl_query: Arc::new(|_, _| {
            async {
                SystemctlResult {
                    status: Some(0),
                    stdout: "LoadState=loaded\nActiveState=inactive\n".into(),
                    stderr: String::new(),
                    error: None,
                }
            }
            .boxed()
        }),
        ..Default::default()
    };
    (dir, internals)
}

#[tokio::test]
async fn keeps_a_forked_native_pty_target_in_the_launchers_session_and_process_group() {
    let (directory, internals) = fixture("");
    // Use an absolute scratch file: systemd-run's cwd is the caller's cwd.
    let launcher = directory.path().join("systemd-run");
    let leader = directory.path().join("leader");
    std::fs::write(&launcher, format!(
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > '{}'\nwhile [ \"$1\" != -- ]; do shift; done\nshift\n\"$@\"\nstatus=$?\nexit \"$status\"\n",
        leader.display(),
    )).unwrap();
    let (kernel, ctx, _) = setup();
    let mut request = terminal_spec();
    request.argv = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%s\\n' \"$$\" > target; /bin/ps -o pgid= -o sid= -p \"$$\" >> target".into(),
    ];
    request.cwd = directory.path().to_string_lossy().into_owned();
    request.env = Some([("HOME".into(), request.cwd.clone())].into());
    request.shell_activity = Some(false);
    let (handle, _) = hoh_subprocess_local::terminal::spawn_terminal(
        &ctx,
        request,
        hoh_subprocess_local::process_inspector::create_process_inspector(),
        Some((internals, vec![])),
    )
    .await
    .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle.done.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.exit_code, Some(0));
    handle.terminate().await.unwrap();
    let leader = std::fs::read_to_string(leader)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let target = std::fs::read_to_string(directory.path().join("target"))
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(target.len(), 3);
    assert_ne!(target[0], leader);
    assert_eq!(&target[1..], [leader; 2]);
    shutdown(kernel).await;
}
#[tokio::test]
async fn routes_linux_scope_decisions_into_launches_and_probes_again_after_a_scope_never_starts() {
    let (_directory, internals) = fixture("exit 127");
    let kernel = Kernel::start();
    let probes = Arc::new(AtomicUsize::new(0));
    let runtime =
        LocalSubprocessRuntime::new_with_observations(kernel.root(), Observations(probes.clone()))
            .unwrap();
    runtime.internals.lock().unwrap().linux_scope = Some((internals, vec![]));
    let mut handles = vec![];
    for expected in 1..=2 {
        let handle = runtime.spawn(spec("echo must-not-run")).unwrap();
        assert_eq!(probes.load(Ordering::Acquire), expected);
        let error = handle.done.clone().await.unwrap_err();
        assert!(error.get_ref().is::<LinuxScopeNotStartedError>());
        assert!(out(&handle).text.is_empty());
        // No yield here: invalidation must precede publication of done to the caller.
        handles.push(handle);
    }
    for handle in handles {
        assert!(handle.wait_for_exit(None).await.unwrap());
    }
    shutdown(kernel).await;
}
#[tokio::test]
async fn wraps_linux_terminals_in_the_selected_scope_and_binds_owner_liveness() {
    let (directory, internals) =
        fixture("while [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"");
    let kernel = Kernel::start();
    let probes = Arc::new(AtomicUsize::new(0));
    let runtime =
        LocalSubprocessRuntime::new_with_observations(kernel.root(), Observations(probes.clone()))
            .unwrap();
    runtime.internals.lock().unwrap().linux_scope = Some((internals, vec![]));
    let mut request = terminal_spec();
    request.argv = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf 'native-pty'; exit 7".into(),
    ];
    request.cwd = directory.path().to_string_lossy().into_owned();
    request.env = Some([("HOME".into(), request.cwd.clone())].into());
    request.shell_activity = None;
    let mut handle = runtime.spawn_terminal(request).await.unwrap();
    let result = handle.done.clone().await.unwrap();
    assert_eq!(result.exit_code, Some(7));
    let mut output = String::new();
    use tokio::io::AsyncReadExt;
    handle.output.read_to_string(&mut output).await.unwrap();
    assert_eq!(output, "native-pty");
    handle.lifecycle.terminate().await.unwrap();
    assert_eq!(probes.load(Ordering::Acquire), 1);
    shutdown(kernel).await;
}
#[tokio::test]
async fn probes_linux_again_after_a_terminal_scope_never_starts_its_target() {
    let (directory, internals) = fixture("exit 127");
    let kernel = Kernel::start();
    let probes = Arc::new(AtomicUsize::new(0));
    let runtime =
        LocalSubprocessRuntime::new_with_observations(kernel.root(), Observations(probes.clone()))
            .unwrap();
    runtime.internals.lock().unwrap().linux_scope = Some((internals, vec![]));
    for expected in 1..=2 {
        let mut request = terminal_spec();
        request.argv = vec!["/bin/sh".into(), "-c".into(), "echo must-not-run".into()];
        request.cwd = directory.path().to_string_lossy().into_owned();
        request.env = Some([("HOME".into(), request.cwd.clone())].into());
        request.shell_activity = None;
        let handle = runtime.spawn_terminal(request).await.unwrap();
        assert_eq!(probes.load(Ordering::Acquire), expected);
        assert!(
            handle
                .done
                .clone()
                .await
                .unwrap_err()
                .get_ref()
                .is::<LinuxScopeNotStartedError>()
        );
        handle.lifecycle.terminate().await.unwrap();
    }
    shutdown(kernel).await;
}
