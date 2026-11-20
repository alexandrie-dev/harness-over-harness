use std::{path::Path, sync::Arc};

use hoh_error::HarnessError;
use hoh_kernel::{Context, Kernel, Plugin};
use hoh_skill::{SkillRegistry, SkillViewOptions, Skills};
use hoh_skill_office::{
    Config, NAME, PROVIDER_NAME, PythonEnvironment, SKILL_NAMES, SkillOfficePlugin,
    USER_PYTHON_SECTION, WORKSPACE_DEPENDENCIES_SECTION,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn packaged_assets() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}

fn copy_assets(target: &Path) {
    for name in SKILL_NAMES {
        let directory = target.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::copy(
            packaged_assets().join(name).join("SKILL.md"),
            directory.join("SKILL.md"),
        )
        .unwrap();
    }
}

fn strip_frontmatter(raw: &str) -> &str {
    let after_open = raw
        .strip_prefix("---\r\n")
        .or_else(|| raw.strip_prefix("---\n"))
        .unwrap();
    let end = after_open.find("\n---").unwrap();
    let after_header = &after_open[end + 4..];
    after_header
        .strip_prefix("\r\n")
        .or_else(|| after_header.strip_prefix('\n'))
        .unwrap_or(after_header)
        .trim()
}

async fn registry_context() -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx)
}

async fn snapshot(ctx: &Context) -> (bool, Vec<String>) {
    let snapshot = ctx
        .get::<Skills>()
        .unwrap()
        .snapshot(Arc::new(SkillViewOptions::default()))
        .await
        .unwrap();
    (
        snapshot.complete,
        snapshot
            .skills
            .iter()
            .map(|skill| skill.text("name").unwrap().to_owned())
            .collect(),
    )
}

async fn get(ctx: &Context, name: &str) -> Option<hoh_skill::SkillDefinition> {
    ctx.get::<Skills>()
        .unwrap()
        .get(name, Arc::new(SkillViewOptions::default()))
        .await
        .unwrap()
}

#[test]
fn metadata_and_config_defaults_match_the_source() {
    assert_eq!(SkillOfficePlugin::NAME, NAME);
    assert_eq!(NAME, "skill-office");
    assert_eq!(PROVIDER_NAME, "dsh-office");
    let config = Config::default();
    assert!(config.asset_root.is_none());
    assert_eq!(
        config.python_environment,
        PythonEnvironment::WorkspaceDependencies
    );
}

#[tokio::test]
async fn packaged_catalog_loads_exact_bodies_and_disposal_removes_candidates() {
    let (kernel, ctx) = registry_context().await;
    let fiber = ctx.plugin::<SkillOfficePlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    let catalog = ctx
        .get::<Skills>()
        .unwrap()
        .snapshot(Arc::new(SkillViewOptions::default()))
        .await
        .unwrap();
    assert!(catalog.complete);
    assert_eq!(
        catalog
            .skills
            .iter()
            .map(|skill| skill.text("name").unwrap())
            .collect::<Vec<_>>(),
        SKILL_NAMES
    );
    for skill in &catalog.skills {
        assert!(skill.get("rank").is_none());
        assert!(skill.get("locator").is_none());
        assert!(skill.text("description").unwrap().len() <= 500);
    }

    for name in SKILL_NAMES {
        let definition = get(&ctx, name).await.unwrap();
        assert_eq!(definition.text("provider"), Some(PROVIDER_NAME));
        assert_eq!(definition.text("source"), Some("bundled"));
        assert_eq!(
            definition.get("invocation"),
            Some(&json!({"modelInvocable":true,"userInvocable":true}))
        );
        assert_eq!(
            definition.get("resourceBase"),
            Some(&json!({
                "kind":"directory",
                "path":packaged_assets().join(name).to_string_lossy(),
            }))
        );
        let raw = std::fs::read_to_string(packaged_assets().join(name).join("SKILL.md")).unwrap();
        let mut expected = strip_frontmatter(&raw).to_owned();
        if name != "office" {
            expected.push_str("\n\n## Python environment\n\n");
            expected.push_str(WORKSPACE_DEPENDENCIES_SECTION);
        }
        assert_eq!(definition.text("content"), Some(expected.as_str()));
        assert!(definition.get("rank").is_none());
        assert!(definition.get("locator").is_none());
    }

    fiber.dispose().await;
    assert_eq!(snapshot(&ctx).await, (true, vec![]));
    kernel.shutdown().await;
}

#[tokio::test]
async fn asset_root_dot_segments_are_normalized_for_resources_and_diagnostics() {
    for suffix in ["parent/../assets", "./assets"] {
        let root = TempDir::new().unwrap();
        std::fs::create_dir_all(root.path().join("parent")).unwrap();
        let assets = root.path().join("assets");
        copy_assets(&assets);
        let spelled = format!("{}/{suffix}", root.path().to_string_lossy());
        let (kernel, ctx) = registry_context().await;
        ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":spelled}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let definition = get(&ctx, "office-docx").await.unwrap();
        assert_eq!(
            definition.get("resourceBase"),
            Some(&json!({
                "kind": "directory",
                "path": assets.join("office-docx").to_string_lossy(),
            }))
        );

        std::fs::remove_file(assets.join("office-docx/SKILL.md")).unwrap();
        assert!(get(&ctx, "office-docx").await.is_none());
        let warning = ctx
            .logger()
            .buffer()
            .into_iter()
            .find(|record| record.message.contains("skill \"office-docx\""))
            .unwrap();
        assert!(
            warning.message.contains(
                assets
                    .join("office-docx/SKILL.md")
                    .to_string_lossy()
                    .as_ref()
            ),
            "{}",
            warning.message
        );
        assert!(!warning.message.contains("/../"), "{}", warning.message);
        assert!(!warning.message.contains("/./"), "{}", warning.message);
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn relocated_unicode_assets_and_python_environment_variants_are_exact() {
    for (environment, section) in [
        ("workspace-dependencies", WORKSPACE_DEPENDENCIES_SECTION),
        ("user", USER_PYTHON_SECTION),
    ] {
        let root = TempDir::new().unwrap();
        let assets = root.path().join("中文 assets");
        copy_assets(&assets);
        let (kernel, ctx) = registry_context().await;
        ctx.plugin::<SkillOfficePlugin>(json!({
            "assetRoot":assets,
            "pythonEnvironment":environment,
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();
        for name in ["office-docx", "office-pptx", "office-xlsx"] {
            let definition = get(&ctx, name).await.unwrap();
            assert!(definition.text("content").unwrap().ends_with(section));
            assert_eq!(
                definition.get("resourceBase"),
                Some(&json!({
                    "kind":"directory",
                    "path":assets.join(name).to_string_lossy(),
                }))
            );
        }
        assert!(
            !get(&ctx, "office")
                .await
                .unwrap()
                .text("content")
                .unwrap()
                .contains("## Python environment")
        );
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn config_rejects_unknown_environment_empty_and_relative_asset_roots() {
    let (kernel, ctx) = registry_context().await;
    for (config, expected) in [
        (json!({"pythonEnvironment":"system"}), "pythonEnvironment"),
        (json!({"assetRoot":""}), "assetRoot"),
        (json!({"assetRoot":"assets"}), "absolute directory"),
    ] {
        let error = ctx
            .plugin::<SkillOfficePlugin>(config)
            .unwrap()
            .settled()
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(snapshot(&ctx).await, (true, vec![]));
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn quoted_crlf_and_folded_frontmatter_preserve_body_newlines() {
    for (header, newline, description) in [
        (
            "description: \"Quoted: description\"",
            "\r\n",
            "Quoted: description",
        ),
        (
            "description: >-\n  Folded\n  description",
            "\n",
            "Folded description",
        ),
        (
            "base: &description Office workflow\ndescription: *description",
            "\n",
            "Office workflow",
        ),
        ("!!map\ndescription: Tagged mapping", "\n", "Tagged mapping"),
        ("description: !!str 3", "\n", "3"),
        ("description: 1_000", "\n", "1_000"),
        ("description: 0b101", "\n", "0b101"),
        ("description: -0x1F", "\n", "-0x1F"),
        ("description: +0x1F", "\n", "+0x1F"),
        ("description: 1_0.5", "\n", "1_0.5"),
    ] {
        let root = TempDir::new().unwrap();
        copy_assets(root.path());
        let body = "# Office instructions\n\ndescription: instruction text";
        let content = format!("---\n{header}\n---\n\n{body}\n").replace('\n', newline);
        std::fs::write(root.path().join("office-docx/SKILL.md"), content).unwrap();
        let (kernel, ctx) = registry_context().await;
        ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":root.path()}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let definition = get(&ctx, "office-docx").await.unwrap();
        assert_eq!(definition.text("description"), Some(description));
        assert_eq!(
            definition.text("content"),
            Some(
                format!(
                    "{}\n\n## Python environment\n\n{}",
                    body.replace('\n', newline),
                    WORKSPACE_DEPENDENCIES_SECTION
                )
                .as_str()
            )
        );
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn yaml_metadata_accepts_js_object_keys_nonfinite_numbers_and_custom_tags() {
    for header in [
        "description: Office workflow\n1: y",
        "description: Office workflow\ntrue: y",
        "description: Office workflow\nupper: .inf\nlower: -.Inf\nmissing: .nan",
        "description: Office workflow\nfoo: !custom bar",
        "description: Office workflow\n? [a, b]\n: x",
        "description: Office workflow\n1: a\n\"1\": b",
        "description: Office workflow\nb: !!binary YQ==",
        "description: Office workflow\nb: !!set\n  ? a",
    ] {
        let root = TempDir::new().unwrap();
        copy_assets(root.path());
        std::fs::write(
            root.path().join("office-docx/SKILL.md"),
            format!("---\n{header}\n---\n# Body\n"),
        )
        .unwrap();
        let (kernel, ctx) = registry_context().await;
        ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":root.path()}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let definition = get(&ctx, "office-docx").await.unwrap();
        assert_eq!(definition.text("description"), Some("Office workflow"));
        assert!(definition.text("content").unwrap().starts_with("# Body"));
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn skill_body_uses_javascript_trim_for_bom_whitespace() {
    let root = TempDir::new().unwrap();
    copy_assets(root.path());
    std::fs::write(
        root.path().join("office/SKILL.md"),
        "---\ndescription: Office workflow\n---\n\u{feff}# Body\u{feff}",
    )
    .unwrap();
    let (kernel, ctx) = registry_context().await;
    ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":root.path()}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert_eq!(
        get(&ctx, "office").await.unwrap().text("content"),
        Some("# Body")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn persistent_missing_and_invalid_metadata_are_complete_omissions() {
    for (replacement, expected_warning) in [
        (None, "ENOENT"),
        (Some("# Missing frontmatter\n"), "has no YAML frontmatter"),
        (Some("---\n\n---\n# Body\n"), "has no description"),
        (Some("---\nnull\n---\n# Body\n"), "has no description"),
        (Some("---\nscalar\n---\n# Body\n"), "has no description"),
        (
            Some("---\nname: office-docx\n---\n# Body\n"),
            "has no description",
        ),
        (
            Some("---\ndescription: 3\n---\n# Body\n"),
            "has no description",
        ),
        (
            Some("---\ndescription: 0x3\n---\n# Body\n"),
            "has no description",
        ),
        (
            Some("---\ndescription: \"\"\n---\n# Body\n"),
            "has no description",
        ),
        (
            Some("---\ndescription: [unclosed\n---\n# Body\n"),
            "YAMLParseError",
        ),
    ] {
        let root = TempDir::new().unwrap();
        copy_assets(root.path());
        let path = root.path().join("office-docx/SKILL.md");
        if let Some(replacement) = replacement {
            std::fs::write(&path, replacement).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        let (kernel, ctx) = registry_context().await;
        ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":root.path()}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        assert_eq!(
            snapshot(&ctx).await,
            (
                true,
                vec!["office", "office-pptx", "office-xlsx"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );
        let warnings = ctx
            .logger()
            .buffer()
            .into_iter()
            .filter(|record| record.message.contains("skill-office: skill"))
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains(expected_warning));
        if replacement.is_none() {
            assert!(
                warnings[0].message.starts_with(
                    "skill-office: skill \"office-docx\" is unavailable: Error: ENOENT"
                )
            );
            assert!(warnings[0].message.contains("SKILL.md"));
        } else if expected_warning == "YAMLParseError" {
            assert!(warnings[0].message.starts_with(
                "skill-office: skill \"office-docx\" is unavailable: YAMLParseError:"
            ));
            assert!(
                !warnings[0]
                    .message
                    .contains("unavailable: Error: YAMLParseError:")
            );
        }
        assert!(get(&ctx, "office-xlsx").await.is_some());
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn deleted_or_malformed_skill_after_discovery_invalidates_the_catalog() {
    for malformed in [false, true] {
        let root = TempDir::new().unwrap();
        copy_assets(root.path());
        let (kernel, ctx) = registry_context().await;
        ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":root.path()}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        assert_eq!(snapshot(&ctx).await.1.len(), 4);
        let path = root.path().join("office-docx/SKILL.md");
        if malformed {
            std::fs::write(path, "---\ndescription: [unclosed\n---\n# Body\n").unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
        }
        assert!(get(&ctx, "office-docx").await.is_none());
        assert_eq!(
            snapshot(&ctx).await,
            (
                true,
                vec!["office", "office-pptx", "office-xlsx"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn transient_read_failure_is_incomplete_recovers_and_get_rethrows() {
    let root = TempDir::new().unwrap();
    copy_assets(root.path());
    let path = root.path().join("office-docx/SKILL.md");
    let original = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let (kernel, ctx) = registry_context().await;
    ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":root.path()}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert_eq!(
        snapshot(&ctx).await,
        (
            false,
            vec!["office", "office-pptx", "office-xlsx"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        )
    );
    assert_eq!(
        ctx.logger()
            .buffer()
            .iter()
            .filter(|record| record.message.contains("skill-office: skill"))
            .count(),
        1
    );
    assert!(!snapshot(&ctx).await.0);
    assert_eq!(
        ctx.logger()
            .buffer()
            .iter()
            .filter(|record| record.message.contains("skill-office: skill"))
            .count(),
        1,
        "a repeated identical transient failure is warned once"
    );
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, &original).unwrap();
    assert_eq!(snapshot(&ctx).await.1.len(), 4);

    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let error = ctx
        .get::<Skills>()
        .unwrap()
        .get("office-docx", Arc::new(SkillViewOptions::default()))
        .await
        .unwrap_err();
    let error = HarnessError::of(error.as_ref()).expect("Node filesystem errors remain typed");
    assert_eq!(error.name(), "Error");
    assert_eq!(error.code(), "EISDIR");
    assert!(error.message().contains(", read '"), "{}", error.message());
    assert_eq!(
        ctx.logger()
            .buffer()
            .iter()
            .filter(|record| record.message.contains("skill-office: skill"))
            .count(),
        2,
        "a successful read clears failure deduplication"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_utf8_is_lossily_decoded_like_node_instead_of_becoming_transient_io() {
    let root = TempDir::new().unwrap();
    copy_assets(root.path());
    let path = root.path().join("office-docx/SKILL.md");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\nMalformed byte: \xff\n");
    std::fs::write(&path, bytes).unwrap();
    let (kernel, ctx) = registry_context().await;
    ctx.plugin::<SkillOfficePlugin>(json!({"assetRoot":root.path()}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert_eq!(snapshot(&ctx).await.1.len(), 4);
    let content = get(&ctx, "office-docx")
        .await
        .unwrap()
        .text("content")
        .unwrap()
        .to_owned();
    assert!(content.contains("Malformed byte: \u{fffd}"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn packaged_guidance_preserves_progressive_budgets_and_tool_contracts() {
    let (kernel, ctx) = registry_context().await;
    ctx.plugin::<SkillOfficePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let route = get(&ctx, "office").await.unwrap();
    assert_eq!(
        route.text("description"),
        Some(
            "Create or edit Office files; inspect Office files and PDFs. Load before rendering, conversion, or recalculation to enable office_render, office_convert, and office_recalculate next step."
        )
    );
    assert!(
        route
            .text("description")
            .unwrap()
            .chars()
            .count()
            .div_ceil(4)
            <= 50
    );
    assert!(route.text("content").unwrap().chars().count().div_ceil(4) <= 200);
    for (name, previous_tokens) in [
        ("office-docx", 1_688),
        ("office-pptx", 1_879),
        ("office-xlsx", 2_330),
    ] {
        let content = get(&ctx, name)
            .await
            .unwrap()
            .text("content")
            .unwrap()
            .to_owned();
        assert!(content.chars().count().div_ceil(4) <= previous_tokens * 7 / 10);
        assert!(!content.contains("read_image"));
        assert!(!content.contains("dsoffice"));
        assert!(content.contains("`office_render`"));
        assert!(content.contains("\"Content and structure checks passed; visual inspection was not completed.\" or \"内容与结构检查通过；视觉检查未完成。\""));
    }
    for name in SKILL_NAMES {
        let content = get(&ctx, name)
            .await
            .unwrap()
            .text("content")
            .unwrap()
            .to_owned();
        assert!(content.contains("If `office_render`, `office_convert`, or `office_recalculate` is not in your tools, load `office` with `skill`; they appear next step."));
        for forbidden in [
            "operation:",
            "`office` tool",
            "office-render",
            "office-recalculate",
            "office-export-pdf",
            "LibreOffice Kit CLI",
            "--output-dir",
            "\"<node>\"",
        ] {
            assert!(!content.contains(forbidden), "{name}: {forbidden}");
        }
    }
    let route = get(&ctx, "office")
        .await
        .unwrap()
        .text("content")
        .unwrap()
        .to_owned();
    for expected in [
        "`office_render`",
        "`office_recalculate`",
        "`office_convert` with `format`",
        "`format: \"xlsx\"` or `\"ods\"`",
    ] {
        assert!(route.contains(expected), "{expected}");
    }
    assert!(
        get(&ctx, "office-xlsx")
            .await
            .unwrap()
            .text("content")
            .unwrap()
            .contains("`office_recalculate` with `format: \"xlsx\"`")
    );
    for (format, legacy, library) in [
        ("docx", "doc", "python-docx"),
        ("xlsx", "xls", "openpyxl"),
        ("pptx", "ppt", "python-pptx"),
    ] {
        let expected = format!(
            "{library} cannot read legacy `.{legacy}` files. Use `office_convert` with `format: \"{format}\"`, copy the result into the workspace, then edit the copy."
        );
        assert!(
            get(&ctx, &format!("office-{format}"))
                .await
                .unwrap()
                .text("content")
                .unwrap()
                .contains(&expected),
            "{format}: {expected}"
        );
    }
    kernel.shutdown().await;
}

#[test]
fn locale_assets_are_complete_json_metadata() {
    for locale in ["de", "en", "es", "fr", "ja", "pt", "ru", "zh"] {
        let raw = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("locale")
                .join(format!("{locale}.json")),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert!(value.pointer("/meta/title").unwrap().as_str().is_some());
        assert!(
            value
                .pointer("/meta/description")
                .unwrap()
                .as_str()
                .is_some()
        );
    }
}
