use hoh_error::HarnessError;
use hoh_kernel::Kernel;
use hoh_llm::ContentBlock;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortController;
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tool_workspace_dependencies::{
    PrimaryRuntimeManifest, ToolWorkspaceDependencies, WorkspaceDependencyError,
    install_primary_runtime, parse_primary_runtime, read_primary_runtime, resolve_primary_runtime,
    workspace_dependency_paths,
};
use hoh_tools::{
    GenericCallView, ToolCallKind, ToolCallView, ToolExecutionInput, ToolRuntime, Tools,
};
use hoh_util_values::{js_json_stringify, js_json_stringify_pretty};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn host_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        value => value,
    }
}

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        value => value,
    }
}

fn manifest() -> PrimaryRuntimeManifest {
    PrimaryRuntimeManifest {
        desktop_version: "1.0.0".into(),
        platform: host_platform().into(),
        arch: host_arch().into(),
        payload_digest: None,
        python: "3.12.14".into(),
        node: Some("24.21.0".into()),
        pnpm: Some("11.7.0".into()),
        python_packages: IndexMap::from([
            ("python-docx".into(), "1.2.0".into()),
            ("python-pptx".into(), "1.0.2".into()),
            ("openpyxl".into(), "3.1.5".into()),
        ]),
    }
}

struct Fixture {
    root: TempDir,
    source: String,
    installed: String,
    manifest: PrimaryRuntimeManifest,
}

impl Fixture {
    async fn new() -> Self {
        let root = TempDir::new().unwrap();
        let source = root.path().join("resources").to_string_lossy().into_owned();
        let installed = root
            .path()
            .join("home/dsh-runtimes/dsh-primary-runtime")
            .to_string_lossy()
            .into_owned();
        let manifest = manifest();
        let paths = workspace_dependency_paths(&source, &manifest);
        for path in [
            &paths.python,
            paths.node.as_ref().unwrap(),
            paths.pnpm.as_ref().unwrap(),
        ] {
            tokio::fs::create_dir_all(Path::new(path).parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(path, "interpreter").await.unwrap();
        }
        tokio::fs::create_dir_all(&paths.python_packages)
            .await
            .unwrap();
        tokio::fs::create_dir_all(paths.node_packages.as_ref().unwrap())
            .await
            .unwrap();
        write_manifest(&source, &manifest).await;
        Self {
            root,
            source,
            installed,
            manifest,
        }
    }
}

async fn write_value(root: &str, value: &Value) {
    tokio::fs::create_dir_all(root).await.unwrap();
    tokio::fs::write(
        Path::new(root).join("runtime.json"),
        js_json_stringify(value),
    )
    .await
    .unwrap();
}

async fn write_manifest(root: &str, manifest: &PrimaryRuntimeManifest) {
    write_value(root, &serde_json::to_value(manifest).unwrap()).await;
}

fn legacy(manifest: &PrimaryRuntimeManifest) -> Value {
    json!({
        "desktopVersion": manifest.desktop_version,
        "platform": manifest.platform,
        "arch": manifest.arch,
        "components": {
            "python": manifest.python,
            "node": manifest.node,
            "pnpm": manifest.pnpm,
            "numpy": "2.3.5",
            "pandas": "3.0.1"
        },
        "pythonPackages": manifest.python_packages
    })
}

fn assert_code(error: &(dyn std::error::Error + 'static), code: &str) {
    let error = HarnessError::of(error).expect("workspace dependency HarnessError");
    assert!(error.is_named(WorkspaceDependencyError::NAME));
    assert_eq!(error.code(), code);
}

#[test]
fn returns_each_platforms_interpreter_and_package_paths() {
    for platform in ["win32", "darwin", "linux"] {
        let mut manifest = manifest();
        manifest.platform = platform.into();
        manifest.arch = "x64".into();
        manifest.python_packages.clear();
        let paths = workspace_dependency_paths("/runtime", &manifest);
        assert!(paths.python_distributions.is_empty());
        assert_eq!(
            paths.python,
            if platform == "win32" {
                "/runtime/dependencies/python/python.exe"
            } else {
                "/runtime/dependencies/python/bin/python3"
            }
        );
        assert_eq!(
            paths.python_packages,
            if platform == "win32" {
                "/runtime/dependencies/python/Lib/site-packages"
            } else {
                "/runtime/dependencies/python/lib/python3.12/site-packages"
            }
        );
    }
}

#[test]
fn rejects_malformed_flat_legacy_and_locked_metadata() {
    let valid = serde_json::to_value(manifest()).unwrap();
    for invalid in [
        Value::Null,
        json!([]),
        json!({"desktopVersion":""}),
        json!({"platform":"unsupported"}),
        json!({"arch":"ia32"}),
    ] {
        assert!(
            parse_primary_runtime(&invalid)
                .unwrap_err()
                .to_string()
                .contains("invalid metadata")
        );
    }
    for (field, value) in [
        ("python", json!(3)),
        ("node", json!(24)),
        ("pnpm", json!("latest")),
        ("payloadDigest", json!(1)),
        ("pythonPackages", Value::Null),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(parse_primary_runtime(&invalid).is_err(), "field {field}");
    }
    for invalid in [
        json!({"payloadDigest":"invalid"}),
        json!({"pythonPackages":["python-docx"]}),
        json!({"pythonPackages":{"python-docx":"../escape"}}),
        json!({"pythonPackages":{"../escape":"1.2.0"}}),
        json!({"pythonPackages":{"numpy":"2.3.5","Numpy":"2.3.5"}}),
        json!({"pythonPackages":{"typing_extensions":"4.16.0","typing.extensions":"4.16.0"}}),
    ] {
        let mut candidate = valid.clone();
        for (key, value) in invalid.as_object().unwrap() {
            candidate[key] = value.clone();
        }
        assert!(parse_primary_runtime(&candidate).is_err());
    }
    for components in [
        Value::Null,
        json!([]),
        json!("invalid"),
        json!({"python":"3.12.14"}),
    ] {
        let mut candidate = legacy(&manifest());
        candidate["components"] = components;
        assert!(parse_primary_runtime(&candidate).is_err());
    }
}

#[test]
fn normalizes_legacy_metadata_and_rejects_conflicts_or_mixed_fields() {
    let manifest = manifest();
    let parsed = parse_primary_runtime(&legacy(&manifest)).unwrap();
    assert_eq!(parsed, manifest);
    for (distribution, name) in [
        ("numpy", "numpy"),
        ("pandas", "pandas"),
        ("Numpy", "numpy"),
        ("PANDAS", "pandas"),
    ] {
        let mut candidate = legacy(&manifest);
        candidate["pythonPackages"] = Value::Object(serde_json::Map::from_iter([(
            distribution.to_owned(),
            json!("0.0.1"),
        )]));
        assert!(
            parse_primary_runtime(&candidate)
                .unwrap_err()
                .to_string()
                .contains(&format!("conflicting {name} distribution version"))
        );
        let mut candidate = legacy(&manifest);
        let version = if name == "numpy" { "2.3.5" } else { "3.0.1" };
        candidate["pythonPackages"] = Value::Object(serde_json::Map::from_iter([(
            distribution.to_owned(),
            json!(version),
        )]));
        let mut expected = manifest.clone();
        expected.python_packages = IndexMap::from([(distribution.to_owned(), version.to_owned())]);
        assert_eq!(parse_primary_runtime(&candidate).unwrap(), expected);
    }
    for field in ["python", "node", "pnpm"] {
        let mut candidate = legacy(&manifest);
        candidate[field] = json!("1.0.0");
        assert!(parse_primary_runtime(&candidate).is_err());
    }
    let mut without_node = serde_json::to_value(&manifest).unwrap();
    without_node.as_object_mut().unwrap().remove("node");
    assert!(parse_primary_runtime(&without_node).is_err());
    let mut legacy_without_node = legacy(&manifest);
    legacy_without_node["components"]
        .as_object_mut()
        .unwrap()
        .remove("node");
    assert!(parse_primary_runtime(&legacy_without_node).is_err());
}

#[tokio::test]
async fn legacy_normalization_does_not_rewrite_and_equivalent_flat_source_reuses_installation() {
    let item = Fixture::new().await;
    let mut stored = legacy(&item.manifest);
    stored.as_object_mut().unwrap().remove("pythonPackages");
    let text = js_json_stringify(&stored);
    tokio::fs::write(Path::new(&item.source).join("runtime.json"), &text)
        .await
        .unwrap();
    let normalized = read_primary_runtime(&item.source).await.unwrap();
    assert!(normalized.python_packages.is_empty());
    assert_eq!(
        tokio::fs::read_to_string(Path::new(&item.source).join("runtime.json"))
            .await
            .unwrap(),
        text
    );

    write_value(&item.source, &legacy(&item.manifest)).await;
    let installed = install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    let user = Path::new(&installed.python_packages).join("user-package.py");
    tokio::fs::write(&user, "keep").await.unwrap();
    write_manifest(&item.source, &item.manifest).await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    assert_eq!(tokio::fs::read_to_string(user).await.unwrap(), "keep");
    let installed_text = tokio::fs::read_to_string(Path::new(&item.installed).join("runtime.json"))
        .await
        .unwrap();
    assert!(installed_text.contains("\"components\""));
}

#[tokio::test]
async fn accepts_arbitrary_flat_python_distributions_without_special_library_fields() {
    let item = Fixture::new().await;
    let mut flat = item.manifest.clone();
    flat.python_packages = IndexMap::from([("custom-library".into(), "1.0.0".into())]);
    write_manifest(&item.source, &flat).await;
    assert_eq!(read_primary_runtime(&item.source).await.unwrap(), flat);
    assert_eq!(
        resolve_primary_runtime(&item.source)
            .await
            .unwrap()
            .python_distributions,
        flat.python_packages
    );
}

#[cfg(unix)]
#[tokio::test]
async fn upgraded_python_commands_remain_relative_after_install_relocation() {
    let item = Fixture::new().await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    let paths = workspace_dependency_paths(&item.source, &item.manifest);
    let bin = Path::new(&paths.python).parent().unwrap();
    tokio::fs::rename(bin.join("python3"), bin.join("python3.12"))
        .await
        .unwrap();
    for name in ["python", "python3"] {
        tokio::fs::symlink("python3.12", bin.join(name))
            .await
            .unwrap();
    }
    let mut changed = item.manifest.clone();
    changed.payload_digest = Some("a".repeat(64));
    write_manifest(&item.source, &changed).await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    let moved = item.root.path().join("moved runtime with spaces");
    tokio::fs::rename(&item.installed, &moved).await.unwrap();
    tokio::fs::remove_dir_all(&item.source).await.unwrap();
    let relocated = resolve_primary_runtime(moved.to_str().unwrap())
        .await
        .unwrap();
    let bin = Path::new(&relocated.python).parent().unwrap();
    for name in ["python", "python3"] {
        assert_eq!(
            tokio::fs::read_link(bin.join(name)).await.unwrap(),
            PathBuf::from("python3.12")
        );
        assert_eq!(
            tokio::fs::read_to_string(bin.join(name)).await.unwrap(),
            "interpreter"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_escaping_python_aliases_during_copy_reuse_and_in_place_preparation() {
    for mode in ["copy", "reuse", "in-place"] {
        let item = Fixture::new().await;
        if mode == "reuse" {
            install_primary_runtime(&item.source, &item.installed)
                .await
                .unwrap();
        }
        let payload = if mode == "reuse" {
            &item.installed
        } else {
            &item.source
        };
        let python = workspace_dependency_paths(payload, &item.manifest).python;
        let bin = Path::new(&python).parent().unwrap();
        let outside = bin.parent().unwrap().join("outside-python");
        tokio::fs::rename(&python, &outside).await.unwrap();
        tokio::fs::symlink("../outside-python", &python)
            .await
            .unwrap();
        let error = if mode == "in-place" {
            resolve_primary_runtime(&item.source).await.unwrap_err()
        } else {
            install_primary_runtime(&item.source, &item.installed)
                .await
                .unwrap_err()
        };
        assert!(error.to_string().contains("invalid Python alias"));
        assert_code(error.as_ref(), "WORKSPACE_DEPENDENCY_INVALID_PAYLOAD");
    }
}

#[tokio::test]
async fn installs_offline_reuses_the_release_and_preserves_user_packages() {
    let item = Fixture::new().await;
    let installed = install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    let user = Path::new(&installed.python_packages).join("user-package.py");
    tokio::fs::write(&user, "user content").await.unwrap();
    assert_eq!(
        install_primary_runtime(&item.source, &item.installed)
            .await
            .unwrap(),
        installed
    );
    assert_eq!(
        tokio::fs::read_to_string(user).await.unwrap(),
        "user content"
    );
}

#[tokio::test]
async fn replaces_changed_locked_payload_and_recovers_an_interrupted_swap() {
    let item = Fixture::new().await;
    let mut first = item.manifest.clone();
    first.payload_digest = Some("a".repeat(64));
    first.python_packages = IndexMap::from([("python-docx".into(), "1.1.2".into())]);
    write_manifest(&item.source, &first).await;
    let source_file = Path::new(&workspace_dependency_paths(&item.source, &first).python_packages)
        .join("library.py");
    tokio::fs::write(&source_file, "first").await.unwrap();
    let installed = install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    let old_package = Path::new(&installed.python_packages).join("old-package.py");
    tokio::fs::write(&old_package, "old dependency")
        .await
        .unwrap();
    first.payload_digest = Some("b".repeat(64));
    first.python_packages = IndexMap::from([("python-docx".into(), "1.2.0".into())]);
    write_manifest(&item.source, &first).await;
    tokio::fs::write(&source_file, "second").await.unwrap();
    let new_package = Path::new(&workspace_dependency_paths(&item.source, &first).python_packages)
        .join("new-package.py");
    tokio::fs::write(&new_package, "new dependency")
        .await
        .unwrap();
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    assert_eq!(read_primary_runtime(&item.installed).await.unwrap(), first);
    assert_eq!(
        tokio::fs::read_to_string(Path::new(&installed.python_packages).join("library.py"))
            .await
            .unwrap(),
        "second"
    );
    assert_eq!(
        tokio::fs::read_to_string(Path::new(&installed.python_packages).join("new-package.py"))
            .await
            .unwrap(),
        "new dependency"
    );
    assert!(tokio::fs::read(old_package).await.is_err());
    tokio::fs::rename(&item.installed, format!("{}.previous", item.installed))
        .await
        .unwrap();
    first.desktop_version = "2.0.0".into();
    write_manifest(&item.source, &first).await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    assert_eq!(
        read_primary_runtime(&item.installed)
            .await
            .unwrap()
            .desktop_version,
        "2.0.0"
    );
}

#[tokio::test]
async fn upgrades_an_installed_release_manifest_that_has_no_payload_digest() {
    let item = Fixture::new().await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    let mut locked = item.manifest.clone();
    locked.payload_digest = Some("a".repeat(64));
    write_manifest(&item.source, &locked).await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    assert_eq!(
        read_primary_runtime(&item.installed)
            .await
            .unwrap()
            .payload_digest,
        locked.payload_digest
    );
}

#[tokio::test]
async fn keeps_the_installed_release_when_the_replacement_is_incomplete() {
    let item = Fixture::new().await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    let mut next = item.manifest.clone();
    next.desktop_version = "2.0.0".into();
    write_manifest(&item.source, &next).await;
    tokio::fs::remove_file(workspace_dependency_paths(&item.source, &next).python)
        .await
        .unwrap();
    assert!(
        install_primary_runtime(&item.source, &item.installed)
            .await
            .is_err()
    );
    assert_eq!(
        read_primary_runtime(&item.installed)
            .await
            .unwrap()
            .desktop_version,
        "1.0.0"
    );
}

#[tokio::test]
async fn removes_a_stale_regular_previous_entry_before_publication() {
    let item = Fixture::new().await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    tokio::fs::write(format!("{}.previous", item.installed), "stale")
        .await
        .unwrap();
    let mut next = item.manifest.clone();
    next.desktop_version = "2.0.0".into();
    write_manifest(&item.source, &next).await;
    install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap();
    assert_eq!(
        read_primary_runtime(&item.installed)
            .await
            .unwrap()
            .desktop_version,
        "2.0.0"
    );
    assert!(
        tokio::fs::symlink_metadata(format!("{}.previous", item.installed))
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_linked_current_and_previous_installation_directories() {
    for previous in [false, true] {
        let item = Fixture::new().await;
        if previous {
            install_primary_runtime(&item.source, &item.installed)
                .await
                .unwrap();
        }
        let outside = item.root.path().join("outside");
        tokio::fs::create_dir(&outside).await.unwrap();
        tokio::fs::write(outside.join("keep"), "untouched")
            .await
            .unwrap();
        tokio::fs::create_dir_all(Path::new(&item.installed).parent().unwrap())
            .await
            .unwrap();
        let link = if previous {
            format!("{}.previous", item.installed)
        } else {
            item.installed.clone()
        };
        tokio::fs::symlink(&outside, &link).await.unwrap();
        let error = install_primary_runtime(&item.source, &item.installed)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("filesystem link"));
        assert_eq!(
            tokio::fs::read_to_string(outside.join("keep"))
                .await
                .unwrap(),
            "untouched"
        );
    }
}

#[tokio::test]
async fn rejects_wrong_payload_entry_types_in_place_copy_and_reuse() {
    for mode in ["in-place", "copy", "reuse"] {
        for field in ["python", "node", "pnpm", "pythonPackages", "nodePackages"] {
            let item = Fixture::new().await;
            if mode == "reuse" {
                install_primary_runtime(&item.source, &item.installed)
                    .await
                    .unwrap();
            }
            let payload = if mode == "reuse" {
                &item.installed
            } else {
                &item.source
            };
            let paths = workspace_dependency_paths(payload, &item.manifest);
            let (path, directory) = match field {
                "python" => (paths.python, false),
                "node" => (paths.node.unwrap(), false),
                "pnpm" => (paths.pnpm.unwrap(), false),
                "pythonPackages" => (paths.python_packages, true),
                _ => (paths.node_packages.unwrap(), true),
            };
            if directory {
                tokio::fs::remove_dir_all(&path).await.unwrap();
                tokio::fs::write(&path, "not a directory").await.unwrap();
            } else {
                tokio::fs::remove_file(&path).await.unwrap();
                tokio::fs::create_dir(&path).await.unwrap();
            }
            let error = if mode == "in-place" {
                resolve_primary_runtime(&item.source).await.unwrap_err()
            } else {
                install_primary_runtime(&item.source, &item.installed)
                    .await
                    .unwrap_err()
            };
            assert!(error.to_string().contains(if directory {
                "expected directory at"
            } else {
                "expected file at"
            }));
            assert_code(error.as_ref(), "WORKSPACE_DEPENDENCY_INVALID_PAYLOAD");
        }
    }
}

#[tokio::test]
async fn malformed_missing_and_incompatible_metadata_keep_typed_causes() {
    let item = Fixture::new().await;
    let mut incompatible = item.manifest.clone();
    incompatible.arch = if host_arch() == "x64" { "arm64" } else { "x64" }.into();
    write_manifest(&item.source, &incompatible).await;
    let error = install_primary_runtime(&item.source, &item.installed)
        .await
        .unwrap_err();
    assert_code(error.as_ref(), "WORKSPACE_DEPENDENCY_UNSUPPORTED_PLATFORM");
    tokio::fs::write(Path::new(&item.source).join("runtime.json"), "{")
        .await
        .unwrap();
    let error = read_primary_runtime(&item.source).await.unwrap_err();
    assert!(error.to_string().contains("not valid JSON"));
    assert_code(error.as_ref(), "WORKSPACE_DEPENDENCY_INVALID_PAYLOAD");
    tokio::fs::remove_file(Path::new(&item.source).join("runtime.json"))
        .await
        .unwrap();
    let error = read_primary_runtime(&item.source).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "primary runtime: runtime.json is missing"
    );
    assert_code(error.as_ref(), "WORKSPACE_DEPENDENCY_INVALID_PAYLOAD");
}

#[tokio::test]
async fn uses_python_only_payload_in_place_without_copying() {
    let item = Fixture::new().await;
    let manifest = PrimaryRuntimeManifest {
        node: None,
        pnpm: None,
        python_packages: IndexMap::from([("python-docx".into(), "1.2.0".into())]),
        ..item.manifest.clone()
    };
    write_manifest(&item.source, &manifest).await;
    let paths = resolve_primary_runtime(&item.source).await.unwrap();
    assert!(paths.node.is_none());
    assert!(paths.pnpm.is_none());
    assert!(paths.node_packages.is_none());
    assert_eq!(paths.python_distributions, manifest.python_packages);
    assert_eq!(
        serde_json::to_value(&paths)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["python", "pythonPackages", "pythonDistributions"]
    );
    assert!(!Path::new(&item.installed).exists());
}

async fn tool_fixture(
    source: &str,
    root: Option<&str>,
) -> (Kernel, hoh_kernel::Context, hoh_kernel::Fiber) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let config = root.map_or_else(
        || json!({"source":source}),
        |root| json!({"source":source,"root":root}),
    );
    let fiber = ctx.plugin::<ToolWorkspaceDependencies>(config).unwrap();
    fiber.settled().await.unwrap();
    (kernel, ctx, fiber)
}

async fn execute(ctx: &hoh_kernel::Context, id: &str) -> hoh_tools::ToolExecutionResult {
    execute_tool_fixture(
        ctx,
        ToolExecutionInput::new(
            id.into(),
            "load_workspace_dependencies",
            json!({}),
            AbortController::new().signal(),
        ),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn real_tool_retries_failure_renders_exact_json_and_unregisters() {
    let item = Fixture::new().await;
    let metadata = tokio::fs::read(Path::new(&item.source).join("runtime.json"))
        .await
        .unwrap();
    tokio::fs::remove_file(Path::new(&item.source).join("runtime.json"))
        .await
        .unwrap();
    let (kernel, ctx, fiber) = tool_fixture(&item.source, Some(&item.installed)).await;
    assert!(execute(&ctx, "failed").await.is_error());
    let failed = execute(&ctx, "failed-again").await;
    assert_eq!(
        failed.error().unwrap().info.as_ref().unwrap().name,
        "WorkspaceDependencyError"
    );
    assert_eq!(
        failed.error().unwrap().info.as_ref().unwrap().code,
        "WORKSPACE_DEPENDENCY_INVALID_PAYLOAD"
    );
    tokio::fs::write(Path::new(&item.source).join("runtime.json"), metadata)
        .await
        .unwrap();
    let result = execute(&ctx, "success").await;
    assert!(!result.is_error());
    let value = result.value().unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "python",
            "node",
            "nodePackages",
            "pnpm",
            "pythonPackages",
            "pythonDistributions"
        ]
    );
    assert_eq!(
        result.content(),
        &[ContentBlock::text(js_json_stringify_pretty(value, 2))]
    );
    let tools = ctx.get::<Tools>().unwrap();
    let definition = tools
        .get("load_workspace_dependencies", None, None)
        .unwrap();
    assert_eq!(definition.name, "load_workspace_dependencies");
    assert_eq!(
        definition.parameters,
        json!({"type":"object","properties":{}})
            .as_object()
            .unwrap()
            .clone()
    );
    assert_eq!(
        definition.output.schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        [
            "python",
            "node",
            "pnpm",
            "pythonPackages",
            "nodePackages",
            "pythonDistributions"
        ]
    );
    assert_eq!(
        definition.present_call.as_ref().unwrap()(&json!({})),
        Some(ToolCallView::Generic(GenericCallView {
            title: "Load workspace dependencies".into(),
            kind: Some(ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: None,
        }))
    );
    fiber.dispose().await;
    assert!(
        tools
            .get("load_workspace_dependencies", None, None)
            .is_none()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn tool_preserves_unsupported_platform_errors() {
    let item = Fixture::new().await;
    let mut incompatible = item.manifest.clone();
    incompatible.platform = if host_platform() == "linux" {
        "darwin".into()
    } else {
        "linux".into()
    };
    write_manifest(&item.source, &incompatible).await;
    let (kernel, ctx, _fiber) = tool_fixture(&item.source, Some(&item.installed)).await;
    let result = execute(&ctx, "platform").await;
    let error = result.error().unwrap();
    assert_eq!(
        error.message,
        "primary runtime: incompatible platform or architecture"
    );
    let info = error.info.as_ref().unwrap();
    assert_eq!(info.name, "WorkspaceDependencyError");
    assert_eq!(info.code, "WORKSPACE_DEPENDENCY_UNSUPPORTED_PLATFORM");
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn tool_wraps_a_linked_installation_as_preparation_failed() {
    let item = Fixture::new().await;
    tokio::fs::create_dir_all(Path::new(&item.installed).parent().unwrap())
        .await
        .unwrap();
    let outside = item.root.path().join("outside-tool-installation");
    tokio::fs::create_dir(&outside).await.unwrap();
    tokio::fs::symlink(&outside, &item.installed).await.unwrap();
    let (kernel, ctx, _fiber) = tool_fixture(&item.source, Some(&item.installed)).await;
    let result = execute(&ctx, "linked").await;
    let error = result.error().unwrap();
    assert_eq!(
        error.message,
        format!(
            "primary runtime: preparation failed: primary runtime: installation path is a filesystem link: {}",
            item.installed
        )
    );
    let info = error.info.as_ref().unwrap();
    assert_eq!(info.name, "WorkspaceDependencyError");
    assert_eq!(info.code, "WORKSPACE_DEPENDENCY_PREPARATION_FAILED");
    kernel.shutdown().await;
}

#[tokio::test]
async fn concurrent_tool_calls_share_one_successful_preparation() {
    let item = Fixture::new().await;
    let (kernel, ctx, _fiber) = tool_fixture(&item.source, None).await;
    let (first, second) = tokio::join!(execute(&ctx, "first"), execute(&ctx, "second"));
    assert_eq!(first.value(), second.value());
    assert!(!first.is_error());
    kernel.shutdown().await;
}

#[tokio::test]
async fn unused_disposal_never_prepares_the_payload_and_relative_config_fails() {
    let item = Fixture::new().await;
    let (kernel, _ctx, fiber) = tool_fixture(&item.source, Some(&item.installed)).await;
    fiber.dispose().await;
    assert!(!Path::new(&item.installed).exists());
    kernel.shutdown().await;

    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    for config in [
        json!({"source":"relative"}),
        json!({"source":item.source,"root":"relative"}),
    ] {
        let fiber = ctx.plugin::<ToolWorkspaceDependencies>(config).unwrap();
        let error = fiber.settled().await.unwrap_err();
        assert!(error.to_string().contains("must be absolute paths"));
    }
    kernel.shutdown().await;
}

#[test]
fn plugin_schema_rejects_missing_empty_and_non_string_sources() {
    let definition = hoh_kernel::PluginDef::of::<ToolWorkspaceDependencies>();
    for value in [
        json!({}),
        json!({"source":""}),
        json!({"source":1}),
        json!({"source":"/absolute","root":null}),
    ] {
        assert!(hoh_kernel::config::validate(definition.schema(), &value).is_err());
    }
}
