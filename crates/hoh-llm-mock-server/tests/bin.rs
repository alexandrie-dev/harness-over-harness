#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};

use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn standalone_reports_lifecycle_events_and_exits_143_on_sigterm() {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
        Err(error) => panic!("reserve port: {error}"),
    };
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    let child = Command::new(env!("CARGO_BIN_EXE_dsh-llm-mock-server"))
        .args([
            "--sequence",
            "connection_refused,success",
            "--listen-delay-ms",
            "5",
            "--port",
            &port.to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn binary");
    let mut child = ChildGuard(child);
    let stdout = child.0.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let unavailable: Value =
        serde_json::from_str(&lines.next().expect("unavailable line").expect("read"))
            .expect("unavailable JSON");
    assert_eq!(unavailable["type"], "unavailable");
    assert_eq!(unavailable["listenDelayMs"], 5);
    let ready: Value = serde_json::from_str(&lines.next().expect("ready line").expect("read"))
        .expect("ready JSON");
    assert_eq!(ready["type"], "ready");

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .body("{}")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    let _ = response.text().await.expect("response body");
    let request: Value = serde_json::from_str(&lines.next().expect("request line").expect("read"))
        .expect("request JSON");
    let result: Value = serde_json::from_str(&lines.next().expect("result line").expect("read"))
        .expect("result JSON");
    assert_eq!(request["type"], "request");
    assert_eq!(result["type"], "result");

    let pid = Pid::from_raw(child.0.id() as i32).expect("positive pid");
    kill_process(pid, Signal::TERM).expect("signal child");
    let status = child.0.wait().expect("wait");
    assert_eq!(status.code(), Some(143));
}

#[test]
fn unavailable_event_keeps_javascript_port_before_server_validation() {
    let output = Command::new(env!("CARGO_BIN_EXE_dsh-llm-mock-server"))
        .args([
            "--sequence",
            "connection_refused,success",
            "--listen-delay-ms",
            "0",
            "--port",
            "65536",
        ])
        .output()
        .expect("run binary");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("stdout UTF-8");
    let first: Value = serde_json::from_str(stdout.lines().next().expect("unavailable line"))
        .expect("unavailable JSON");
    assert_eq!(first["baseURL"], "http://127.0.0.1:65536/v1");
    let stderr = String::from_utf8(output.stderr).expect("stderr UTF-8");
    assert!(stderr.contains("port must be an integer between 0 and 65535"));
}
