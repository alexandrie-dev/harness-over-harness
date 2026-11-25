use hoh_kernel::{FiberState, Kernel};
use hoh_loader::{ProfileContext, ProfileContextKey};
use hoh_plugin_whisper_dictation::AuthorPlugin;
use hoh_sdk_jsonrpc_server::{HohRpcMethodRegistry, HohRpcMethods};
use std::sync::Arc;

#[tokio::test]
async fn mounts() {
    let profile = tempfile::tempdir().unwrap();
    let kernel = Kernel::start();
    let root = kernel.root();
    root.provide::<ProfileContextKey>(Arc::new(ProfileContext {
        name: "window".into(),
        dir: profile.path().into(),
        home: profile.path().into(),
    }))
    .await
    .unwrap();
    root.provide::<HohRpcMethods>(Arc::new(HohRpcMethodRegistry::default()))
        .await
        .unwrap();
    let fiber = root.plugin::<AuthorPlugin>(Default::default()).unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    kernel.shutdown().await;
}
