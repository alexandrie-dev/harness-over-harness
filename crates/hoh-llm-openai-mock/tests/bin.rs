use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn binary_serves_the_shipped_scenario_and_writes_a_journal() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let journal = directory.path().join("journal.jsonl");
    let mut child = Child(
        Command::new(env!("CARGO_BIN_EXE_hoh-llm-openai-mock"))
            .args([
                "--scenario",
                concat!(env!("CARGO_MANIFEST_DIR"), "/scenarios/window.json"),
                "--journal",
            ])
            .arg(&journal)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn mock"),
    );
    let mut line = String::new();
    BufReader::new(child.0.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready line");
    let ready: Value = serde_json::from_str(&line).expect("ready JSON");
    assert_eq!(ready["type"], "ready");
    let base_url = ready["baseURL"].as_str().expect("baseURL");

    let text = reqwest::Client::new()
        .post(format!("{base_url}/chat/completions"))
        .header("content-type", "application/json")
        .body(json!({"model":"window-mock","stream":true,"messages":[]}).to_string())
        .send()
        .await
        .expect("completion")
        .text()
        .await
        .expect("stream body");
    assert!(text.contains("Это поток от mock."), "{text}");
    assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");

    let journal = std::fs::read_to_string(journal).expect("journal file");
    assert!(journal.contains("/v1/chat/completions"), "{journal}");
}

#[tokio::test]
async fn sigterm_after_ready_stops_the_binary_with_code_143() {
    let mut child = Child(
        Command::new(env!("CARGO_BIN_EXE_hoh-llm-openai-mock"))
            .args([
                "--scenario",
                concat!(env!("CARGO_MANIFEST_DIR"), "/scenarios/window.json"),
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn mock"),
    );
    let mut line = String::new();
    BufReader::new(child.0.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready line");
    assert!(line.contains("\"ready\""), "{line}");
    let sent = Command::new("kill")
        .args(["-TERM", &child.0.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    let status = child.0.wait().expect("wait for the mock");
    assert_eq!(status.code(), Some(143), "{status:?}");
}
