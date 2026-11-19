#![allow(dead_code)]
use hoh_error::{BoxError, SharedError};
use hoh_session::{SessionEvent, SessionHeader, SessionId};
use hoh_session_persistence::SessionHandle;
use hoh_session_persistence_jsonl::{
    Config, JsonlCompression, JsonlSessionPersistence, format::*, testing::IoHook,
};
use serde_json::{Value, json};
use std::{error::Error, sync::Arc};
pub struct Fixture {
    pub kernel: hoh_kernel::Kernel,
    pub root: tempfile::TempDir,
    pub backend: JsonlSessionPersistence,
}
impl Fixture {
    pub async fn new(compression: JsonlCompression) -> Self {
        Self::with_hook(compression, None).await
    }
    pub async fn with_hook(compression: JsonlCompression, hook: Option<IoHook>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let kernel = hoh_kernel::Kernel::start();
        let mut config = Config::new(root.path().to_string_lossy());
        config.compression = compression;
        let backend = JsonlSessionPersistence::with_io_hook(&kernel.root(), config, hook)
            .await
            .unwrap();
        Self {
            kernel,
            root,
            backend,
        }
    }
    pub fn path(&self, id: &str) -> String {
        log_path(
            &self.root.path().to_string_lossy(),
            None,
            &id.into(),
            self.backend.compression(),
        )
        .unwrap()
    }
    pub async fn finish(self) {
        self.backend.close().await.unwrap();
        self.kernel.shutdown().await;
    }
    pub async fn store(&self, header: &SessionHeader, rows: &[Value]) -> (String, Vec<u8>) {
        self.store_raw(&serde_json::to_value(header).unwrap(), rows)
            .await
    }
    pub async fn store_raw(&self, header: &Value, rows: &[Value]) -> (String, Vec<u8>) {
        let mut physical = header.clone();
        physical["type"] = "session".into();
        if physical.get("delegationDepth").is_none() {
            physical["delegationDepth"] = 0.into();
        }
        let id = physical["id"].as_str().unwrap_or("generation-test");
        let version = physical["version"].as_u64().unwrap_or(4);
        let path = generation_log_path(
            &self.root.path().to_string_lossy(),
            physical["cwd"].as_str(),
            &id.into(),
            version,
            self.backend.compression(),
        )
        .unwrap();
        let mut lines = vec![line(&physical)];
        lines.extend(rows.iter().map(line));
        let bytes = if self.backend.compression() == JsonlCompression::None {
            lines.concat().into_bytes()
        } else {
            let mut bytes = vec![];
            for line in lines {
                bytes.extend(
                    hoh_session_persistence_jsonl::zstd::compress_zstd_frame(line)
                        .await
                        .unwrap(),
                );
            }
            bytes
        };
        tokio::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, &bytes).await.unwrap();
        (path, bytes)
    }
}
pub fn meta(id: &str) -> SessionHeader {
    SessionHeader::new(SessionId::from(id), 1000)
}
pub fn line(value: &Value) -> String {
    format!("{}\n", hoh_util_values::js_json_stringify(value))
}
pub fn event(kind: &str, seq: u64, data: Value) -> Value {
    json!({"type":kind,"seq":seq,"time":seq+1,"data":data})
}
pub fn log() -> Vec<Value> {
    vec![
        event("turn/start", 0, json!({"turn":1})),
        json!({"type":"user/message","seq":1,"time":2,"data":{"id":"one-turn-user","role":"user","content":[{"type":"text","text":"hi"}],"source":{"kind":"user"}},"surfaceOp":"append"}),
        event("step/start", 2, json!({"turn":1,"step":1})),
        json!({"type":"assistant/message","seq":3,"time":4,"data":{"turn":1,"step":1,"message":{"id":"one-turn-assistant","role":"assistant","content":[{"type":"text","text":"hello"}],"source":{"kind":"model","provider":"mock","model":"mock"}},"stream":[{"type":"chunk","time":3,"chunk":{"type":"block-start","index":0,"blockType":"text"}},{"type":"text-chunks","time0":3,"index":0,"dt":[],"texts":["hello"]},{"type":"chunk","time":4,"chunk":{"type":"block-end","index":0,"block":{"type":"text","text":"hello"}}},{"type":"chunk","time":4,"chunk":{"type":"finish","reason":{"kind":"stop"}}}]},"surfaceOp":"append"}),
        event("step/end", 4, json!({"turn":1,"step":1})),
        event(
            "turn/end",
            5,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ]
}
pub fn arcs(values: &[Value]) -> Vec<Arc<SessionEvent>> {
    values
        .iter()
        .cloned()
        .map(SessionEvent::from_value)
        .map(Arc::new)
        .collect()
}
pub fn values(events: &[Arc<SessionEvent>]) -> Vec<Value> {
    events.iter().map(|e| e.as_value().clone()).collect()
}
pub async fn read(handle: &dyn SessionHandle) -> Vec<Value> {
    values(&handle.read(None, None, None).await.unwrap().events)
}
pub fn fail<T>(result: std::result::Result<T, BoxError>, text: &str) -> BoxError {
    let failure = match result {
        Ok(_) => panic!("expected failure containing {text}"),
        Err(e) => e,
    };
    assert!(
        failure.to_string().contains(text),
        "expected {text:?}, got {failure:?}"
    );
    failure
}
pub fn fail_match<T>(result: std::result::Result<T, BoxError>, pattern: &str) -> BoxError {
    let error = match result {
        Ok(_) => panic!("expected failure matching {pattern}"),
        Err(error) => error,
    };
    assert!(
        regex::Regex::new(pattern)
            .unwrap()
            .is_match(&error.to_string()),
        "expected {pattern:?}, got {error:?}"
    );
    error
}
pub fn underlying<'a>(mut failure: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
    while let Some(shared) = failure.downcast_ref::<SharedError>() {
        failure = shared.get_ref();
    }
    failure
}
pub fn has_class<T: Error + 'static>(failure: &(dyn Error + 'static)) -> bool {
    underlying(failure).is::<T>()
}
pub fn no_op_report() -> Arc<dyn Fn(BoxError) + Send + Sync> {
    Arc::new(|failure| panic!("unexpected background failure: {failure}"))
}
pub async fn append_bytes(path: &str, bytes: &[u8]) {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .unwrap();
    file.write_all(bytes).await.unwrap();
    file.flush().await.unwrap();
}
