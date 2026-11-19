mod common;
use common::*;
use hoh_subprocess::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;
async fn until(output: &Arc<Mutex<String>>, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !output.lock().unwrap().contains(expected) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("missing {expected:?} in {:?}", output.lock().unwrap()));
}
#[tokio::test]
async fn terminates_and_joins_an_owned_terminal_during_disposal() {
    let (kernel, _, runtime) = setup();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".bashrc"), "PS1='READY> '\n").unwrap();
    let mut spec = terminal_spec();
    spec.cwd = home.path().to_string_lossy().into_owned();
    spec.env = Some([("HOME".into(), spec.cwd.clone())].into());
    let mut terminal = runtime.spawn_terminal(spec).await.unwrap();
    let output = Arc::new(Mutex::new(String::new()));
    let recorded = output.clone();
    let reader = tokio::spawn(async move {
        let mut bytes = [0; 4096];
        loop {
            let count = terminal.output.read(&mut bytes).await.unwrap();
            if count == 0 {
                break;
            }
            recorded
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&bytes[..count]));
        }
    });
    until(&output, "READY>").await;
    shutdown(kernel).await;
    tokio::time::timeout(Duration::from_secs(5), reader)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn preserves_scalar_prompt_hooks_and_reports_shell_activity() {
    let (kernel, _, runtime) = setup();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".bashrc"),
        "PS1='READY> '\nPROMPT_COMMAND='printf HOOK; # user comment'\nset -C\n",
    )
    .unwrap();
    let mut spec = terminal_spec();
    spec.cwd = home.path().to_string_lossy().into_owned();
    spec.env = Some([("HOME".into(), spec.cwd.clone())].into());
    let terminal = runtime.spawn_terminal(spec).await.unwrap();
    let output = Arc::new(Mutex::new(String::new()));
    let recorded = output.clone();
    let mut stream = terminal.output;
    let reader = tokio::spawn(async move {
        let mut bytes = [0; 4096];
        loop {
            let count = stream.read(&mut bytes).await.unwrap();
            if count == 0 {
                break;
            }
            recorded
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&bytes[..count]));
        }
    });
    until(&output, "READY>").await;
    assert!(output.lock().unwrap().contains("HOOK"));
    tokio::time::timeout(Duration::from_secs(5), async {
        while terminal.lifecycle.inspect_activity().await.unwrap().state
            != SubprocessTerminalActivityState::Idle
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminal.lifecycle.write("sleep 60\r").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while terminal.lifecycle.inspect_activity().await.unwrap().state
            != SubprocessTerminalActivityState::Busy
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminal.lifecycle.write("\u{3}").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while terminal.lifecycle.inspect_activity().await.unwrap().state
            != SubprocessTerminalActivityState::Idle
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminal.lifecycle.terminate().await.unwrap();
    assert_eq!(
        terminal.lifecycle.inspect_activity().await.unwrap().state,
        SubprocessTerminalActivityState::Idle
    );
    tokio::time::timeout(Duration::from_secs(5), reader)
        .await
        .unwrap()
        .unwrap();
    shutdown(kernel).await;
}

#[tokio::test]
async fn unloading_joins_and_cleans_an_abandoned_pty_allocation() {
    use futures_util::FutureExt;
    let (kernel, _, runtime) = setup();
    let home = tempfile::tempdir().unwrap();
    let pid_file = home.path().join("pid");
    let mut spec = terminal_spec();
    spec.cwd = home.path().to_string_lossy().into_owned();
    spec.env = Some([("HOME".into(), spec.cwd.clone())].into());
    spec.shell_activity = None;
    spec.argv = vec![
        "/bin/sh".into(),
        "-c".into(),
        "echo $$ > pid; exec sleep 60".into(),
    ];
    // Poll once to start the blocking allocation, then abandon the caller's future.
    if let Some(result) = runtime.spawn_terminal(spec).now_or_never() {
        result.unwrap().lifecycle.terminate().await.unwrap();
    }
    shutdown(kernel).await;
    if let Ok(pid) = std::fs::read_to_string(pid_file) {
        let pid: u32 = pid.trim().parse().unwrap();
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|text| hoh_subprocess_local::process_inspector::parse_proc_stat(&text));
        assert!(
            state.is_none_or(|state| hoh_subprocess_local::process_inspector::quiescent(
                &state.state
            ))
        );
    }
}
