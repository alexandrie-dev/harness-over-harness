use hoh_kernel::Kernel;
use hoh_llm::ToolCallId;
use hoh_session::SessionId;
use hoh_spill::{SaveTextSpill, SpillOwner, SpillSource, SpillStore};
use hoh_spill_local::cleanup::gather_sweep_roots;
use hoh_spill_local::{
    Config, DEFAULT_ROOT_PREFIX, LocalSpillStore, SaveTextOptions, SweepOptions, SweepRoot, WarnFn,
    discover_default_roots, encode_segment, is_errno, private_root, save_text_file, session_dir,
    sweep_spill_roots,
};
use std::{
    fs, io,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tempfile::TempDir;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn root() -> TempDir {
    let root = tempfile::Builder::new()
        .prefix("dsh-spill-test-")
        .tempdir()
        .unwrap();
    private_dir(root.path());
    root
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn warnings() -> (WarnFn, Arc<Mutex<Vec<String>>>) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let captured = messages.clone();
    let warn: WarnFn = Arc::new(move |message| captured.lock().unwrap().push(message));
    (warn, messages)
}

fn write_aged(path: &Path, content: &str, age: Duration) {
    fs::write(path, content).unwrap();
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(SystemTime::now().checked_sub(age).unwrap()))
        .unwrap();
}

fn cutoff(age: Duration) -> i64 {
    let time = SystemTime::now().checked_sub(age).unwrap();
    i64::try_from(
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn active(path: &Path) -> SweepRoot {
    SweepRoot {
        path: path.to_string_lossy().into_owned(),
        prune_when_empty: false,
    }
}

async fn sweep(roots: Vec<SweepRoot>, cutoff_ms: i64) -> Arc<Mutex<Vec<String>>> {
    let (warn, messages) = warnings();
    sweep_spill_roots(SweepOptions {
        roots,
        cutoff_ms,
        warn,
    })
    .await;
    messages
}

fn request() -> SaveTextSpill {
    SaveTextSpill {
        owner: SpillOwner {
            session_id: SessionId::from("sess-1"),
        },
        source: SpillSource::Tool {
            tool_name: "web_fetch".into(),
            call_id: ToolCallId::from("call-1"),
            label: "result".into(),
        },
        suggested_name: "web_fetch.txt".into(),
        content: "the full body".into(),
    }
}

#[test]
fn keeps_the_safe_set_literal() {
    assert_eq!(encode_segment("web_fetch.txt"), "web_fetch.txt");
    assert_eq!(encode_segment("a-B_9.z"), "a-B_9.z");
}

#[test]
fn escapes_separators_and_tilde() {
    assert_eq!(encode_segment("../etc/passwd"), "..~002Fetc~002Fpasswd");
    assert_eq!(encode_segment("a/b"), "a~002Fb");
    assert_eq!(encode_segment("~"), "~007E");
}

#[test]
fn escapes_the_whole_segment_dot_tokens() {
    assert_eq!(encode_segment("."), "~002E");
    assert_eq!(encode_segment(".."), "~002E~002E");
}

#[test]
fn encodes_the_empty_string_to_a_non_empty_segment() {
    assert_eq!(encode_segment(""), "~");
}

#[test]
fn is_a_stable_per_session_hash_under_the_root() {
    let directory = session_dir("/spill", "sess-1");
    assert_eq!(directory, session_dir("/spill", "sess-1"));
    assert_eq!(directory.parent().unwrap(), Path::new("/spill"));
    let name = directory.file_name().unwrap().to_string_lossy();
    let hash = name.strip_prefix("session-").unwrap();
    assert_eq!(hash.len(), 12);
    assert!(
        hash.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_ne!(directory, session_dir("/spill", "sess-2"));
}

#[tokio::test]
async fn writes_the_content_under_the_session_dir_and_reports_bytes() {
    let root = root();
    let saved = save_text_file(SaveTextOptions {
        root: root.path().to_string_lossy().into_owned(),
        session_id: "sess-1".into(),
        suggested_name: "r.txt".into(),
        content: "héllo".into(),
    })
    .await
    .unwrap();
    assert_eq!(fs::read_to_string(&saved.path).unwrap(), "héllo");
    assert_eq!(saved.bytes, 6);
    assert_eq!(
        Path::new(&saved.path).parent().unwrap(),
        session_dir(root.path(), "sess-1")
    );
    let file_name = Path::new(&saved.path)
        .file_name()
        .unwrap()
        .to_string_lossy();
    let (nonce, suggested_name) = file_name.split_once('-').unwrap();
    assert_eq!(nonce.len(), 12);
    assert!(
        nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(suggested_name, "r.txt");
}

#[tokio::test]
async fn sanitizes_a_traversal_shaped_suggested_name_into_one_segment() {
    let root = root();
    let saved = save_text_file(SaveTextOptions {
        root: root.path().to_string_lossy().into_owned(),
        session_id: "sess-1".into(),
        suggested_name: "../../evil".into(),
        content: "x".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        Path::new(&saved.path).parent().unwrap(),
        session_dir(root.path(), "sess-1")
    );
    assert!(!saved.path.contains("/.."));
}

#[cfg(unix)]
#[tokio::test]
async fn creates_the_session_directory_and_file_with_owner_only_posix_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let saved = save_text_file(SaveTextOptions {
        root: root.path().to_string_lossy().into_owned(),
        session_id: "sess-1".into(),
        suggested_name: "r.txt".into(),
        content: "x".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        fs::metadata(Path::new(&saved.path).parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&saved.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn gives_distinct_paths_to_two_saves_of_the_same_name() {
    let root = root();
    let options = |content: &str| SaveTextOptions {
        root: root.path().to_string_lossy().into_owned(),
        session_id: "sess-1".into(),
        suggested_name: "r.txt".into(),
        content: content.into(),
    };
    assert_ne!(
        save_text_file(options("a")).await.unwrap().path,
        save_text_file(options("b")).await.unwrap().path
    );
}

#[test]
fn private_root_is_a_stable_absolute_directory_under_the_temp_dir() {
    let first = private_root().unwrap();
    assert!(Path::new(&first).is_absolute());
    assert_eq!(private_root().unwrap(), first);
}

#[cfg(unix)]
#[test]
fn private_root_reports_mkdtemp_failure_in_node_form() {
    const CHILD_MARKER: &str = "HOH_SPILL_LOCAL_MKDTEMP_ERROR_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let missing = Path::new(&std::env::var_os("TMPDIR").unwrap()).to_path_buf();
        let error = private_root().unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "ENOENT: no such file or directory, mkdtemp '{}/{}XXXXXX'",
                missing.display(),
                DEFAULT_ROOT_PREFIX
            )
        );
        return;
    }

    let base = tempfile::tempdir().unwrap();
    let missing = base.path().join("missing");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "private_root_reports_mkdtemp_failure_in_node_form",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .env("TMPDIR", &missing)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn registers_as_spill_store_and_saves_under_the_configured_root() {
    let root = root();
    let kernel = Kernel::start();
    let fiber = kernel
        .root()
        .plugin::<LocalSpillStore>(serde_json::json!({
            "root": root.path(), "cleanupPeriodDays": 0
        }))
        .unwrap();
    fiber.settled().await.unwrap();
    let saved = kernel
        .root()
        .get::<SpillStore>()
        .unwrap()
        .save_text(request())
        .await
        .unwrap();
    assert_eq!(
        Path::new(saved.locator.as_str()).parent().unwrap(),
        session_dir(root.path(), "sess-1")
    );
    assert_eq!(saved.bytes, 13);
    assert_eq!(
        fs::read_to_string(saved.locator.as_str()).unwrap(),
        "the full body"
    );
    assert_eq!(
        saved.retrieval_hint,
        "Use read with offset/limit, or grep this path to search within it."
    );
    kernel.shutdown().await;
}

#[test]
fn resolves_a_relative_configured_root_to_absolute() {
    let store = LocalSpillStore::new(Config {
        root: Some(".".into()),
        cleanup_period_days: 0,
        ..Config::default()
    })
    .unwrap();
    assert!(Path::new(&store.root).is_absolute());
}

#[test]
fn falls_back_to_the_private_root_when_none_is_configured() {
    let store = LocalSpillStore::new(Config {
        root: None,
        cleanup_period_days: 0,
        ..Config::default()
    })
    .unwrap();
    assert_eq!(store.root, private_root().unwrap());
}

#[tokio::test]
async fn rejects_when_the_root_is_not_writable() {
    let root = root();
    let file = root.path().join("file");
    fs::write(&file, "x").unwrap();
    let kernel = Kernel::start();
    let fiber = kernel
        .root()
        .plugin::<LocalSpillStore>(serde_json::json!({
            "root": file, "cleanupPeriodDays": 0
        }))
        .unwrap();
    fiber.settled().await.unwrap();
    let error = kernel
        .root()
        .get::<SpillStore>()
        .unwrap()
        .save_text(request())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "ENOTDIR: not a directory, mkdir '{}'",
            session_dir(&file, "sess-1").display()
        )
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn reports_an_overlong_spill_name_in_node_fs_form() {
    let root = root();
    let suggested_name = "x".repeat(300);
    let error = save_text_file(SaveTextOptions {
        root: root.path().to_string_lossy().into_owned(),
        session_id: "s".into(),
        suggested_name: suggested_name.clone(),
        content: "x".into(),
    })
    .await
    .unwrap_err()
    .to_string();
    let prefix = format!(
        "ENAMETOOLONG: name too long, open '{}/",
        session_dir(root.path(), "s").display()
    );
    assert!(error.starts_with(&prefix), "{error}");
    assert!(error.ends_with(&format!("-{suggested_name}'")), "{error}");
}

#[test]
fn save_text_options_debug_redacts_content() {
    let options = SaveTextOptions {
        root: "/spill".into(),
        session_id: "session".into(),
        suggested_name: "secret.txt".into(),
        content: "token=do-not-log-this".into(),
    };
    let debug = format!("{options:?}");
    assert!(!debug.contains("do-not-log-this"), "{debug}");
    assert!(debug.contains("content_len: 21"), "{debug}");
}

#[tokio::test]
async fn rejects_a_negative_or_fractional_cleanup_period_at_load() {
    for value in [serde_json::json!(-1), serde_json::json!(1.5)] {
        let kernel = Kernel::start();
        let fiber = kernel
            .root()
            .plugin::<LocalSpillStore>(serde_json::json!({"cleanupPeriodDays": value}))
            .unwrap();
        assert!(fiber.settled().await.is_err());
        kernel.shutdown().await;
    }
}

#[test]
fn defaults_cleanup_period_days_to_30() {
    let config: Config = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(config.cleanup_period_days, 30);
    let null: Config =
        serde_json::from_value(serde_json::json!({"cleanupPeriodDays": null})).unwrap();
    assert_eq!(null.cleanup_period_days, 30);
    assert!(serde_json::from_value::<Config>(serde_json::json!({"root": null})).is_err());
}

#[test]
fn accepts_an_integral_float_cleanup_period_like_javascript() {
    let config: Config =
        serde_json::from_value(serde_json::json!({"cleanupPeriodDays": 30.0})).unwrap();
    assert_eq!(config.cleanup_period_days, 30);
}

#[test]
fn preserves_additional_config_keys_like_a_non_strict_schemastery_object() {
    let config: Config = serde_json::from_value(serde_json::json!({
        "cleanupPeriodDays": 0,
        "futureOption": { "enabled": true }
    }))
    .unwrap();
    assert_eq!(
        config.extra.get("futureOption"),
        Some(&serde_json::json!({ "enabled": true }))
    );
}

#[tokio::test]
async fn deletes_files_older_than_the_cutoff_and_keeps_fresh_ones() {
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    let old = directory.join("old");
    let fresh = directory.join("fresh");
    write_aged(&old, "old", DAY * 40);
    write_aged(&fresh, "fresh", DAY);
    sweep(vec![active(root.path())], cutoff(DAY * 30)).await;
    assert!(!old.exists());
    assert!(fresh.exists());
}

#[tokio::test]
async fn keeps_a_file_exactly_at_the_boundary() {
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    let boundary = directory.join("boundary");
    fs::write(&boundary, "x").unwrap();
    let modified = fs::metadata(&boundary).unwrap().modified().unwrap();
    let cutoff = i64::try_from(
        modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    sweep(vec![active(root.path())], cutoff).await;
    assert!(boundary.exists());
}

#[tokio::test]
async fn disabled_cleanup_sweeps_nothing() {
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    let old = directory.join("old");
    write_aged(&old, "old", DAY * 40);
    let kernel = Kernel::start();
    let fiber = kernel
        .root()
        .plugin::<LocalSpillStore>(serde_json::json!({"root": root.path(), "cleanupPeriodDays": 0}))
        .unwrap();
    fiber.settled().await.unwrap();
    kernel.shutdown().await;
    assert!(old.exists());
}

#[tokio::test]
async fn prunes_empty_active_session_directories() {
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    write_aged(&directory.join("old"), "old", DAY * 40);
    sweep(vec![active(root.path())], cutoff(DAY * 30)).await;
    assert!(!directory.exists());
    assert!(root.path().exists());
}

#[cfg(unix)]
#[tokio::test]
async fn skips_a_symlink_inside_a_session_dir_and_non_session_siblings() {
    use std::os::unix::fs::symlink;
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    let target = root.path().join("target");
    write_aged(&target, "target", DAY * 40);
    let link = directory.join("link");
    symlink(&target, &link).unwrap();
    let unrelated = root.path().join("not-a-session");
    private_dir(&unrelated);
    let unrelated_old = unrelated.join("old");
    write_aged(&unrelated_old, "old", DAY * 40);
    sweep(vec![active(root.path())], cutoff(DAY * 30)).await;
    assert!(link.exists());
    assert!(target.exists());
    assert!(unrelated_old.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn does_not_follow_a_symlinked_session_directory() {
    use std::os::unix::fs::symlink;
    let root = root();
    let foreign = root.path().join("foreign");
    private_dir(&foreign);
    let old = foreign.join("old");
    write_aged(&old, "x", DAY * 40);
    symlink(&foreign, root.path().join("session-aaaaaaaaaaaa")).unwrap();
    sweep(vec![active(root.path())], cutoff(DAY * 30)).await;
    assert!(old.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn skips_a_posix_session_directory_writable_by_another_user() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    let old = directory.join("old");
    write_aged(&old, "x", DAY * 40);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
    let messages = sweep(vec![active(root.path())], cutoff(DAY * 30)).await;
    assert!(old.exists());
    assert!(messages.lock().unwrap()[0].contains("skipped unsafe session directory"));
}

#[tokio::test]
async fn sweeps_only_exact_session_names_not_lookalikes() {
    let root = root();
    for name in ["session-backup", "session-AAAAAAAAAAAA", "session-abc"] {
        let directory = root.path().join(name);
        private_dir(&directory);
        write_aged(&directory.join("old"), "x", DAY * 40);
    }
    let real = session_dir(root.path(), "sess-1");
    private_dir(&real);
    let real_old = real.join("old");
    write_aged(&real_old, "x", DAY * 40);
    sweep(vec![active(root.path())], cutoff(DAY * 30)).await;
    for name in ["session-backup", "session-AAAAAAAAAAAA", "session-abc"] {
        assert!(root.path().join(name).join("old").exists());
    }
    assert!(!real_old.exists());
}

#[tokio::test]
async fn prunes_an_emptied_discovered_root_but_never_the_active_root() {
    let base = root();
    let discovered = base.path().join("discovered");
    let active_root = base.path().join("active");
    private_dir(&discovered);
    private_dir(&active_root);
    let discovered_session = session_dir(&discovered, "old-sess");
    private_dir(&discovered_session);
    write_aged(&discovered_session.join("old"), "x", DAY * 40);
    let active_session = session_dir(&active_root, "sess-1");
    private_dir(&active_session);
    write_aged(&active_session.join("old"), "x", DAY * 40);
    sweep(
        vec![
            SweepRoot {
                path: discovered.to_string_lossy().into_owned(),
                prune_when_empty: true,
            },
            active(&active_root),
        ],
        cutoff(DAY * 30),
    )
    .await;
    assert!(!discovered.exists());
    assert!(active_root.exists());
    assert!(!active_session.exists());
}

#[tokio::test]
async fn de_duplicates_repeated_roots_and_non_prunable_status_wins() {
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    write_aged(&directory.join("old"), "x", DAY * 40);
    sweep(
        vec![
            SweepRoot {
                path: root.path().to_string_lossy().into_owned(),
                prune_when_empty: true,
            },
            active(root.path()),
            SweepRoot {
                path: root.path().to_string_lossy().into_owned(),
                prune_when_empty: true,
            },
        ],
        cutoff(DAY * 30),
    )
    .await;
    assert!(!directory.exists());
    assert!(root.path().exists());
}

#[tokio::test]
async fn does_not_prune_a_discovered_root_with_a_fresh_file() {
    let root = root();
    let directory = session_dir(root.path(), "sess-1");
    private_dir(&directory);
    fs::write(directory.join("fresh"), "x").unwrap();
    sweep(
        vec![SweepRoot {
            path: root.path().to_string_lossy().into_owned(),
            prune_when_empty: true,
        }],
        cutoff(DAY * 30),
    )
    .await;
    assert!(root.path().exists());
}

#[tokio::test]
async fn covers_configured_root_and_discovered_default_roots() {
    let base = root();
    let active_root = base.path().join("active");
    private_dir(&active_root);
    let prior = tempfile::Builder::new()
        .prefix(DEFAULT_ROOT_PREFIX)
        .tempdir_in(base.path())
        .unwrap()
        .keep();
    private_dir(&prior);
    let active_directory = session_dir(&active_root, "active-sess");
    private_dir(&active_directory);
    let active_old = active_directory.join("old");
    write_aged(&active_old, "x", DAY * 40);
    let prior_directory = session_dir(&prior, "old-sess");
    private_dir(&prior_directory);
    let prior_old = prior_directory.join("old");
    write_aged(&prior_old, "x", DAY * 40);
    let (warn, messages) = warnings();
    let roots = gather_sweep_roots(
        active_root.to_str().unwrap(),
        warn.clone(),
        Some(base.path()),
    )
    .await;
    assert_eq!(roots.len(), 2, "{:?}", messages.lock().unwrap());
    let prior = fs::canonicalize(&prior)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(roots.iter().any(|root| root.path == prior));
    sweep_spill_roots(SweepOptions {
        roots,
        cutoff_ms: cutoff(DAY * 30),
        warn,
    })
    .await;
    assert!(!prior_old.exists());
    assert!(!Path::new(&prior).exists());
    assert!(!active_old.exists());
    assert!(active_root.exists());
}

#[tokio::test]
async fn de_dups_when_active_root_is_itself_discovered() {
    let base = root();
    let active_root = tempfile::Builder::new()
        .prefix(DEFAULT_ROOT_PREFIX)
        .tempdir_in(base.path())
        .unwrap()
        .keep();
    private_dir(&active_root);
    let directory = session_dir(&active_root, "sess-1");
    private_dir(&directory);
    let old = directory.join("old");
    write_aged(&old, "x", DAY * 40);
    let (warn, _) = warnings();
    let roots = gather_sweep_roots(
        active_root.to_str().unwrap(),
        warn.clone(),
        Some(base.path()),
    )
    .await;
    assert_eq!(
        roots,
        vec![active(&fs::canonicalize(&active_root).unwrap())]
    );
    sweep_spill_roots(SweepOptions {
        roots,
        cutoff_ms: cutoff(DAY * 30),
        warn,
    })
    .await;
    assert!(!old.exists());
    assert!(active_root.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn de_dups_a_configured_symlink_alias_and_keeps_target_writable() {
    use std::os::unix::fs::symlink;
    let base = root();
    let target = tempfile::Builder::new()
        .prefix(DEFAULT_ROOT_PREFIX)
        .tempdir_in(base.path())
        .unwrap()
        .keep();
    private_dir(&target);
    let alias = base.path().join("alias");
    symlink(&target, &alias).unwrap();
    let directory = session_dir(&target, "sess-1");
    private_dir(&directory);
    let old = directory.join("old");
    write_aged(&old, "x", DAY * 40);
    let (warn, _) = warnings();
    let roots = gather_sweep_roots(alias.to_str().unwrap(), warn.clone(), Some(base.path())).await;
    assert_eq!(roots.len(), 1);
    assert!(!roots[0].prune_when_empty);
    sweep_spill_roots(SweepOptions {
        roots,
        cutoff_ms: cutoff(DAY * 30),
        warn,
    })
    .await;
    assert!(!old.exists());
    assert!(target.exists());
    let saved = save_text_file(SaveTextOptions {
        root: alias.to_string_lossy().into_owned(),
        session_id: "s".into(),
        suggested_name: "x".into(),
        content: "ok".into(),
    })
    .await
    .unwrap();
    assert_eq!(fs::read_to_string(saved.path).unwrap(), "ok");
}

#[tokio::test]
async fn omits_a_missing_active_root() {
    let root = root();
    let (warn, _) = warnings();
    assert!(
        gather_sweep_roots(
            root.path().join("missing").to_str().unwrap(),
            warn,
            Some(root.path())
        )
        .await
        .is_empty()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn skips_a_root_another_posix_user_could_replace() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let parent = root.path().join("unsafe");
    let configured = parent.join("configured");
    private_dir(&configured);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let (warn, messages) = warnings();
    assert!(
        gather_sweep_roots(
            configured.to_str().unwrap(),
            warn,
            Some(&root.path().join("missing"))
        )
        .await
        .is_empty()
    );
    assert!(
        messages
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.contains("skipped unsafe root"))
    );
}

#[tokio::test]
async fn an_unsafe_root_is_contained_logged_never_thrown() {
    let root = root();
    let file = root.path().join("file");
    fs::write(&file, "x").unwrap();
    let messages = sweep(vec![active(&file)], cutoff(DAY * 30)).await;
    assert!(messages.lock().unwrap()[0].contains("skipped unsafe root"));
}

#[tokio::test]
async fn contains_an_exception_from_the_warning_sink() {
    let root = root();
    let file = root.path().join("file");
    fs::write(&file, "x").unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let warn: WarnFn = Arc::new(move |_| {
        captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("logger failed");
    });
    sweep_spill_roots(SweepOptions {
        roots: vec![active(&file)],
        cutoff_ms: cutoff(DAY),
        warn,
    })
    .await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_nonexistent_root_is_silent() {
    let root = root();
    let messages = sweep(vec![active(&root.path().join("missing"))], cutoff(DAY)).await;
    assert!(messages.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn discover_returns_only_real_default_roots() {
    use std::os::unix::fs::symlink;
    let base = root();
    let real = tempfile::Builder::new()
        .prefix(DEFAULT_ROOT_PREFIX)
        .tempdir_in(base.path())
        .unwrap()
        .keep();
    private_dir(&real);
    fs::write(
        base.path().join(format!("{DEFAULT_ROOT_PREFIX}file01")),
        "x",
    )
    .unwrap();
    symlink(
        &real,
        base.path().join(format!("{DEFAULT_ROOT_PREFIX}link01")),
    )
    .unwrap();
    private_dir(&base.path().join("unrelated"));
    let (warn, _) = warnings();
    assert_eq!(
        discover_default_roots(warn, Some(base.path())).await,
        vec![fs::canonicalize(real).unwrap().to_string_lossy()]
    );
}

#[tokio::test]
async fn discover_returns_empty_and_warns_when_base_is_unreadable() {
    let root = root();
    let (warn, messages) = warnings();
    assert!(
        discover_default_roots(warn, Some(&root.path().join("missing")))
            .await
            .is_empty()
    );
    assert!(messages.lock().unwrap()[0].contains("failed to scan"));
}

#[test]
fn is_errno_matches_system_error_code_and_rejects_non_matches() {
    let missing = fs::File::open("/definitely/not/a/file").unwrap_err();
    assert!(is_errno(&missing, "ENOENT"));
    assert!(!is_errno(&missing, "EPERM"));
    assert!(!is_errno(&io::Error::other("plain"), "ENOENT"));
}

#[tokio::test]
async fn routes_a_sweep_failure_to_the_warning_sink() {
    let root = root();
    let file = root.path().join("not-a-dir");
    fs::write(&file, "x").unwrap();
    let messages = sweep(vec![active(&file)], cutoff(DAY)).await;
    assert_eq!(messages.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cleanup_is_awaited_before_direct_sweep_returns() {
    let root = root();
    let directory = session_dir(root.path(), "s");
    private_dir(&directory);
    let old = directory.join("old");
    write_aged(&old, "x", DAY * 40);
    sweep(vec![active(root.path())], cutoff(DAY * 30)).await;
    assert!(!old.exists());
}

#[tokio::test]
async fn loads_cleanup_period_and_prunes_only_expired_session_contents() {
    let root = root();
    let config: Config = serde_json::from_value(serde_json::json!({
        "root": root.path(),
        "cleanupPeriodDays": 30
    }))
    .unwrap();
    let old_directory = session_dir(root.path(), "old-session");
    let fresh_directory = session_dir(root.path(), "fresh-session");
    private_dir(&old_directory);
    private_dir(&fresh_directory);
    let old = old_directory.join("old.txt");
    let fresh = fresh_directory.join("fresh.txt");
    write_aged(&old, "old", DAY * 40);
    write_aged(&fresh, "fresh", DAY);

    sweep(
        vec![active(root.path())],
        cutoff(DAY * config.cleanup_period_days as u32),
    )
    .await;

    assert!(!old.exists());
    assert!(!old_directory.exists());
    assert!(fresh.exists());
    assert!(fresh_directory.exists());
    assert!(root.path().exists());
}
