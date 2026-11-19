//! Port of credentials-local/tests/records.spec.ts: the record half of the seam. The store keeps an
//! owner's payload verbatim, presence rather than content answers "configured", and every write goes
//! through one serialized read-modify-write so a rotating credential cannot be lost between processes.
mod support;

use hoh_credentials::{
    ApiKeyRecord, CredentialKey, CredentialRecord, CredentialRecordEntry, CredentialRecordInfo,
    CredentialRecordKind, GrantRecord, credential_key, credential_key_scope, credential_ref,
    parse_credential_key,
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use support::*;

fn codex() -> CredentialKey {
    key("llm-pi-ai", "openai-codex")
}
fn bedrock() -> CredentialKey {
    key("llm-pi-ai", "amazon-bedrock")
}
fn other_owner() -> CredentialKey {
    key("llm-kimi", "openai-codex")
}

fn grant(payload: Value) -> CredentialRecord {
    CredentialRecord::Grant(GrantRecord { payload })
}

fn api_key(key: Option<&str>, env: &[(&str, &str)]) -> CredentialRecord {
    CredentialRecord::ApiKey(ApiKeyRecord {
        key: key.map(str::to_owned),
        env: (!env.is_empty()).then(|| {
            env.iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<IndexMap<_, _>>()
        }),
    })
}

/// Store one record outright; the seam offers only the read-modify-write path.
async fn put(
    booted: &Booted,
    key: &CredentialKey,
    record: CredentialRecord,
) -> Result<Option<CredentialRecord>, hoh_error::BoxError> {
    booted
        .credentials()
        .modify_record(
            key,
            Box::new(move |_| Box::pin(async move { Ok(Some(record)) })),
        )
        .await
}

#[test]
fn rejects_a_segment_that_is_not_a_lowercase_hyphenated_identifier() {
    assert!(
        credential_key("llm-pi-ai", "OpenAI")
            .unwrap_err()
            .to_string()
            .contains("credential key segment")
    );
    assert!(
        credential_key("", "codex")
            .unwrap_err()
            .to_string()
            .contains("credential key segment")
    );
}

#[test]
fn stays_disjoint_from_the_reference_grammar() {
    // The `/` is what makes the two key spaces incapable of colliding.
    assert!(
        credential_ref(codex().as_str())
            .unwrap_err()
            .to_string()
            .contains("credential ref")
    );
}

#[test]
fn reads_back_the_owning_plugin_which_is_what_makes_an_orphan_recognizable() {
    assert_eq!(credential_key_scope(&codex()), "llm-pi-ai");
    assert_eq!(credential_key_scope(&other_owner()), "llm-kimi");
}

#[test]
fn admits_a_stored_key_and_refuses_one_that_is_not_two_segments() {
    assert_eq!(
        parse_credential_key("llm-pi-ai/openai-codex").unwrap(),
        codex()
    );
    for bad in ["openai-codex", "a/b/c"] {
        assert!(
            parse_credential_key(bad)
                .unwrap_err()
                .to_string()
                .contains("must be \"<scope>/<id>\"")
        );
    }
}

#[tokio::test]
async fn returns_a_grant_payload_exactly_as_its_owner_wrote_it() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    // Fields the seam has never heard of ride along: an owner's SDK gains them between releases.
    let payload = json!({"type": "oauth", "access": "at", "refresh": "rt", "expires": 1786000000000_i64, "accountId": "acct_1"});
    put(&booted, &codex(), grant(payload.clone()))
        .await
        .unwrap();

    assert_eq!(
        booted.credentials().read_record(&codex()).await.unwrap(),
        Some(grant(payload.clone()))
    );
    let reread = boot(config(&path)).await;
    assert_eq!(
        reread.credentials().read_record(&codex()).await.unwrap(),
        Some(grant(payload))
    );
    reread.shutdown().await;
    booted.shutdown().await;
}

#[tokio::test]
async fn treats_a_record_carrying_no_key_and_no_environment_as_configured() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    // A stored decision, not a blank — the opposite reading of the empty-value rule the reference
    // half follows.
    put(&booted, &bedrock(), api_key(None, &[])).await.unwrap();
    assert_eq!(
        booted
            .credentials()
            .describe_record(&bedrock())
            .await
            .unwrap(),
        CredentialRecordInfo {
            configured: true,
            kind: Some(CredentialRecordKind::ApiKey),
            writable: true
        }
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn describes_an_absent_record_as_unconfigured_but_writable() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    assert_eq!(
        booted
            .credentials()
            .describe_record(&codex())
            .await
            .unwrap(),
        CredentialRecordInfo {
            configured: false,
            kind: None,
            writable: true
        }
    );
    assert_eq!(
        booted.credentials().read_record(&codex()).await.unwrap(),
        None
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn stores_provider_environment_values_beside_or_instead_of_a_key() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    let record = api_key(None, &[("AWS_PROFILE", "prod")]);
    put(&booted, &bedrock(), record.clone()).await.unwrap();
    assert_eq!(
        booted.credentials().read_record(&bedrock()).await.unwrap(),
        Some(record)
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn keeps_references_and_records_in_one_document_without_either_disturbing_the_other() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    booted
        .credentials()
        .set(&reference("DSH_RECORDS_KEY"), "sk-live")
        .await
        .unwrap();
    put(&booted, &codex(), grant(json!({"token": "t"})))
        .await
        .unwrap();

    assert_eq!(
        read(&path),
        "version: 1\nrefs:\n  DSH_RECORDS_KEY: sk-live\nrecords:\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload:\n      token: t\n"
    );
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_RECORDS_KEY").await,
        Some(("sk-live".into(), "file".into()))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn reads_every_record_shape_back_off_disk() {
    let dir = temp_dir();
    let path = doc(&dir);
    // Written by hand rather than through the API: this is the parse path, and an api-key record is
    // legal with a key, with environment values, with both, or with neither.
    write_credentials(
        &path,
        "version: 1\nrecords:\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload:\n      access: at\n  llm-pi-ai/amazon-bedrock:\n    kind: api-key\n    env:\n      AWS_PROFILE: prod\n  llm-pi-ai/azure:\n    kind: api-key\n    key: sk-azure\n  llm-pi-ai/both:\n    kind: api-key\n    key: sk-both\n    env:\n      REGION: eu\n  llm-pi-ai/ambient:\n    kind: api-key\n",
    );
    let booted = boot(config(&path)).await;
    let credentials = booted.credentials();

    assert_eq!(
        credentials.read_record(&codex()).await.unwrap(),
        Some(grant(json!({"access": "at"})))
    );
    assert_eq!(
        credentials.read_record(&bedrock()).await.unwrap(),
        Some(api_key(None, &[("AWS_PROFILE", "prod")]))
    );
    assert_eq!(
        credentials
            .read_record(&key("llm-pi-ai", "azure"))
            .await
            .unwrap(),
        Some(api_key(Some("sk-azure"), &[]))
    );
    assert_eq!(
        credentials
            .read_record(&key("llm-pi-ai", "both"))
            .await
            .unwrap(),
        Some(api_key(Some("sk-both"), &[("REGION", "eu")]))
    );
    assert_eq!(
        credentials
            .read_record(&key("llm-pi-ai", "ambient"))
            .await
            .unwrap(),
        Some(api_key(None, &[]))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn publishes_a_record_an_external_edit_reshaped_whatever_shape_it_took() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    put(&booted, &codex(), grant(json!({"scopes": ["a"]})))
        .await
        .unwrap();
    let seen = record_updates(&booted.ctx);

    // A sequence where a mapping stood, then a mapping that gained a field: neither is caught by an
    // identity check, and reporting them as unchanged would leave a stale credential everywhere.
    for payload in ["[1]", "{a: 1}", "{a: 1, b: 2}"] {
        write_credentials(
            &path,
            &format!(
                "version: 1\nrecords:\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload: {payload}\n"
            ),
        );
        // Any write folds the unobserved document in before committing its own.
        put(&booted, &bedrock(), api_key(None, &[])).await.unwrap();
    }

    assert_eq!(
        snapshot(&seen)
            .iter()
            .filter(|key| key.as_str() == codex().as_str())
            .count(),
        3
    );
    assert_eq!(
        booted.credentials().read_record(&codex()).await.unwrap(),
        Some(grant(json!({"a": 1, "b": 2})))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn keeps_two_owners_of_the_same_provider_id_apart() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    put(&booted, &codex(), grant(json!({"owner": "pi-ai"})))
        .await
        .unwrap();
    put(&booted, &other_owner(), grant(json!({"owner": "kimi"})))
        .await
        .unwrap();
    let credentials = booted.credentials();
    assert_eq!(
        credentials.read_record(&codex()).await.unwrap(),
        Some(grant(json!({"owner": "pi-ai"})))
    );
    assert_eq!(
        credentials.read_record(&other_owner()).await.unwrap(),
        Some(grant(json!({"owner": "kimi"})))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn shows_the_mutation_the_record_as_it_stands_and_commits_its_replacement() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    put(&booted, &codex(), grant(json!({"expires": 1})))
        .await
        .unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();

    let next = booted
        .credentials()
        .modify_record(
            &codex(),
            Box::new(move |current| {
                sink.lock().unwrap().push(current);
                Box::pin(async { Ok(Some(grant(json!({"expires": 2})))) })
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec![Some(grant(json!({"expires": 1})))]
    );
    assert_eq!(next, Some(grant(json!({"expires": 2}))));
    booted.shutdown().await;
}

#[tokio::test]
async fn leaves_the_entry_untouched_when_the_mutation_declines() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    put(&booted, &codex(), grant(json!({"expires": 1})))
        .await
        .unwrap();
    let before = read(&path);
    let seen = record_updates(&booted.ctx);

    // The refresh path declines whenever a second reader finds the credential already rotated;
    // declining must not rewrite the document or announce a change that did not happen.
    let result = booted
        .credentials()
        .modify_record(&codex(), Box::new(|_| Box::pin(async { Ok(None) })))
        .await
        .unwrap();

    assert_eq!(result, Some(grant(json!({"expires": 1}))));
    assert_eq!(read(&path), before);
    assert!(snapshot(&seen).is_empty());
    booted.shutdown().await;
}

#[tokio::test]
async fn announces_a_committed_write_and_a_committed_delete_and_stays_silent_on_an_absent_delete() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    let seen = record_updates(&booted.ctx);
    let credentials = booted.credentials();

    put(&booted, &codex(), grant(json!(1))).await.unwrap();
    credentials.delete_record(&codex()).await.unwrap();
    credentials.delete_record(&codex()).await.unwrap();

    assert_eq!(
        snapshot(&seen),
        vec![codex().to_string(), codex().to_string()]
    );
    assert_eq!(credentials.read_record(&codex()).await.unwrap(), None);
    booted.shutdown().await;
}

#[tokio::test]
async fn removes_a_later_record_without_disturbing_the_annotation_above_the_first() {
    let dir = temp_dir();
    let path = doc(&dir);
    // The comment sits above the section's first entry. Removing a *later* entry must leave it
    // exactly where it is.
    write_credentials(
        &path,
        "version: 1\nrecords:\n  # the one to keep\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload: 1\n  llm-pi-ai/amazon-bedrock:\n    kind: api-key\n",
    );
    let booted = boot(config(&path)).await;

    booted
        .credentials()
        .delete_record(&bedrock())
        .await
        .unwrap();

    assert_eq!(
        read(&path),
        "version: 1\nrecords:\n  # the one to keep\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload: 1\n"
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn folds_an_unobserved_external_record_edit_into_a_write_instead_of_overwriting_it() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    put(&booted, &codex(), grant(json!({"v": 1})))
        .await
        .unwrap();
    // Landed on disk with no watcher to report it — the same blind spot as a debounce window, a
    // missed event, or another process's write.
    write_credentials(
        &path,
        "version: 1\nrecords:\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload:\n      v: 1\n  llm-pi-ai/amazon-bedrock:\n    kind: api-key\n    env:\n      AWS_PROFILE: prod\n",
    );

    put(&booted, &codex(), grant(json!({"v": 2})))
        .await
        .unwrap();

    let credentials = booted.credentials();
    assert_eq!(
        credentials.read_record(&bedrock()).await.unwrap(),
        Some(api_key(None, &[("AWS_PROFILE", "prod")]))
    );
    assert_eq!(
        credentials.read_record(&codex()).await.unwrap(),
        Some(grant(json!({"v": 2})))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn keeps_both_records_when_two_providers_write_the_same_document_concurrently() {
    let dir = temp_dir();
    let path = doc(&dir);
    let first = boot(config(&path)).await;
    let second = boot(config(&path)).await;

    let a = async {
        for v in [1, 2, 3] {
            put(&first, &codex(), grant(json!({ "v": v })))
                .await
                .unwrap();
        }
    };
    let b = async {
        for v in [1, 2, 3] {
            put(&second, &bedrock(), grant(json!({ "v": v })))
                .await
                .unwrap();
        }
    };
    tokio::join!(a, b);

    let reread = boot(config(&path)).await;
    let credentials = reread.credentials();
    assert_eq!(
        credentials.read_record(&codex()).await.unwrap(),
        Some(grant(json!({"v": 3})))
    );
    assert_eq!(
        credentials.read_record(&bedrock()).await.unwrap(),
        Some(grant(json!({"v": 3})))
    );
    reread.shutdown().await;
    second.shutdown().await;
    first.shutdown().await;
}

#[tokio::test]
async fn enumerates_stored_records_by_address_and_tag_never_by_value() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    put(&booted, &codex(), grant(json!({"secret": "do-not-list"})))
        .await
        .unwrap();
    put(&booted, &bedrock(), api_key(Some("sk-listed"), &[]))
        .await
        .unwrap();

    let listed = booted.credentials().list_records().await.unwrap();

    assert_eq!(
        listed,
        vec![
            CredentialRecordEntry {
                key: codex(),
                kind: CredentialRecordKind::Grant
            },
            CredentialRecordEntry {
                key: bedrock(),
                kind: CredentialRecordKind::ApiKey
            },
        ]
    );
    let rendered = serde_json::to_string(&listed).unwrap();
    assert!(!rendered.contains("do-not-list"));
    assert!(!rendered.contains("sk-listed"));
    booted.shutdown().await;
}

// The JS source also refuses a Date, a bigint, a function and NaN as a grant payload. Rust's
// `serde_json::Value` cannot hold the first three, and NaN cannot be built into a `Value::Number`,
// so the only payload shape left to refuse is unrepresentable here; this case is not ported (see
// PORT_NOTES.md).

#[tokio::test]
async fn refuses_an_api_key_record_this_document_could_not_read_back() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;

    // The same admission rule as the read path: an api-key record the parser would reject at the
    // next boot is refused before it is rendered.
    let failure = put(&booted, &codex(), api_key(Some(""), &[]))
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("empty key"), "{failure}");
    let failure = put(&booted, &codex(), api_key(None, &[("not a name", "value")]))
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("must match"), "{failure}");
    let failure = put(&booted, &codex(), api_key(None, &[("AWS_REGION", "")]))
        .await
        .unwrap_err();
    assert!(
        failure.to_string().contains("non-empty string"),
        "{failure}"
    );
    assert_eq!(
        booted.credentials().read_record(&codex()).await.unwrap(),
        None
    );
    assert!(!path.exists());
    booted.shutdown().await;
}

#[tokio::test]
async fn refuses_record_writes_once_disposed() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    let credentials = booted.credentials();
    booted.fiber.dispose().await;

    let failure = credentials
        .modify_record(
            &codex(),
            Box::new(|_| Box::pin(async { Ok(Some(grant(json!(1)))) })),
        )
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("disposed"), "{failure}");
    let failure = credentials.delete_record(&codex()).await.unwrap_err();
    assert!(failure.to_string().contains("disposed"), "{failure}");
    booted.kernel.shutdown().await;
}
