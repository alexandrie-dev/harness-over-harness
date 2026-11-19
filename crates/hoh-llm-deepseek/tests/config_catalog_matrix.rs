use hoh_attachment::{ImageAttachmentRef, ImageMediaType};
use hoh_llm::{ImageBlock, LlmImageRequestPricing};
use hoh_llm_deepseek::listing::ListedCapabilities;
use hoh_llm_deepseek::*;
use indexmap::IndexMap;
use serde_json::json;

fn vision_model() -> DeepSeekCatalogModel {
    DeepSeekCatalogModel {
        id: "vision".into(),
        input_modalities: Some(vec!["text".into(), "image".into()]),
        ..Default::default()
    }
}

#[test]
fn validates_scalar_configuration_bounds() {
    let cases = [
        (
            Options {
                max_tokens: Some(0),
                ..Default::default()
            },
            "maxTokens",
        ),
        (
            Options {
                default_context_window: Some(0),
                ..Default::default()
            },
            "defaultContextWindow",
        ),
        (
            Options {
                stream_idle_timeout_ms: Some(0.0),
                ..Default::default()
            },
            "streamIdleTimeoutMs",
        ),
        (
            Options {
                stream_idle_timeout_ms: Some(f64::NAN),
                ..Default::default()
            },
            "streamIdleTimeoutMs",
        ),
        (
            Options {
                max_request_files_bytes: Some(0),
                ..Default::default()
            },
            "maxRequestFilesBytes",
        ),
        (
            Options {
                max_inline_request_image_bytes: Some(0),
                ..Default::default()
            },
            "maxInlineRequestImageBytes",
        ),
        (
            Options {
                max_images_per_request: Some(0),
                ..Default::default()
            },
            "maxImagesPerRequest",
        ),
        (
            Options {
                image_offload_byte_quantum: Some(0),
                ..Default::default()
            },
            "imageOffloadByteQuantum",
        ),
        (
            Options {
                inline_image_offload_byte_quantum: Some(0),
                ..Default::default()
            },
            "inlineImageOffloadByteQuantum",
        ),
        (
            Options {
                image_offload_count_quantum: Some(0),
                ..Default::default()
            },
            "imageOffloadCountQuantum",
        ),
        (
            Options {
                files_api_timeout_ms: Some(f64::INFINITY),
                ..Default::default()
            },
            "filesApiTimeoutMs",
        ),
        (
            Options {
                file_expires_after_seconds: Some(3_599),
                ..Default::default()
            },
            "fileExpiresAfterSeconds",
        ),
        (
            Options {
                file_expires_after_seconds: Some(2_592_001),
                ..Default::default()
            },
            "fileExpiresAfterSeconds",
        ),
        (
            Options {
                file_quota_cleanup_batch: Some(0),
                ..Default::default()
            },
            "fileQuotaCleanupBatch",
        ),
        (
            Options {
                file_quota_cleanup_batch: Some(1_001),
                ..Default::default()
            },
            "fileQuotaCleanupBatch",
        ),
    ];
    for (options, field) in cases {
        let error = resolve_adapter_options(&options).unwrap_err();
        assert!(error.to_string().contains(field), "{field}: {error}");
    }
}

#[test]
fn rejects_offload_quanta_larger_than_request_limits() {
    for (options, expected) in [
        (
            Options {
                max_request_files_bytes: Some(10),
                image_offload_byte_quantum: Some(11),
                ..Default::default()
            },
            "imageOffloadByteQuantum must not exceed maxRequestFilesBytes",
        ),
        (
            Options {
                max_inline_request_image_bytes: Some(10),
                inline_image_offload_byte_quantum: Some(11),
                ..Default::default()
            },
            "inlineImageOffloadByteQuantum must not exceed maxInlineRequestImageBytes",
        ),
        (
            Options {
                max_images_per_request: Some(10),
                image_offload_count_quantum: Some(11),
                ..Default::default()
            },
            "imageOffloadCountQuantum must not exceed maxImagesPerRequest",
        ),
    ] {
        assert!(
            resolve_adapter_options(&options)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
}

#[test]
fn validates_file_refresh_margin_and_base_url() {
    let error = resolve_adapter_options(&Options {
        file_expires_after_seconds: Some(3_600),
        file_refresh_margin_seconds: Some(3_600),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.to_string().contains("below fileExpiresAfterSeconds"));

    for base_url in [
        "ftp://example.test",
        "https://user:pass@example.test",
        "https://example.test/?query=1",
        "https://example.test/#fragment",
    ] {
        let error = resolve_adapter_options(&Options {
            base_url: Some(base_url.into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("HTTP(S) root"), "{base_url}");
    }
}

#[test]
fn validates_catalog_entries_and_detaches_defaults() {
    let cases = [
        (
            DeepSeekCatalogModel {
                id: String::new(),
                ..Default::default()
            },
            "ids must be non-empty",
        ),
        (
            DeepSeekCatalogModel {
                id: "m".into(),
                name: Some(String::new()),
                ..Default::default()
            },
            "empty name",
        ),
        (
            DeepSeekCatalogModel {
                id: "m".into(),
                input_modalities: Some(vec![]),
                ..Default::default()
            },
            "must not be empty",
        ),
        (
            DeepSeekCatalogModel {
                id: "m".into(),
                input_modalities: Some(vec!["audio".into()]),
                ..Default::default()
            },
            "only \"text\" and \"image\"",
        ),
        (
            DeepSeekCatalogModel {
                id: "m".into(),
                input_modalities: Some(vec!["text".into(), "text".into()]),
                ..Default::default()
            },
            "must not contain duplicates",
        ),
        (
            DeepSeekCatalogModel {
                id: "m".into(),
                system_prompt_update: Some("leading-only".into()),
                ..Default::default()
            },
            "systemPromptUpdate",
        ),
        (
            DeepSeekCatalogModel {
                id: "m".into(),
                tool_update: Some("replace".into()),
                ..Default::default()
            },
            "toolUpdate",
        ),
    ];
    for (model, expected) in cases {
        let error = resolve_adapter_options(&Options {
            models: Some(vec![model]),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{expected}: {error}");
    }

    let mut resolved = resolve_adapter_options(&Options::default()).unwrap();
    resolved.models[0].name = Some("changed".into());
    assert_ne!(
        resolve_adapter_options(&Options::default()).unwrap().models[0]
            .name
            .as_deref(),
        Some("changed")
    );
}

#[test]
fn rejects_removed_image_detail_from_deserialized_configuration() {
    for image_detail in [json!("low"), json!(null)] {
        let options: Options = serde_json::from_value(json!({
            "models": [{
                "id": "vision",
                "inputModalities": ["image"],
                "imageDetail": image_detail
            }]
        }))
        .unwrap();
        assert_eq!(
            resolve_adapter_options(&options).unwrap_err().to_string(),
            "llm-deepseek: catalog model imageDetail is no longer supported; use imagePixelBudget"
        );
    }
}

fn image(name: &str, width: u64, height: u64, bytes: u64) -> ImageAttachmentRef {
    ImageAttachmentRef {
        attachment_id: format!("sha256:{name:0<64}").into(),
        media_type: ImageMediaType::Png,
        bytes,
        width,
        height,
        name: Some(name.into()),
        original_dimensions: None,
    }
}

#[test]
fn prices_text_only_and_projected_images() {
    let connection = resolve_adapter_options(&Options {
        models: Some(vec![
            vision_model(),
            DeepSeekCatalogModel {
                id: "text".into(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    })
    .unwrap();
    let source = image("photo", 1_920, 1_080, 1_024);
    let block = ImageBlock {
        attachment: source.clone(),
        offloaded: None,
    };

    let unlisted = deep_seek_image_request_pricing(&connection, "unlisted", None)
        .price_images(std::slice::from_ref(&block));
    let text = deep_seek_image_request_pricing(&connection, "text", None)
        .price_images(std::slice::from_ref(&block));
    assert_eq!(unlisted[0].visual_tokens, 0);
    assert_eq!(text[0].visual_tokens, 0);

    let vision = deep_seek_image_request_pricing(&connection, "vision", None)
        .price_images(std::slice::from_ref(&block));
    assert_eq!(vision[0].visual_tokens, 968);
    assert!(vision[0].text.contains("1708x961"));

    let offloaded =
        deep_seek_image_request_pricing(&connection, "vision", None).price_images(&[ImageBlock {
            attachment: source,
            offloaded: Some(true),
        }]);
    assert_eq!(offloaded[0].visual_tokens, 0);
}

#[test]
fn applies_numeric_and_low_image_projection_budgets() {
    for (budget, expected) in [
        (ImagePixelBudget::Pixels(640_000), (800, 800, 422)),
        (ImagePixelBudget::Low(ImageLow::Low), (512, 512, 184)),
    ] {
        let model = DeepSeekCatalogModel {
            image_pixel_budget: Some(budget),
            ..vision_model()
        };
        let target = resolve_request_image_target(&model, &image("square", 4_096, 4_096, 1));
        assert_eq!((target.width, target.height), (expected.0, expected.1));
        assert_eq!(
            deep_seek_image_tokens(target.width, target.height),
            expected.2
        );
    }

    for (width, height, expected_width, expected_height) in
        [(8_192, 1, 4_096, 1), (1, 8_192, 1, 4_096)]
    {
        let target =
            resolve_request_image_target(&vision_model(), &image("thin", width, height, 1));
        assert_eq!(
            (target.width, target.height),
            (expected_width, expected_height)
        );
    }
}

#[test]
fn catalog_listing_layers_and_refusals_cover_declared_capabilities() {
    let supported = resolve_adapter_options(&Options::default()).unwrap().models;
    let listing = parse_model_listing(&json!({"data":[
        {"id":"deepseek-flash","name":"Listed","context_window":77,"max_output_tokens":50,"input_modalities":["text"],"api_capabilities":{"anthropic_messages":{"system_prompt_update":"leading-only","tool_update":"leading-only"}}},
        {"id":"vouched","client_requirements":{"deepseek-harness":{"min_version":"0.1.0"}},"api_capabilities":{"anthropic_messages":{"tool_update":"addition-only"}}},
        {"id":"unknown","api_capabilities":{"anthropic_messages":{}}},
        {"id":"future","client_requirements":{"deepseek-harness":{"min_version":"9.0.0"}},"api_capabilities":{"anthropic_messages":{}}}
    ]}))
    .unwrap();
    assert!(matches!(
        listing.entries[0].capabilities,
        ListedCapabilities::Usable { .. }
    ));
    let result = resolve_listed_catalog(ListedCatalogInput {
        listing: Some(&listing.entries),
        supported: &supported,
        overrides: &IndexMap::new(),
        route_max_tokens: 40,
        client_version: "0.2.0",
    });
    let known = result
        .offered
        .iter()
        .find(|model| model.id == "deepseek-flash")
        .unwrap();
    assert_eq!(known.name.as_deref(), Some("Listed"));
    assert_eq!(known.max_tokens, Some(40));
    assert_eq!(
        known.input_modalities.as_deref(),
        Some(["text".to_owned()].as_slice())
    );
    assert!(known.image_max_bytes.is_none());
    let vouched = result
        .offered
        .iter()
        .find(|model| model.id == "vouched")
        .unwrap();
    assert_eq!(vouched.tool_update.as_deref(), Some("addition-only"));
    assert_eq!(
        result.refusals.get("unknown"),
        Some(&CatalogRefusal::Unsupported)
    );
    assert!(matches!(
        result.refusals.get("future"),
        Some(CatalogRefusal::UpgradeRequired { .. })
    ));
}
