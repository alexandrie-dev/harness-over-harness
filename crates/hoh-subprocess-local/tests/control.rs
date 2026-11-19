mod common;
use common::*;
use hoh_subprocess::*;
use hoh_subprocess_local::spawn::{SpawnInternals, spawn_subprocess};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[tokio::test]
async fn leaves_the_channel_absent_on_an_ordinary_spawn() {
    let (kernel, _, runtime) = setup();
    let handle = runtime.spawn(spec("printf plain")).unwrap();
    assert!(handle.control.is_none());
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert_eq!(out(&handle).text, "plain");
    assert!(handle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn returns_exact_binary_control_bytes_through_fallback_independently_of_stdio() {
    binary_control(false).await;
}
#[tokio::test]
async fn returns_exact_binary_control_bytes_through_managed_independently_of_stdio() {
    binary_control(true).await;
}
async fn binary_control(managed: bool) {
    let (kernel, ctx, runtime) = setup();
    let mut spec = spec(
        "head -c 262144 <&7 >&7; printf 'ordinary stdout\\n'; printf 'ordinary stderr\\n' >&2",
    );
    spec.stdio.control = Some(SubprocessControlMode::Pipe);
    let mut handle = if managed {
        runtime.spawn(spec).unwrap()
    } else {
        spawn_subprocess(&ctx, spec, SpawnInternals::default())
            .unwrap()
            .handle
    };
    let channel = handle.control.take().unwrap();
    let input = (0..256 * 1024).map(|n| (n % 256) as u8).collect::<Vec<_>>();
    let (mut read, mut write) = tokio::io::split(channel);
    let expected = input.clone();
    let (received, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            async {
                let mut received = vec![];
                read.read_to_end(&mut received).await.unwrap();
                received
            },
            async {
                write.write_all(&input).await.unwrap();
                write.shutdown().await.unwrap();
            }
        )
    })
    .await
    .unwrap();
    assert_eq!(received, expected);
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert_eq!(out(&handle).text, "ordinary stdout\n");
    assert_eq!(
        handle.collected.stderr.as_ref().unwrap().read_from(0).text,
        "ordinary stderr\n"
    );
    assert!(handle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn closes_the_caller_endpoint_when_service_disposal_terminates_an_active_program() {
    let (kernel, _, runtime) = setup();
    let mut spec = spec("printf ready >&7; exec sleep 60");
    spec.stdio.control = Some(SubprocessControlMode::Pipe);
    let mut handle = runtime.spawn(spec).unwrap();
    let mut channel = handle.control.take().unwrap();
    let mut ready = [0; 5];
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        channel.read_exact(&mut ready),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&ready, b"ready");
    shutdown(kernel).await;
    let mut rest = vec![];
    channel.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
    assert!(handle.wait_for_exit(None).await.unwrap());
}
#[tokio::test]
async fn joins_an_exited_range_and_disposes_its_paused_control_endpoint_without_draining_it() {
    let (kernel, _, runtime) = setup();
    let mut spec = spec("head -c 4096 /dev/zero >&7");
    spec.stdio.control = Some(SubprocessControlMode::Pipe);
    let mut handle = runtime.spawn(spec).unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert!(handle.control.is_some());
    assert!(handle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
    let mut pending = vec![];
    handle
        .control
        .take()
        .unwrap()
        .read_to_end(&mut pending)
        .await
        .unwrap();
    assert!(
        pending.is_empty(),
        "disposal discards an unread control buffer"
    );
}
#[tokio::test]
async fn rejects_a_caller_authored_control_marker_before_starting_a_child() {
    let (kernel, _, runtime) = setup();
    let mut spec = spec("echo must-not-execute");
    spec.env = Some([("DSH_SUBPROCESS_CONTROL".into(), Some("pipe".into()))].into());
    assert_eq!(
        runtime.spawn(spec).err().unwrap().to_string(),
        "DSH_SUBPROCESS_CONTROL is reserved for subprocess control-channel setup"
    );
    shutdown(kernel).await;
}

fn open_socket_count() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|target| target.to_string_lossy().starts_with("socket:["))
        .count()
}

#[tokio::test]
async fn control_startup_failure_fd_fixture() {
    if std::env::var_os("HOH_CONTROL_STARTUP_FAILURE_FD_FIXTURE").is_none() {
        return;
    }
    let (kernel, ctx, _) = setup();
    let baseline = open_socket_count();
    for index in 0..50 {
        let mut request = spec("true");
        request.cwd = format!(
            "/nonexistent-hoh-control-cwd-{}-{index}",
            std::process::id()
        );
        request.stdio.control = Some(SubprocessControlMode::Pipe);
        let handle = spawn_subprocess(&ctx, request, SpawnInternals::default()).unwrap();
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), handle.done.clone())
            .await
            .expect("startup rejection deadline")
            .unwrap_err();
        assert!(error.to_string().contains("ENOENT"), "{error}");
        drop(handle);
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while open_socket_count() != baseline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("control listener descriptors leaked after rejected done");
    assert_eq!(open_socket_count(), baseline);
    shutdown(kernel).await;
}

#[tokio::test]
async fn releases_control_listeners_when_startup_fails_before_the_helper_connects() {
    // The integration-test harness runs cases concurrently. Count descriptors in
    // a one-test child so sockets owned by neighboring cases cannot enter or
    // leave the baseline while this regression is polling it.
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "control_startup_failure_fd_fixture",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("HOH_CONTROL_STARTUP_FAILURE_FD_FIXTURE", "1")
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .expect("isolated control descriptor regression deadline")
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn long_tmpdir_control_fixture() {
    if std::env::var_os("HOH_LONG_TMPDIR_CONTROL_FIXTURE").is_none() {
        return;
    }
    assert_eq!(
        std::env::var_os("TMPDIR").unwrap().as_encoded_bytes().len(),
        100
    );
    let (kernel, ctx, _) = setup();
    let mut request = spec("printf long-path-ok >&7");
    request.stdio.control = Some(SubprocessControlMode::Pipe);
    let mut handle = spawn_subprocess(&ctx, request, SpawnInternals::default())
        .unwrap()
        .handle;
    let mut control = handle.control.take().unwrap();
    let mut output = String::new();
    control.read_to_string(&mut output).await.unwrap();
    assert_eq!(output, "long-path-ok");
    assert_eq!(done(&handle).await.exit_code, Some(0));
    shutdown(kernel).await;
}

#[tokio::test]
async fn keeps_the_control_socket_path_short_when_tmpdir_is_one_hundred_bytes() {
    let mut name = format!("hoh-long-tmpdir-{}-", std::process::id());
    name.push_str(&"x".repeat(95 - name.len()));
    let directory = std::path::PathBuf::from(format!("/tmp/{name}"));
    assert_eq!(directory.as_os_str().as_encoded_bytes().len(), 100);
    std::fs::create_dir(&directory).unwrap();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "long_tmpdir_control_fixture", "--nocapture"])
        .env("HOH_LONG_TMPDIR_CONTROL_FIXTURE", "1")
        .env("TMPDIR", &directory)
        .output()
        .await
        .unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn occupied_fd_seven_control_fixture() {
    if std::env::var_os("HOH_OCCUPIED_FD7_CONTROL_FIXTURE").is_none() {
        return;
    }
    assert!(std::fs::metadata("/proc/self/fd/7").is_ok());
    let (kernel, ctx, _) = setup();
    let mut request = spec("printf replaced-fd7 >&7");
    request.stdio.control = Some(SubprocessControlMode::Pipe);
    let mut handle = spawn_subprocess(&ctx, request, SpawnInternals::default())
        .unwrap()
        .handle;
    let mut control = handle.control.take().unwrap();
    let mut output = String::new();
    control.read_to_string(&mut output).await.unwrap();
    assert_eq!(output, "replaced-fd7");
    assert_eq!(done(&handle).await.exit_code, Some(0));
    shutdown(kernel).await;
}

#[tokio::test]
async fn helper_atomically_replaces_an_inherited_open_fd_seven() {
    let output = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            "exec 7</dev/null; exec \"$1\" --exact occupied_fd_seven_control_fixture --nocapture",
            "hoh-fd7-wrapper",
        ])
        .arg(std::env::current_exe().unwrap())
        .env("HOH_OCCUPIED_FD7_CONTROL_FIXTURE", "1")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
