//! The window profile booted in this process over supplied streams: what `provide_stdio` and
//! `run_profile` wire together, without a child process.
mod support;
use hoh::profile_boot::*;
use hoh_kernel::Undo;
use hoh_launch_environment::create_launch_environment_snapshot;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn write_profile(root: &Path) {
    let home = root.join("home");
    let dir = home.join("profiles").join("window");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    std::fs::write(
        dir.join("package.json"),
        json!({"name":"dsh-profile-window","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@hoh/window"]}}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("cordis.patch.yml"),
        "- id: model-host\n  config:\n    baseUrl: http://127.0.0.1:9/v1\n    model: window-mock\n",
    )
    .unwrap();
}

// After a successful `shutdown` the server raises the host's exit flag *before* it disposes the
// kernel root, so a boot that is still finishing can tell the disposal is the requested exit. The
// flag comes from `provide_stdio` through the server's `with_exiting` hook; passing `|| {}` for it
// leaves the flag down at the moment the root goes away.
#[tokio::test]
async fn the_shutdown_response_raises_the_host_exit_flag_before_the_root_is_disposed() {
    let root = tempfile::tempdir().unwrap();
    write_profile(root.path());
    let env = support::env(root.path());
    let home = root.path().join("home");
    let profile = prepare_profile(
        "window",
        false,
        None,
        &home,
        &ShippedBundles,
        &env,
        &mut |warning| panic!("{warning}"),
    )
    .unwrap();

    let (mut client_in, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, client_out) = tokio::io::duplex(64 * 1024);
    let completed = Arc::new(Mutex::new(Vec::new()));
    let observed = completed.clone();
    let host = ProfileHost::new(
        Arc::new(|code| panic!("unexpected forced exit {code}")),
        Arc::new(move |code| observed.lock().unwrap().push(code)),
    )
    .with_window_stdio(WindowStdio {
        input: Box::new(server_in),
        output: Box::new(server_out),
    });
    let flag_at_disposal = Arc::new(Mutex::new(None));
    let seen = flag_at_disposal.clone();
    let probe = host.clone();
    host.kernel
        .root()
        .effect("test.root-disposed", move || {
            Undo::sync(move || *seen.lock().unwrap() = Some(probe.exit_requested()))
        })
        .unwrap();

    let running = run_profile(
        RunProfileOptions {
            profile,
            patch_files: vec![],
            home,
            bundles: Arc::new(ShippedBundles),
            environment: create_launch_environment_snapshot(&[]),
            env,
            args: vec![],
            registry: Arc::new(hoh::registry::registry()),
            install_root: None,
        },
        host.clone(),
    )
    .await
    .expect("the window profile boots over supplied streams");
    assert!(running.loader.is_some());

    let mut frames = BufReader::new(client_out).lines();
    for frame in [
        json!({"jsonrpc":"2.0","id":"initialize","method":"initialize",
            "params":{"cwd":root.path(),"provider":"openai-compatible","model":"window-mock"}}),
        json!({"jsonrpc":"2.0","id":"shutdown","method":"shutdown","params":{}}),
    ] {
        client_in
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
    }
    let mut answered = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        while answered.len() < 2 {
            let line = frames
                .next_line()
                .await
                .unwrap()
                .expect("engine closed early");
            if let Ok(frame) = serde_json::from_str::<Value>(&line)
                && frame["id"].is_string()
            {
                answered.push(frame);
            }
        }
    })
    .await
    .expect("initialize and shutdown were not answered");
    assert!(
        answered.iter().all(|frame| frame.get("result").is_some()),
        "{answered:?}"
    );

    tokio::time::timeout(Duration::from_secs(30), async {
        while completed.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the engine did not report its exit");
    assert_eq!(*completed.lock().unwrap(), [0]);
    assert_eq!(
        *flag_at_disposal.lock().unwrap(),
        Some(true),
        "the exit flag was not raised before the root was disposed"
    );
}
