#[path = "support/memory.rs"]
mod memory;

use hoh_credentials::{
    CredentialInfo, Credentials, CredentialsReferenceUpdated, ResolvedCredential, credential_ref,
    is_credential_key_segment,
};
use hoh_kernel::{Context, Kernel};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

async fn boot(seed: Value) -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<memory::MemoryCredentialsPlugin>(seed)
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx)
}

#[test]
fn brands_posix_shell_identifiers() {
    for value in ["DEEPSEEK_API_KEY", "_private", "lower_case9"] {
        assert_eq!(credential_ref(value).unwrap(), value);
    }
}

#[test]
fn rejects_every_other_shape() {
    for invalid in ["", "9LEADING", "WITH-DASH", "WITH SPACE", "ns:key"] {
        assert_eq!(credential_ref(invalid).unwrap_err().name(), "TypeError");
    }
}

#[test]
fn answers_whether_credential_key_would_accept_the_segment() {
    for valid in ["llm-pi-ai", "openai-codex", "a", "z9"] {
        assert!(is_credential_key_segment(valid));
    }
    for invalid in ["", "My_Proxy", "z.ai", "UPPER", "9leading", "a/b"] {
        assert!(!is_credential_key_segment(invalid));
    }
}

#[tokio::test]
async fn mounts_as_ctx_credentials_and_resolves_a_seeded_reference_with_its_source() {
    let (kernel, ctx) = boot(json!({"DEEPSEEK_API_KEY":"sk-seeded"})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    assert_eq!(
        provider.resolve(&reference).await.unwrap(),
        Some(ResolvedCredential {
            value: "sk-seeded".into(),
            source: "memory".into()
        })
    );
    assert_eq!(
        provider.describe(&reference).await.unwrap(),
        CredentialInfo {
            configured: true,
            source: Some("memory".into()),
            writable: true,
            masked: None,
        }
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn treats_an_empty_stored_value_as_absent_everywhere() {
    let (kernel, ctx) = boot(json!({"DEEPSEEK_API_KEY":""})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    assert_eq!(provider.resolve(&reference).await.unwrap(), None);
    assert_eq!(
        provider.describe(&reference).await.unwrap(),
        CredentialInfo {
            configured: false,
            source: None,
            writable: true,
            masked: None
        }
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn stores_through_set_removes_through_unset_and_emits_the_committed_change() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    ctx.on::<CredentialsReferenceUpdated>(move |event| {
        seen.lock().unwrap().push(event.reference.clone())
    })
    .unwrap();
    provider.set(&reference, "sk-live").await.unwrap();
    assert_eq!(
        provider.resolve(&reference).await.unwrap(),
        Some(ResolvedCredential {
            value: "sk-live".into(),
            source: "memory".into()
        })
    );
    provider.unset(&reference).await.unwrap();
    assert_eq!(provider.resolve(&reference).await.unwrap(), None);
    assert_eq!(*events.lock().unwrap(), [reference.clone(), reference]);
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_an_empty_set_and_keeps_an_absent_unset_silent() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    ctx.on::<CredentialsReferenceUpdated>(move |event| {
        seen.lock().unwrap().push(event.reference.clone())
    })
    .unwrap();
    assert!(
        provider
            .set(&reference, "")
            .await
            .unwrap_err()
            .to_string()
            .contains("empty value")
    );
    provider.unset(&reference).await.unwrap();
    assert!(events.lock().unwrap().is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn removes_the_service_with_its_fiber() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<memory::MemoryCredentialsPlugin>(json!({}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(ctx.get::<Credentials>().is_some());
    fiber.dispose().await;
    assert!(ctx.get::<Credentials>().is_none());
    kernel.shutdown().await;
}
