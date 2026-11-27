#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::Duration;

use hoh_external_harness::{AuthState, ExternalHarness, StartParams, VersionStatus, code};
use hoh_harness_codex::{CodexConfig, CodexHarness};
use tempfile::TempDir;

const STUB: &str = env!("CARGO_BIN_EXE_codex-stub");
const HOST: &str = "codex-code-mode-host";
// Process supervision forks. Keep it from inheriting another test's writable stub file.
static INSTALLATION_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn harness(path: &Path, codex_home: &Path) -> CodexHarness {
    CodexHarness::new(CodexConfig {
        extra_env: vec![
            ("PATH".into(), path.to_str().unwrap().into()),
            ("CODEX_HOME".into(), codex_home.to_str().unwrap().into()),
            ("CODEX_STUB_VERSION".into(), "codex-cli 0.166.0".into()),
            ("CODEX_STUB_SCENARIO".into(), "discover".into()),
            (
                "CODEX_STUB_LOG".into(),
                codex_home.with_extension("log").to_str().unwrap().into(),
            ),
        ],
        request_timeout: Duration::from_secs(3),
        ..CodexConfig::default()
    })
}

fn start_params(temp: &TempDir) -> StartParams {
    StartParams {
        session_id: "hoh-installation-test".into(),
        cwd: temp.path().to_str().unwrap().into(),
        cwd_identity: None,
        model: None,
        effort: None,
        interaction_mode: Default::default(),
        policy: Default::default(),
        resume: None,
    }
}

#[tokio::test]
async fn discovery_skips_an_unpaired_native_cli_and_uses_the_standalone_package() {
    let _serial = INSTALLATION_TESTS.lock().await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("path");
    fs::create_dir(&path).unwrap();
    let incomplete_cli = path.join("codex");
    fs::copy(STUB, &incomplete_cli).unwrap();

    let codex_home = temp.path().join("codex-home");
    let package = codex_home.join("packages/standalone/current");
    let bin = package.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        package.join("codex-package.json"),
        r#"{"version":"0.166.0"}"#,
    )
    .unwrap();
    fs::copy(STUB, bin.join("codex")).unwrap();
    fs::hard_link(bin.join("codex"), bin.join(HOST)).unwrap();

    let (found, status) = harness(&path, &codex_home)
        .discover_with_status()
        .await
        .unwrap();

    assert!(found.installed);
    assert_eq!(status, VersionStatus::Supported, "{:?}", found.detail);
    assert_eq!(found.auth, AuthState::Authenticated);
    assert_eq!(found.models[0].id, "gpt-stub");
    let log = fs::read_to_string(codex_home.with_extension("log")).unwrap();
    let launch: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(
        launch["program"],
        bin.join("codex").canonicalize().unwrap().to_str().unwrap()
    );
}

#[tokio::test]
async fn discovery_accepts_a_symlink_to_a_macos_bundle_with_a_packaged_helper() {
    let _serial = INSTALLATION_TESTS.lock().await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("path");
    fs::create_dir(&path).unwrap();

    let package = temp.path().join("codex-package");
    let bundle_bin = package.join("CodexCLI.app/Contents/MacOS");
    let resources = package.join("codex-resources");
    fs::create_dir_all(&bundle_bin).unwrap();
    fs::create_dir(&resources).unwrap();
    fs::create_dir(package.join("bin")).unwrap();
    fs::write(package.join("codex-package.json"), "{}").unwrap();
    fs::copy(STUB, bundle_bin.join("codex")).unwrap();
    fs::hard_link(bundle_bin.join("codex"), resources.join(HOST)).unwrap();
    symlink(bundle_bin.join("codex"), path.join("codex")).unwrap();

    let (found, status) = harness(&path, &temp.path().join("codex-home"))
        .discover_with_status()
        .await
        .unwrap();
    assert_eq!(status, VersionStatus::Supported, "{:?}", found.detail);
    assert_eq!(found.models[0].id, "gpt-stub");
}

#[tokio::test]
async fn an_unpaired_native_cli_reports_the_missing_helper_in_discovery_and_startup() {
    let _serial = INSTALLATION_TESTS.lock().await;
    let temp = TempDir::new().unwrap();
    fs::copy(STUB, temp.path().join("codex")).unwrap();
    let harness = harness(temp.path(), &temp.path().join("codex-home"));

    let found = harness.discover().await.unwrap();
    let detail = found.detail.unwrap();
    assert!(detail.contains(HOST), "{detail}");
    assert!(detail.contains("complete Codex CLI package"), "{detail}");
    assert!(found.models.is_empty());

    let result = harness.start_or_resume(start_params(&temp)).await;
    let error = match result {
        Ok(_) => panic!("started Codex without its Code Mode host"),
        Err(error) => error,
    };
    assert_eq!(error.code, code::NOT_INSTALLED);
    assert_eq!(error.message, detail);

    let explicit = temp.path().join("codex-aarch64-apple-darwin");
    fs::rename(temp.path().join("codex"), &explicit).unwrap();
    let harness = CodexHarness::new(CodexConfig {
        executable: explicit,
        ..CodexConfig::default()
    });
    assert_eq!(harness.discover().await.unwrap().detail, Some(detail));
}

#[tokio::test]
async fn an_older_cli_without_the_code_mode_host_feature_can_start() {
    let _serial = INSTALLATION_TESTS.lock().await;
    let temp = TempDir::new().unwrap();
    let executable = temp.path().join("codex");
    fs::copy(STUB, &executable).unwrap();
    let harness = CodexHarness::new(CodexConfig {
        executable,
        extra_env: vec![("CODEX_STUB_LEGACY".into(), "1".into())],
        ..CodexConfig::default()
    });

    let (discovery, status) = harness.discover_with_status().await.unwrap();
    assert_eq!(status, VersionStatus::Supported, "{:?}", discovery.detail);
    assert_eq!(discovery.models[0].id, "gpt-stub");
    let started = harness.start_or_resume(start_params(&temp)).await.unwrap();
    started.session.terminate(Duration::ZERO).await;
}
