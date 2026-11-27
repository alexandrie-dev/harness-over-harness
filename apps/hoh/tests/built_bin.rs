use hoh_loader::{Entry, parse_entries};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Output, Stdio},
    time::Duration,
};
fn command(root: &Path, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hoh"));
    command
        .args(args)
        .env_clear()
        .env("HOME", root)
        .env("DSH_HOME", root.join("home"))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}
async fn run(root: &Path, args: &[&str]) -> Output {
    tokio::time::timeout(Duration::from_secs(60), command(root, args).output())
        .await
        .unwrap()
        .unwrap()
}
fn ok(result: &Output) -> String {
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        result.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout.clone()).unwrap()
}
fn rows(text: &str) -> Vec<Entry> {
    parse_entries(text, Path::new("/temporary-dump.yml")).unwrap()
}
#[cfg(unix)]
#[tokio::test]
async fn runs_through_an_installed_style_symlink() {
    let root = tempfile::tempdir().unwrap();
    let link = root.path().join("hoh-installed");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_hoh"), &link).unwrap();
    let output = tokio::process::Command::new(link)
        .arg("--version")
        .env_clear()
        .env("HOME", root.path())
        .env("DSH_HOME", root.path().join("home"))
        .current_dir(root.path())
        .output()
        .await
        .unwrap();
    assert_eq!(ok(&output), concat!(env!("CARGO_PKG_VERSION"), "\n"));
}
#[tokio::test]
async fn prints_its_complete_launcher_help_before_loading_the_environment() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "DSH_HOME=/forbidden\n").unwrap();
    assert_eq!(
        ok(&run(root.path(), &["--help"]).await)
            .strip_suffix('\n')
            .unwrap(),
        include_str!("expected/launcher-help.txt")
    );
    assert!(!root.path().join("home/profiles").exists());
}
#[tokio::test]
async fn requires_a_profile_and_rejects_removed_flags() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec![],
        vec!["--config", "obsolete.yml"],
        vec!["--port", "3000"],
    ] {
        let result = run(root.path(), &args).await;
        assert_eq!(result.status.code(), Some(1));
        assert!(!result.stderr.is_empty());
        assert!(!root.path().join("home/profiles").exists());
    }
}
#[tokio::test]
async fn does_not_load_a_project_environment_for_version() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "DSH_HOME=/forbidden\n").unwrap();
    let output = ok(&run(root.path(), &["--version"]).await);
    assert_eq!(output, concat!(env!("CARGO_PKG_VERSION"), "\n"));
    assert!(!root.path().join("home/profiles").exists());
}
#[tokio::test]
async fn prints_version_without_home_or_dsh_home() {
    let root = tempfile::tempdir().unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_hoh"))
        .arg("--version")
        .env_clear()
        .current_dir(root.path())
        .output()
        .await
        .unwrap();
    assert_eq!(ok(&output), concat!(env!("CARGO_PKG_VERSION"), "\n"));
}

#[tokio::test]
async fn reports_overlay_read_and_parse_errors_identically_for_boot_and_dump() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing-overlay.yml");
    let expected = format!(
        "dsh: failed to read overlay {}: Error: ENOENT: no such file or directory, open '{}'\n",
        missing.display(),
        missing.display()
    );
    for tail in [vec![], vec!["--dump-config"]] {
        let mut args = vec!["headless", "--patch", "missing-overlay.yml"];
        args.extend(tail);
        let result = run(root.path(), &args).await;
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert_eq!(String::from_utf8(result.stderr).unwrap(), expected);
    }

    let malformed = root.path().join("malformed-overlay.yml");
    std::fs::write(&malformed, "[\n").unwrap();
    let mut errors = vec![];
    for tail in [vec![], vec!["--dump-config"]] {
        let mut args = vec!["headless", "--patch", "malformed-overlay.yml"];
        args.extend(tail);
        let result = run(root.path(), &args).await;
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.starts_with(&format!(
                "dsh: failed to parse overlay {}:",
                malformed.display()
            )),
            "{error}"
        );
        errors.push(error);
    }
    assert_eq!(errors[0], errors[1]);
}

#[tokio::test]
async fn reports_optional_patch_read_errors_identically_for_boot_and_dump() {
    let root = tempfile::tempdir().unwrap();
    let patches = root.path().join("home/cordis.patch.yml");
    std::fs::create_dir_all(&patches).unwrap();
    let mut errors = vec![];
    for tail in [vec![], vec!["--dump-config"]] {
        let mut args = vec!["headless"];
        args.extend(tail);
        let result = run(root.path(), &args).await;
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.starts_with(&format!(
                "dsh: failed to read patches {}: Error: EISDIR:",
                patches.display()
            )),
            "{error}"
        );
        errors.push(error);
    }
    assert_eq!(errors[0], errors[1]);
}

#[tokio::test]
async fn dump_reads_the_home_patch_before_explicit_overlays() {
    let root = tempfile::tempdir().unwrap();
    let patches = root.path().join("home/cordis.patch.yml");
    std::fs::create_dir_all(&patches).unwrap();
    let result = run(
        root.path(),
        &[
            "headless",
            "--patch",
            "missing-overlay.yml",
            "--dump-config",
        ],
    )
    .await;
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .starts_with(&format!(
                "dsh: failed to read patches {}: Error: EISDIR:",
                patches.display()
            ))
    );
}

#[tokio::test]
async fn reports_structural_overlay_and_patch_errors_with_the_source_text() {
    let root = tempfile::tempdir().unwrap();
    let overlay = root.path().join("structural.yml");
    for (text, expected) in [
        (
            "key: value\n",
            format!(
                "dsh: overlay {} must be a top-level YAML array of loader patch entries\n",
                overlay.display()
            ),
        ),
        (
            "- scalar\n",
            format!(
                "dsh: overlay entry 1 in {} must be a mapping (a loader patch entry)\n",
                overlay.display()
            ),
        ),
    ] {
        std::fs::write(&overlay, text).unwrap();
        let result = run(root.path(), &["headless", "--patch", "structural.yml"]).await;
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert_eq!(String::from_utf8(result.stderr).unwrap(), expected);
    }

    let patches = root.path().join("home/cordis.patch.yml");
    for (text, expected) in [
        (
            "key: value\n",
            format!(
                "dsh: patches {} must be a top-level YAML array of loader patch entries\n",
                patches.display()
            ),
        ),
        (
            "- scalar\n",
            format!(
                "dsh: patches entry 1 in {} must be a mapping (a loader patch entry)\n",
                patches.display()
            ),
        ),
    ] {
        std::fs::write(&patches, text).unwrap();
        let result = run(root.path(), &["headless", "--dump-config"]).await;
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert_eq!(String::from_utf8(result.stderr).unwrap(), expected);
    }
}

#[tokio::test]
async fn reads_overlays_before_home_patches_and_installs_proxy_before_either() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(home.join("cordis.patch.yml")).unwrap();
    let missing = root.path().join("missing-overlay.yml");
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        command(root.path(), &["headless", "--patch", "missing-overlay.yml"])
            .env("HTTPS_PROXY", "invalid-proxy")
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "dsh: HTTPS_PROXY is not a valid URL; connecting directly\n\
dsh: failed to read overlay {}: Error: ENOENT: no such file or directory, open '{}'\n",
            missing.display(),
            missing.display()
        )
    );
}
#[tokio::test]
async fn fails_loud_on_a_nonexistent_profile_with_the_plugin_command_hint() {
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), &["--profile", "nonexistent"]).await;
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(stderr.contains("plugin"), "{stderr}");
}
#[tokio::test]
async fn prints_the_web_profile_bundle_layers_without_a_user_layer() {
    let root = tempfile::tempdir().unwrap();
    let output = ok(&run(root.path(), &["web", "--dump-default-config"]).await);
    let tree = rows(&output);
    assert!(
        tree.iter()
            .any(|r| r.name == "@deepseek-ai/dsh-agent-loop" && r.config["agents"] == json!([]))
    );
    assert!(
        tree.iter()
            .any(|r| r.name == "@deepseek-ai/dsh-host-webserver")
    );
    assert!(output.contains("@deepseek-ai/dsh-base"));
    assert!(!root.path().join("home/profiles/node_modules").exists());
}
#[tokio::test]
async fn creates_a_custom_profile_from_a_shipped_template_before_printing_it() {
    let root = tempfile::tempdir().unwrap();
    let output = ok(&run(
        root.path(),
        &[
            "--profile",
            "rescue",
            "--from-default-profile",
            "web",
            "--dump-default-config",
        ],
    )
    .await);
    assert!(output.contains("@deepseek-ai/dsh-web-app"));
    assert!(
        root.path()
            .join("home/profiles/rescue/package.json")
            .exists()
    );
}
#[tokio::test]
async fn rejects_an_unknown_source_before_creating_the_target_profile() {
    let root = tempfile::tempdir().unwrap();
    let result = run(
        root.path(),
        &[
            "--profile",
            "rescue",
            "--from-default-profile",
            "unknown",
            "--dump-default-config",
        ],
    )
    .await;
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("unknown default profile \"unknown\""));
    assert!(error.contains("\"web\""));
    assert!(!root.path().join("home/profiles/rescue").exists());
}
#[tokio::test]
async fn prints_the_headless_profile_without_host_or_browser_layers() {
    let root = tempfile::tempdir().unwrap();
    let output = ok(&run(root.path(), &["--profile", "headless", "--dump-config"]).await);
    let tree = rows(&output);
    assert!(tree.iter().any(|r| r.name == "@deepseek-ai/dsh-headless"));
    assert!(
        !tree
            .iter()
            .any(|r| r.name.starts_with("@deepseek-ai/dsh-host-")
                || r.name.starts_with("@deepseek-ai/dsh-client-")
                || r.name == "@deepseek-ai/dsh-web-app")
    );
}
#[tokio::test]
async fn window_profiles_share_host_plugins_and_select_the_native_execution_bundle() {
    let root = tempfile::tempdir().unwrap();
    let native = rows(&ok(&run(
        root.path(),
        &["--profile", "window", "--dump-config"],
    )
    .await));
    let external = rows(&ok(&run(
        root.path(),
        &["--profile", "window-external", "--dump-config"],
    )
    .await));

    let native_ids = [
        "agent-loop",
        "bash-sandbox",
        "destructive-guard",
        "sandbox",
        "shell-env",
        "subprocess",
        "tool-ask-user",
        "tool-bash",
        "tool-executor-native",
        "tool-fs",
        "tool-todo",
        "tool-working-directory",
    ];
    for id in native_ids {
        assert!(native.iter().any(|row| row.id == id), "window lacks {id}");
        assert!(
            external.iter().all(|row| row.id != id),
            "window-external includes {id}"
        );
    }
    assert_eq!(native.len(), external.len() + native_ids.len());
    for row in &external {
        let original = native.iter().find(|old| old.id == row.id).unwrap();
        if row.id == "sdk-app-startup" {
            assert_eq!(original.config["profile"], "window");
            assert_eq!(row.config["profile"], "window-external");
        } else {
            assert_eq!(row, original);
        }
    }
    for (id, name) in [
        ("agent", "@deepseek-ai/dsh-agent"),
        ("filesystem", "@deepseek-ai/dsh-fs-sandbox"),
        ("harness-codex", "hoh:harness-codex"),
        (
            "session-persistence-jsonl",
            "@deepseek-ai/dsh-session-persistence-jsonl",
        ),
        ("privileged-approver", "hoh:window-privileged-approver"),
        ("sdk-app-startup", "@deepseek-ai/dsh-sdk-app"),
        ("plugin-inventory", "hoh:plugin-inventory"),
        ("author-turn-note", "hoh:plugin-turn-note"),
    ] {
        assert!(
            external.iter().any(|row| row.id == id && row.name == name),
            "window-external lacks {id}"
        );
    }
    assert!(
        root.path()
            .join("home/profiles/window-external/package.json")
            .is_file()
    );
}

#[tokio::test]
async fn prints_the_exact_standalone_sdk_minimal_tree_without_dsh_base() {
    let root = tempfile::tempdir().unwrap();
    let output = ok(&run(
        root.path(),
        &["--profile", "sdk-minimal", "--dump-default-config"],
    )
    .await);
    let tree = rows(&output);
    let expected = [
        ("sdk-app-startup", "sdk-app"),
        ("sdk-jsonrpc-server", "sdk-jsonrpc-server"),
        ("deepseek-llm-api-extensions", "deepseek-llm-api-extensions"),
        ("session-log-deepseek", "session-log-deepseek"),
        (
            "plugin-package-inventory-deepseek",
            "plugin-package-inventory-deepseek",
        ),
        ("llm-deepseek", "llm-deepseek-api-key"),
        ("sandbox", "sandbox-local"),
        ("filesystem", "fs-local"),
        ("attachment-local", "attachment-local"),
        ("tool-fs", "tool-fs"),
        ("working-directory", "working-directory"),
        ("tool-working-directory", "tool-working-directory"),
        ("session-projection", "session-projection"),
        ("token-meter", "token-meter"),
        ("compaction-basic", "compaction-basic"),
        ("image-offload", "compaction-image-offload"),
        ("sandbox-policy", "sandbox-policy"),
        ("subprocess", "subprocess-local"),
        ("pty", "terminal"),
        ("terminal-bash", "terminal-bash"),
        ("terminal-pwsh", "terminal-bash"),
        ("timer", "TIMER"),
        ("llm", "llm"),
        ("session", "session"),
        ("session-title", "session-title"),
        ("system-prompt", "system-prompt"),
        ("tools", "tools"),
        ("tool-executor-native", "tool-executor-native"),
        ("mcp-resources", "mcp-resources"),
        ("agent", "agent"),
        ("llm-retry", "llm-retry"),
        ("jobs", "jobs-local"),
        ("agent-loop", "agent-loop"),
        ("persistent-bash", "tool-bash-persistent"),
        ("persistent-pwsh", "tool-pwsh-persistent"),
        ("sessions", "session-persistence-jsonl"),
        ("session-checkpoint-policy", "session-checkpoint-policy"),
    ];
    assert_eq!(
        tree.iter()
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(id, name)| (
                id.to_string(),
                if *name == "TIMER" {
                    "@deepseek-ai/cordis-plugin-timer".into()
                } else {
                    format!("@deepseek-ai/dsh-{name}")
                }
            ))
            .collect::<Vec<_>>()
    );
    assert!(!output.contains("@deepseek-ai/dsh-base"));
    assert!(!output.contains("@deepseek-ai/dsh-web-app"));
}
#[tokio::test]
async fn composes_the_profile_user_layer_and_a_patch_overlay_in_order() {
    let root = tempfile::tempdir().unwrap();
    ok(&run(root.path(), &["web", "--dump-default-config"]).await);
    let profile = root.path().join("home/profiles/web/cordis.patch.yml");
    std::fs::write(&profile,"- id: agent-loop\n  config: {agents: [{id: personal, provider: personal-provider, model: personal-model}]}\n- id: absent-row\n  config: {x: 1}\n").unwrap();
    let overlay = root.path().join("overlay.yml");
    std::fs::write(&overlay,"- id: agent-loop\n  config: {agents: [{id: configured, provider: configured-provider, model: configured-model}]}\n").unwrap();
    let result = run(
        root.path(),
        &["web", "--patch", overlay.to_str().unwrap(), "--dump-config"],
    )
    .await;
    assert_eq!(result.status.code(), Some(0));
    let output = String::from_utf8(result.stdout).unwrap();
    assert_eq!(
        rows(&output)
            .iter()
            .find(|r| r.id == "agent-loop")
            .unwrap()
            .config,
        json!({"agents":[{"id":"configured","provider":"configured-provider","model":"configured-model"}]})
    );
    assert!(!output.contains("personal-provider"));
    assert!(output.contains(&format!("patched by profile, {}", overlay.display())));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("patch: entry \"absent-row\" not found")
    );
}
#[tokio::test]
async fn schema_dump_reports_the_compiled_packages_and_the_missing_packages_without_boot() {
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), &["headless", "--dump-config-schema"]).await;
    assert_eq!(result.status.code(), Some(1));
    let doc: Value =
        hoh_util_values::js_json_parse(std::str::from_utf8(&result.stdout).unwrap()).unwrap();
    assert_eq!(doc["x-cordis"]["complete"], false);
    let entries = doc["x-cordis"]["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["name"] == "@deepseek-ai/dsh-system-prompt" && e["status"] == "schema")
    );
    assert!(
        entries
            .iter()
            .any(|e| e["name"] == "@deepseek-ai/dsh-headless" && e["status"] == "schema")
    );
    // A package the build does not include stays reported as missing.
    assert!(
        entries
            .iter()
            .any(|e| e["name"] == "@deepseek-ai/dsh-jobs-local" && e["status"] == "error")
    );
    assert!(!root.path().join("home/logs").exists());
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn lets_a_profile_without_a_parser_ignore_app_arguments_and_dispose_on_a_signal() {
    // Tokio's Unix signal handler wakes its driver through a Unix self-pipe.
    use std::io::Write;
    let (_receiver, mut sender) = std::os::unix::net::UnixStream::pair().unwrap();
    sender.write_all(&[1]).expect("Unix signal self-pipe write");
    let root = tempfile::tempdir().unwrap();
    hoh_loader::init_profile(&root.path().join("home/profiles/empty"), &[]).unwrap();
    let mut child = command(root.path(), &["empty", "--anything", "task"])
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "signal handlers were not installed"
        );
        assert!(child.try_wait().unwrap().is_none());
        let status = tokio::fs::read_to_string(format!("/proc/{pid}/status"))
            .await
            .unwrap();
        let mask = status
            .lines()
            .find_map(|line| line.strip_prefix("SigCgt:\t"))
            .and_then(|s| u64::from_str_radix(s, 16).ok())
            .unwrap();
        if mask & (1 << 14) != 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
}

#[tokio::test]
async fn exits_one_when_app_exit_is_followed_by_a_row_failure() {
    let root = tempfile::tempdir().unwrap();
    hoh_loader::init_profile(&root.path().join("home/profiles/empty"), &[]).unwrap();
    let overlay = root.path().join("exit-then-fail.yml");
    std::fs::write(
        &overlay,
        "- insert: [{id: webserver, name: 'hoh:test-exit-then-fail'}]\n",
    )
    .unwrap();
    let result = command(
        root.path(),
        &["empty", "--patch", overlay.to_str().unwrap()],
    )
    .env("HOH_TEST_EXIT_THEN_FAIL", "1")
    .output()
    .await
    .unwrap();
    assert_eq!(
        result.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("failure after exit request"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
