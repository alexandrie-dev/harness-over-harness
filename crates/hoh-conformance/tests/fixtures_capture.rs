use futures_util::StreamExt;
use hoh_conformance::fixtures::capture::CaptureRequests;
use hoh_kernel::Kernel;
use hoh_llm::{GenerateOptions, Llm, LlmPlugin};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_model_request_appends_one_json_line_before_the_adapter_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compact-requests.jsonl");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<LlmPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<CaptureRequests>(json!({"path": path}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let llm = ctx.get::<Llm>().unwrap();
    for model in ["a", "b"] {
        let options = GenerateOptions {
            provider: "nowhere".into(),
            model: model.into(),
            max_tokens: Some(7),
            ..GenerateOptions::default()
        };
        // No adapter is registered: the request fails after it was captured.
        let mut stream = llm.stream(&ctx, options);
        let _ = stream.next().await;
    }
    let lines: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["model"], "b");
    assert_eq!(lines[1]["maxTokens"], 7);
    assert!(lines[0].get("tools").is_none() && lines[0].get("purpose").is_none());
    assert_eq!(lines[0]["messages"], json!([]));
    kernel.shutdown().await;
}
