use hoh_attachment::{
    ImageAttachmentRef, ImageMediaType, RequestImageAttachment, RequestImageDepth,
    RequestImageSpace,
};
use hoh_llm::{
    ContentBlock, ImageBlock, MessageSource, NewUserMessage, StreamChunk, create_user_message,
};
use hoh_llm_deepseek::listing::{ClientRequirement, ListedBadge, ListedCapabilities};
use hoh_llm_deepseek::sse::{SseEvent, SseParser, decode_event};
use hoh_llm_deepseek::translate::Translator;
use hoh_llm_deepseek::*;
use indexmap::IndexMap;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn resolves_defaults_and_builtin_catalog() {
    let options = resolve_adapter_options(&Options::default()).unwrap();
    assert_eq!(options.base_url, PUBLIC_BASE_URL);
    assert_eq!(options.max_tokens, DEFAULT_MAX_TOKENS);
    assert!(is_built_in_catalog(&options.models));
    assert_eq!(options.models[0].id, "deepseek-flash");
}

#[test]
fn resolves_base_url_from_launch_environment_below_explicit_config() {
    use hoh_launch_environment::{
        LaunchEnvironmentLayerInput, LaunchEnvironmentSource, create_launch_environment_snapshot,
    };
    let environment = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::ProjectEnv,
        path: None,
        values: IndexMap::from([(
            "DEEPSEEK_BASE_URL".into(),
            "https://gateway.test/anthropic".into(),
        )]),
    }]);
    assert_eq!(
        resolve_adapter_options_with_environment(&Options::default(), Some(&environment))
            .unwrap()
            .base_url,
        "https://gateway.test/anthropic"
    );
    assert_eq!(
        resolve_adapter_options_with_environment(
            &Options {
                base_url: Some("https://explicit.test".into()),
                ..Default::default()
            },
            Some(&environment),
        )
        .unwrap()
        .base_url,
        "https://explicit.test"
    );
}

#[test]
fn rejects_thinking_conflict() {
    let error = resolve_adapter_options(&Options {
        thinking: Some("disabled".into()),
        reasoning_effort: Some("high".into()),
        ..Default::default()
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "llm-deepseek: only reasoningEffort \"off\" can be configured when thinking is disabled"
    );
}

#[test]
fn rejects_duplicate_models() {
    let model = DeepSeekCatalogModel {
        id: "same".into(),
        ..Default::default()
    };
    let error = resolve_adapter_options(&Options {
        models: Some(vec![model.clone(), model]),
        ..Default::default()
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "llm-deepseek: duplicate catalog model \"same\""
    );
}

#[test]
fn rejects_image_limits_on_text_model() {
    let model = DeepSeekCatalogModel {
        id: "text".into(),
        image_max_bytes: Some(1),
        ..Default::default()
    };
    assert!(
        resolve_adapter_options(&Options {
            models: Some(vec![model]),
            ..Default::default()
        })
        .unwrap_err()
        .to_string()
        .contains("cannot declare image request limits")
    );
}

#[test]
fn rejects_invalid_catalog_context_and_output_limits() {
    for (model, expected) in [
        (
            DeepSeekCatalogModel {
                id: "zero-context".into(),
                context_window: Some(0),
                ..Default::default()
            },
            "llm-deepseek: catalog model \"zero-context\" contextWindow must be a positive integer",
        ),
        (
            DeepSeekCatalogModel {
                id: "zero-output".into(),
                max_tokens: Some(0),
                ..Default::default()
            },
            "llm-deepseek: catalog model \"zero-output\" maxTokens must be a positive integer",
        ),
    ] {
        let error = resolve_adapter_options(&Options {
            models: Some(vec![model]),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn image_token_reference_values() {
    for (width, height, expected) in [
        (100, 100, 184),
        (544, 544, 184),
        (640, 480, 206),
        (800, 800, 422),
        (1024, 768, 496),
        (1066, 600, 407),
        (1300, 1300, 994),
        (1920, 1080, 968),
        (2000, 2000, 994),
        (5000, 5000, 994),
        (300, 50, 200),
        (8192, 100, 593),
        (16, 8192, 590),
        (9000, 1, 1024),
        (1, 9000, 1024),
        (12, 1123, 380),
        (89, 2076, 254),
    ] {
        assert_eq!(
            deep_seek_image_tokens(width, height),
            expected,
            "{width}x{height}"
        );
    }
}

#[test]
fn request_image_reference_dimensions() {
    for (width, height, expected_width, expected_height) in [
        (1224, 1429, 1187, 1386),
        (800, 800, 800, 800),
        (1302, 1302, 1302, 1302),
        (1303, 1303, 1302, 1302),
        (2048, 2048, 1302, 1302),
        (2048, 1024, 1848, 924),
        (3840, 2160, 1708, 961),
        (1080, 2400, 838, 1862),
    ] {
        let result = deep_seek_request_image_dimensions(width, height);
        assert_eq!(
            (result.width, result.height),
            (expected_width, expected_height)
        );
    }
}

fn request_image(bytes: u64, offloaded: bool) -> (hoh_llm::RequestMessage, RequestImageAttachment) {
    let reference = ImageAttachmentRef {
        attachment_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .into(),
        media_type: ImageMediaType::Png,
        bytes,
        width: 10,
        height: 10,
        name: Some("sample.png".into()),
        original_dimensions: None,
    };
    let message = create_user_message(NewUserMessage {
        content: vec![ContentBlock::from(ImageBlock {
            attachment: reference.clone(),
            offloaded: offloaded.then_some(true),
        })],
        source: MessageSource::new("user"),
    })
    .into();
    let version = RequestImageAttachment {
        variant_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .into(),
        attachment: reference,
        data: vec![0; bytes as usize],
        media_type: ImageMediaType::Png,
        bytes,
        width: 10,
        height: 10,
        depth: RequestImageDepth::Uchar,
        space: RequestImageSpace::Srgb,
        has_alpha: true,
    };
    (message, version)
}

#[test]
fn inline_images_enforce_base64_budget_and_report_offload_count() {
    let (message, version) = request_image(4, false);
    let mut connection = resolve_adapter_options(&Options::default()).unwrap();
    connection.max_inline_request_image_bytes = 5;
    connection.inline_image_offload_byte_quantum = 1;
    let versions = HashMap::from([(version.attachment.attachment_id.clone(), version)]);
    let error = images::inline_images(&[message], &versions, &connection).unwrap_err();
    assert_eq!(error.code(), "IMAGE_OFFLOAD_REQUIRED");
    assert_eq!(
        error
            .detail::<hoh_llm::LlmFailure>()
            .and_then(|failure| failure.offload_images),
        Some(1)
    );
}

#[tokio::test]
async fn prepare_images_projects_logged_offload_without_attachment_access() {
    let (message, _) = request_image(4, true);
    let connection = resolve_adapter_options(&Options::default()).unwrap();
    let access = |_image: &ImageAttachmentRef| None;
    let (messages, versions) = images::prepare_images(
        &[message],
        &connection,
        "deepseek-chat",
        None,
        &access,
        None,
    )
    .await
    .unwrap();
    assert!(versions.is_empty());
    assert_eq!(messages[0].content()[0].r#type, "text");
    assert!(
        messages[0].content()[0]
            .text_value()
            .unwrap()
            .contains("image omitted to fit request image limits")
    );
}

#[tokio::test]
async fn files_api_rejects_expiry_before_transport() {
    let client = DeepSeekFilesClient::new("http://127.0.0.1:9/v1", IndexMap::new()).unwrap();
    let error = client
        .upload(
            &[],
            ImageMediaType::Png,
            "empty.png",
            MIN_FILE_EXPIRY_SECONDS - 1,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), "INVALID_REQUEST");
    assert_eq!(
        error.message(),
        "DeepSeek file expiry must be between 3600 and 2592000 seconds."
    );
}

#[test]
fn message_api_root_preserves_v1() {
    assert_eq!(
        messages_api::MESSAGES_TOOL_CHANGES_BETA,
        "mid-conversation-tool-changes-2026-07-01"
    );
    assert_eq!(
        messages_api::messages_api_root("https://example.test/custom/v1///").unwrap(),
        "https://example.test/custom/v1"
    );
    assert_eq!(
        messages_api::messages_api_root("https://example.test/custom/").unwrap(),
        "https://example.test/custom/v1"
    );
}

#[test]
fn model_listing_url_uses_origin() {
    assert_eq!(
        model_listing_url("https://example.test/custom/messages").unwrap(),
        "https://example.test/models"
    );
}

#[test]
fn listing_skips_bad_rows_and_duplicates() {
    let listing = parse_model_listing(
        &json!({"data":[null, 1, {"id":""}, {"id":"a","name":""}, {"id":"a"}]}),
    )
    .unwrap();
    assert_eq!(listing.skipped, 4);
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].id, "a");
}

#[test]
fn listing_decodes_capabilities_and_requirement() {
    let listing = parse_model_listing(&json!({"data":[{
        "id":"m", "input_modalities":["image","audio","text"],
        "context_window":100, "max_output_tokens":10,
        "client_requirements":{"deepseek-harness":{"min_version":"0.1.0"}},
        "api_capabilities":{"anthropic_messages":{"system_prompt_update":"future","tool_update":"addition-only"}}
    }]})).unwrap();
    assert_eq!(
        listing.entries[0].requirement,
        ClientRequirement::MinVersion {
            version: "0.1.0".into()
        }
    );
    let ListedCapabilities::Usable { facts, .. } = &listing.entries[0].capabilities else {
        panic!("usable")
    };
    assert_eq!(facts.input_modalities, ["image", "text"]);
    assert_eq!(facts.system_prompt_update.as_deref(), Some("leading-only"));
    assert_eq!(facts.tool_update.as_deref(), Some("addition-only"));
}

#[test]
fn listing_decodes_localized_badge_and_rejects_unreadable_text() {
    let valid = parse_model_listing(&json!({"data":[{"id":"m","api_capabilities":{"anthropic_messages":{}},"client_metadata":{"deepseek-harness":{"badge":{"text":{"en":"New","zh":" 新 "},"tone":"info"}}}}]})).unwrap();
    assert!(matches!(
        valid.entries[0].badge,
        Some(ListedBadge::Badge { .. })
    ));
    let invalid = parse_model_listing(&json!({"data":[{"id":"m","api_capabilities":{"anthropic_messages":{}},"client_metadata":{"deepseek-harness":{"badge":{"text":"line\nbreak"}}}}]})).unwrap();
    assert!(matches!(
        invalid.entries[0].badge,
        Some(ListedBadge::Invalid { .. })
    ));
}

#[test]
fn listing_badge_uses_javascript_trim_for_bom() {
    let listing = parse_model_listing(&json!({"data":[{
        "id":"m",
        "api_capabilities":{"anthropic_messages":{}},
        "client_metadata":{"deepseek-harness":{"badge":{"text":"\u{feff}New\u{feff}"}}}
    }]}))
    .unwrap();
    let Some(ListedBadge::Badge { badge }) = &listing.entries[0].badge else {
        panic!("BOM-trimmed badge should be accepted")
    };
    assert_eq!(serde_json::to_value(&badge.text).unwrap(), json!("New"));
}

#[test]
fn listed_catalog_gates_unknown_and_versioned_models() {
    let supported = resolve_adapter_options(&Options::default()).unwrap().models;
    let listing = parse_model_listing(&json!({"data":[
        {"id":"unknown","api_capabilities":{"anthropic_messages":{}}},
        {"id":"future","client_requirements":{"deepseek-harness":{"min_version":"9.0.0"}},"api_capabilities":{"anthropic_messages":{}}}
    ]})).unwrap();
    let result = resolve_listed_catalog(ListedCatalogInput {
        listing: Some(&listing.entries),
        supported: &supported,
        overrides: &IndexMap::new(),
        route_max_tokens: 100,
        client_version: "0.2.0-rc.1",
    });
    assert_eq!(
        result.refusals.get("unknown"),
        Some(&CatalogRefusal::Unsupported)
    );
    assert!(matches!(
        result.refusals.get("future"),
        Some(CatalogRefusal::UpgradeRequired { .. })
    ));
}

#[test]
fn listed_catalog_ignores_build_metadata_and_rejects_leading_zero_prerelease() {
    let supported = resolve_adapter_options(&Options::default()).unwrap().models;
    let compatible = parse_model_listing(&json!({"data":[{
        "id":"deepseek-flash",
        "client_requirements":{"deepseek-harness":{"min_version":"0.2.0-rc.1+001"}},
        "api_capabilities":{"anthropic_messages":{}}
    }]}))
    .unwrap();
    let result = resolve_listed_catalog(ListedCatalogInput {
        listing: Some(&compatible.entries),
        supported: &supported,
        overrides: &IndexMap::new(),
        route_max_tokens: 100,
        client_version: "0.2.0-rc.1",
    });
    assert!(
        result
            .offered
            .iter()
            .any(|model| model.id == "deepseek-flash")
    );

    let invalid = parse_model_listing(&json!({"data":[{
        "id":"deepseek-flash",
        "client_requirements":{"deepseek-harness":{"min_version":"0.2.0-rc.01"}},
        "api_capabilities":{"anthropic_messages":{}}
    }]}))
    .unwrap();
    let result = resolve_listed_catalog(ListedCatalogInput {
        listing: Some(&invalid.entries),
        supported: &supported,
        overrides: &IndexMap::new(),
        route_max_tokens: 100,
        client_version: "0.2.0-rc.1",
    });
    assert_eq!(
        result.refusals.get("deepseek-flash"),
        Some(&CatalogRefusal::Unsupported)
    );
    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.contains("unreadable"))
    );
}

#[test]
fn listed_catalog_layers_known_model() {
    let supported = resolve_adapter_options(&Options::default()).unwrap().models;
    let listing = parse_model_listing(&json!({"data":[{"id":"deepseek-flash","name":"Listed","context_window":77,"max_output_tokens":50,"input_modalities":["text"],"api_capabilities":{"anthropic_messages":{"system_prompt_update":"leading-only"}}}]})).unwrap();
    let result = resolve_listed_catalog(ListedCatalogInput {
        listing: Some(&listing.entries),
        supported: &supported,
        overrides: &IndexMap::new(),
        route_max_tokens: 40,
        client_version: "0.2.0",
    });
    let model = &result.offered[0];
    assert_eq!(model.name.as_deref(), Some("Listed"));
    assert_eq!(model.context_window, Some(77));
    assert_eq!(model.max_tokens, Some(40));
    assert!(model.image_max_bytes.is_none());
    assert!(model.system_prompt_update.is_none());
}

#[test]
fn sse_frames_split_lines_and_ignores_comments() {
    let mut parser = SseParser::new();
    assert!(
        parser
            .push(b": keepalive\r\nevent: message_start\r\ndata: {\"type\":\"message_")
            .unwrap()
            .is_empty()
    );
    let events = parser.push(b"start\"}\r\n\r\n").unwrap();
    assert_eq!(
        events,
        vec![SseEvent {
            event: Some("message_start".into()),
            data: "{\"type\":\"message_start\"}".into()
        }]
    );
    assert_eq!(
        decode_event(events[0].clone()).unwrap()["type"],
        "message_start"
    );
}

#[test]
fn sse_rejects_event_type_mismatch() {
    let error = decode_event(SseEvent {
        event: Some("message_stop".into()),
        data: "{\"type\":\"message_start\"}".into(),
    })
    .unwrap_err();
    assert_eq!(error.code(), "MALFORMED_RESPONSE");
}

#[test]
fn translator_emits_text_usage_finish_and_replay() {
    let mut translator = Translator::new("m");
    let mut chunks = Vec::new();
    for event in [
        json!({"type":"message_start","message":{"usage":{"input_tokens":2}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Hi"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"!"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
        json!({"type":"message_stop"}),
    ] {
        chunks.extend(translator.feed(&event).unwrap());
    }
    assert!(translator.finish().is_ok());
    assert!(matches!(
        chunks.last(),
        Some(StreamChunk::Finish {
            replay_state: Some(_),
            ..
        })
    ));
    let usage = chunks
        .iter()
        .find_map(|chunk| match chunk {
            StreamChunk::Usage { usage, .. } => Some(usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(usage.total_tokens, Some(5));
}

#[test]
fn translator_rejects_event_before_start() {
    let mut translator = Translator::new("m");
    let error = translator
        .feed(&json!({"type":"message_stop"}))
        .unwrap_err();
    assert_eq!(error.code(), "MALFORMED_RESPONSE");
}

#[test]
fn file_scope_is_stable_and_credential_scoped() {
    let a = deep_seek_file_scope("https://example.test/v1/", "a");
    assert_eq!(a, deep_seek_file_scope("https://example.test/v1", "a"));
    assert_ne!(a, deep_seek_file_scope("https://example.test/v1", "b"));
    assert_eq!(a.as_str().len(), 64);
}

#[tokio::test]
async fn upload_index_commits_and_reuses_winner() {
    let directory = tempfile::tempdir().unwrap();
    let index = DeepSeekUploadIndex::new(directory.path().join("files-v3.json"));
    let scope = deep_seek_file_scope("https://example.test", "key");
    let candidate = DeepSeekUploadRecord {
        scope: scope.clone(),
        attachment_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .into(),
        variant_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .into(),
        file_id: DeepSeekFileId::new("file-1").unwrap(),
        bytes: 7,
        created_at: 1,
        expires_at: 10_000,
    };
    assert!(
        index
            .commit(candidate.clone(), 0, 10)
            .await
            .unwrap()
            .accepted
    );
    let second = index
        .commit(
            DeepSeekUploadRecord {
                file_id: DeepSeekFileId::new("file-2").unwrap(),
                ..candidate.clone()
            },
            0,
            10,
        )
        .await
        .unwrap();
    assert!(!second.accepted);
    assert_eq!(second.record.file_id.as_str(), "file-1");
    assert_eq!(
        index
            .get(&scope, &candidate.variant_id, 0, 10, None)
            .await
            .unwrap()
            .unwrap()
            .file_id
            .as_str(),
        "file-1"
    );
}

#[tokio::test]
async fn upload_index_coordinates_independent_instances() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("files-v3.json");
    let first = DeepSeekUploadIndex::new(path.clone());
    let second = DeepSeekUploadIndex::new(path);
    let scope = deep_seek_file_scope("https://example.test", "key");
    let candidate = |id: &str| DeepSeekUploadRecord {
        scope: scope.clone(),
        attachment_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .into(),
        variant_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .into(),
        file_id: DeepSeekFileId::new(id).unwrap(),
        bytes: 7,
        created_at: 1,
        expires_at: 10_000,
    };
    let (left, right) = tokio::join!(
        first.commit(candidate("file-1"), 0, 10),
        second.commit(candidate("file-2"), 0, 10)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.accepted, right.accepted);
    assert_eq!(left.record.file_id, right.record.file_id);
}

#[tokio::test]
async fn upload_index_treats_duplicate_persisted_mappings_as_corrupt() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("files-v3.json");
    let scope = deep_seek_file_scope("https://example.test", "key");
    let attachment = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let variant = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    tokio::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "formatVersion": 3,
            "records": [
                {"scope":scope,"attachmentId":attachment,"variantId":variant,"fileId":"file-1","bytes":7,"createdAt":1,"expiresAt":10000},
                {"scope":scope,"attachmentId":attachment,"variantId":variant,"fileId":"file-2","bytes":7,"createdAt":1,"expiresAt":10000}
            ]
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let index = DeepSeekUploadIndex::new(&path);
    assert!(
        index
            .get(&scope, &variant.into(), 0, 10, None)
            .await
            .unwrap()
            .is_none()
    );
    let committed = index
        .commit(
            DeepSeekUploadRecord {
                scope,
                attachment_id: attachment.into(),
                variant_id: variant.into(),
                file_id: DeepSeekFileId::new("file-3").unwrap(),
                bytes: 7,
                created_at: 1,
                expires_at: 10_000,
            },
            0,
            10,
        )
        .await
        .unwrap();
    assert!(committed.accepted);
}

#[test]
fn file_id_rejects_empty_values() {
    assert!(DeepSeekFileId::new("").is_none());
    assert_eq!(DeepSeekFileId::new("file-1").unwrap().as_str(), "file-1");
}
