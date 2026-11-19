use hoh_subprocess_local::{runner_launch::SUBPROCESS_RUNNER_ENV, runner_protocol::*};
use rustix::{
    fd::OwnedFd,
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
};
use std::{
    io::Read,
    os::unix::net::UnixStream,
    process::{Command, Stdio},
};

struct Files(LinuxLaunchFiles);
impl Drop for Files {
    fn drop(&mut self) {
        cleanup_linux_launch_files(&self.0);
    }
}
fn files(pipe_fds: Option<Vec<u64>>) -> Files {
    Files(
        create_linux_launch_files(&LinuxLaunchRequest {
            cwd: "/tmp".into(),
            env: Default::default(),
            pipe_fds,
            control: None,
            control_path: None,
        })
        .unwrap(),
    )
}
#[test]
fn passes_blocking_standard_streams_to_a_real_native_executable_after_initialization() {
    let files = files(Some(vec![0, 1, 2]));
    let (input, child_input) = UnixStream::pair().unwrap();
    let (mut output, child_output) = UnixStream::pair().unwrap();
    for stream in [&child_input, &child_output] {
        fcntl_setfl(stream, fcntl_getfl(stream).unwrap() | OFlags::NONBLOCK).unwrap();
    }
    let child_input: OwnedFd = child_input.into();
    let child_output: OwnedFd = child_output.into();
    let status = Command::new(env!("CARGO_BIN_EXE_hoh-subprocess-runner"))
        .env_clear()
        .env(SUBPROCESS_RUNNER_ENV, &files.0.request_path)
        .args([
            "--",
            "/bin/cat",
            "/proc/self/fdinfo/0",
            "/proc/self/fdinfo/1",
            "/proc/self/fdinfo/2",
        ])
        .stdin(Stdio::from(child_input))
        .stdout(Stdio::from(child_output))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(status.status.success(), "{:?}", status.stderr);
    drop(input);
    let mut text = String::new();
    output.read_to_string(&mut text).unwrap();
    let flags = text
        .lines()
        .filter_map(|line| line.strip_prefix("flags:\t"))
        .map(|value| u32::from_str_radix(value, 8).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(flags.len(), 3);
    assert_eq!(
        flags.iter().map(|flags| flags & 0x800).collect::<Vec<_>>(),
        [0, 0, 0]
    );
}
#[test]
fn preserves_nonblocking_flags_shared_with_the_parent_on_inherited_stdout() {
    let files = files(None);
    let (_reader, writer) = UnixStream::pair().unwrap();
    fcntl_setfl(&writer, fcntl_getfl(&writer).unwrap() | OFlags::NONBLOCK).unwrap();
    let child_output: OwnedFd = writer.try_clone().unwrap().into();
    let output = Command::new(env!("CARGO_BIN_EXE_hoh-subprocess-runner"))
        .env_clear()
        .env(SUBPROCESS_RUNNER_ENV, &files.0.request_path)
        .args(["--", "/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(child_output))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(fcntl_getfl(&writer).unwrap().contains(OFlags::NONBLOCK));
}
