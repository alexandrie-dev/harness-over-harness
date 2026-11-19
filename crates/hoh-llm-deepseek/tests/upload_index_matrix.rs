use hoh_llm_deepseek::{
    DeepSeekFileId, DeepSeekUploadIndex, DeepSeekUploadRecord, deep_seek_file_scope,
};

fn record(
    scope_key: &str,
    variant_byte: char,
    file: &str,
    expires_at: u64,
) -> DeepSeekUploadRecord {
    DeepSeekUploadRecord {
        scope: deep_seek_file_scope("https://example.test/v1", scope_key),
        attachment_id: format!("sha256:{}", "a".repeat(64)).into(),
        variant_id: format!("sha256:{}", variant_byte.to_string().repeat(64)).into(),
        file_id: DeepSeekFileId::new(file).unwrap(),
        bytes: 7,
        created_at: 1,
        expires_at,
    }
}

#[tokio::test]
async fn removes_only_exact_generations_and_keeps_other_mappings() {
    let directory = tempfile::tempdir().unwrap();
    let index = DeepSeekUploadIndex::new(directory.path().join("files-v3.json"));
    let first = record("a", 'b', "file-1", 10_000);
    let second = record("a", 'c', "file-2", 10_000);
    index.commit(first.clone(), 0, 10).await.unwrap();
    index.commit(second.clone(), 0, 10).await.unwrap();

    index
        .remove(
            &first.scope,
            &[(
                first.variant_id.clone(),
                DeepSeekFileId::new("loser").unwrap(),
            )],
        )
        .await
        .unwrap();
    assert!(
        index
            .get(&first.scope, &first.variant_id, 0, 10, None)
            .await
            .unwrap()
            .is_some()
    );

    index
        .remove(
            &first.scope,
            &[(first.variant_id.clone(), first.file_id.clone())],
        )
        .await
        .unwrap();
    assert!(
        index
            .get(&first.scope, &first.variant_id, 0, 10, None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        index
            .get(&second.scope, &second.variant_id, 0, 10, None)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn clear_is_scoped_and_commit_drops_expired_records() {
    let directory = tempfile::tempdir().unwrap();
    let index = DeepSeekUploadIndex::new(directory.path().join("files-v3.json"));
    let expired = record("a", 'b', "expired", 50);
    let retained = record("b", 'c', "retained", 10_000);
    index.commit(expired.clone(), 0, 10).await.unwrap();
    index.commit(retained.clone(), 100, 10).await.unwrap();
    assert!(
        index
            .get(&expired.scope, &expired.variant_id, 100, 10, None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        index
            .get(&retained.scope, &retained.variant_id, 100, 10, None)
            .await
            .unwrap()
            .is_some()
    );

    index.clear(&expired.scope).await.unwrap();
    assert!(
        index
            .get(&retained.scope, &retained.variant_id, 100, 10, None)
            .await
            .unwrap()
            .is_some()
    );
    index.clear(&retained.scope).await.unwrap();
    assert!(
        index
            .get(&retained.scope, &retained.variant_id, 100, 10, None)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn corrupt_cache_is_repaired_on_next_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("files-v3.json");
    tokio::fs::write(&path, b"not json").await.unwrap();
    let index = DeepSeekUploadIndex::new(&path);
    let candidate = record("a", 'b', "file-1", 10_000);
    assert!(
        index
            .get(&candidate.scope, &candidate.variant_id, 0, 10, None)
            .await
            .unwrap()
            .is_none()
    );
    index.commit(candidate, 0, 10).await.unwrap();
    let repaired: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
    assert_eq!(repaired["formatVersion"], 3);
    assert_eq!(repaired["records"].as_array().unwrap().len(), 1);
}
