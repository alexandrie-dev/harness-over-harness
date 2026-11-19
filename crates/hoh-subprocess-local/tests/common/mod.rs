#![allow(dead_code)]
use hoh_kernel::{Context, Kernel};
use hoh_subprocess::*;
use hoh_subprocess_local::{LocalSubprocessRuntime, linux_scope_selector::LinuxScopeObservations};
use std::{sync::Arc, time::Duration};
pub struct NoManager;
impl LinuxScopeObservations for NoManager {
    fn probe(&self) -> Result<Option<Vec<String>>, hoh_error::BoxError> {
        Ok(None)
    }
    fn manager_listener(&self) -> Option<String> {
        None
    }
    fn confirm_manager(&self) -> bool {
        false
    }
    fn launcher_identity(&self) -> Option<String> {
        None
    }
}
pub fn setup() -> (Kernel, Context, Arc<LocalSubprocessRuntime>) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let runtime =
        Arc::new(LocalSubprocessRuntime::new_with_observations(ctx.clone(), NoManager).unwrap());
    (kernel, ctx, runtime)
}
pub fn spec(command: &str) -> SubprocessSpawnSpec {
    SubprocessSpawnSpec {
        argv: vec!["/bin/bash".into(), "-c".into(), command.into()],
        cwd: "/tmp".into(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: collect(64000),
            stderr: collect(64000),
            control: None,
        },
        grace_ms: 100.0,
        signal: None,
        env: None,
    }
}
pub fn collect(bytes: u64) -> SubprocessOutputMode {
    SubprocessOutputMode::Collect(SubprocessCollect {
        max_bytes: bytes,
        spill: Some(SubprocessSpill {
            max_bytes: 64 * 1024 * 1024,
        }),
    })
}
pub fn terminal_spec() -> SubprocessTerminalSpawnSpec {
    SubprocessTerminalSpawnSpec {
        argv: vec!["/bin/bash".into(), "-i".into()],
        cwd: "/tmp".into(),
        env: None,
        rows: 24,
        cols: 80,
        terminal_type: "xterm-256color".into(),
        shell_activity: Some(true),
        grace_ms: 100.0,
        signal: None,
    }
}
pub async fn done(handle: &SubprocessHandle) -> SubprocessOutcome {
    tokio::time::timeout(Duration::from_secs(5), handle.done.clone())
        .await
        .expect("direct result deadline")
        .expect("direct result")
}
pub fn out(handle: &SubprocessHandle) -> SubprocessOutputRead {
    handle.collected.stdout.as_ref().unwrap().read_from(0)
}
pub async fn wait_output(handle: &SubprocessHandle, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !out(handle).text.contains(expected) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("output deadline");
}
pub async fn shutdown(kernel: Kernel) {
    tokio::time::timeout(Duration::from_secs(5), kernel.shutdown())
        .await
        .expect("shutdown deadline");
}
