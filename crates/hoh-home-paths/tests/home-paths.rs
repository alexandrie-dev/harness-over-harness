use hoh_home_paths::*;
use std::{collections::HashMap, path::Path};
fn env(key: &str, value: &str) -> HashMap<String, String> {
    [(key.into(), value.into())].into()
}
fn home() -> std::path::PathBuf {
    std::env::home_dir().unwrap()
}
#[test]
fn owns_shared_default_dsh_home_directory_name() {
    assert_eq!(DSH_HOME_DIR_NAME, ".dsh");
    assert_eq!(DEFAULT_DSH_HOME_DISPLAY, "~/.dsh");
    assert_eq!(default_dsh_home().unwrap(), home().join(".dsh"));
}
#[test]
fn expands_tilde_paths_without_changing_non_tilde_paths() {
    assert_eq!(expand_home_path("~").unwrap(), home());
    for s in ["~/.dsh", "~\\.dsh"] {
        assert_eq!(expand_home_path(s).unwrap(), home().join(".dsh"));
    }
    for s in ["/tmp/.dsh", "~other/.dsh"] {
        assert_eq!(expand_home_path(s).unwrap(), Path::new(s));
    }
}
#[test]
fn resolves_explicit_path_before_dsh_home_and_default() {
    let e = env(DSH_HOME_ENV, "~/env-dsh");
    assert_eq!(
        resolve_dsh_home(Some("/tmp/explicit-dsh"), Some(&e)).unwrap(),
        std::path::absolute("/tmp/explicit-dsh").unwrap()
    );
    assert_eq!(
        resolve_dsh_home(None, Some(&e)).unwrap(),
        home().join("env-dsh")
    );
    assert_eq!(
        resolve_dsh_home(None, Some(&HashMap::new())).unwrap(),
        default_dsh_home().unwrap()
    );
}
#[test]
fn treats_empty_or_whitespace_dsh_home_as_unset() {
    for value in ["", "   "] {
        assert_eq!(
            resolve_dsh_home(None, Some(&env(DSH_HOME_ENV, value))).unwrap(),
            default_dsh_home().unwrap()
        );
    }
}
#[test]
fn joins_child_segments_onto_resolved_home() {
    let e = env(DSH_HOME_ENV, "~/env-dsh");
    assert_eq!(
        dsh_home_path(&[], Some(&e)).unwrap(),
        home().join("env-dsh")
    );
    assert_eq!(
        dsh_home_path(&["storages", "cache"], Some(&e)).unwrap(),
        home().join("env-dsh/storages/cache")
    );
}
#[test]
fn labels_resolved_home_by_whether_it_is_default() {
    assert_eq!(
        dsh_home_display(&default_dsh_home().unwrap()).unwrap(),
        "~/.dsh"
    );
    assert_eq!(
        dsh_home_display(Path::new("/some/other/root")).unwrap(),
        "$DSH_HOME"
    );
}
#[test]
fn owns_shared_default_agents_home_directory_name() {
    assert_eq!(AGENTS_HOME_DIR_NAME, ".agents");
    assert_eq!(DEFAULT_AGENTS_HOME_DISPLAY, "~/.agents");
    assert_eq!(default_agents_home().unwrap(), home().join(".agents"));
}
#[test]
fn resolves_explicit_agents_home_before_environment_and_default() {
    let e = env(AGENTS_HOME_ENV, "~/env-agents");
    assert_eq!(
        resolve_agents_home(Some("/tmp/explicit-agents"), Some(&e)).unwrap(),
        std::path::absolute("/tmp/explicit-agents").unwrap()
    );
    assert_eq!(
        resolve_agents_home(None, Some(&e)).unwrap(),
        home().join("env-agents")
    );
    assert_eq!(
        resolve_agents_home(None, Some(&HashMap::new())).unwrap(),
        default_agents_home().unwrap()
    );
}
#[test]
fn treats_empty_or_whitespace_agents_home_as_unset() {
    for value in ["", "   "] {
        assert_eq!(
            resolve_agents_home(None, Some(&env(AGENTS_HOME_ENV, value))).unwrap(),
            default_agents_home().unwrap()
        );
    }
}
#[test]
fn labels_resolved_agents_home_by_whether_it_is_default() {
    assert_eq!(
        agents_home_display(&default_agents_home().unwrap()).unwrap(),
        "~/.agents"
    );
    assert_eq!(
        agents_home_display(Path::new("/some/other/root")).unwrap(),
        "$DSH_AGENTS_HOME"
    );
}
#[test]
fn resolves_cache_paths_with_all_environment_values() {
    for value in [
        None,
        Some(""),
        Some("   "),
        Some("~/env-dsh"),
        Some("./relative-dsh"),
    ] {
        let e = value.map(|v| env(DSH_HOME_ENV, v)).unwrap_or_default();
        let expected = match value {
            Some("~/env-dsh") => home().join("env-dsh"),
            Some("./relative-dsh") => std::env::current_dir().unwrap().join("relative-dsh"),
            _ => default_dsh_home().unwrap(),
        };
        assert_eq!(
            dsh_cache_path(None, &[], Some(&e)).unwrap(),
            expected.join("cache")
        );
        assert_eq!(
            dsh_cache_path(None, &["models", "index.json"], Some(&e)).unwrap(),
            expected.join("cache/models/index.json")
        );
    }
}
#[test]
fn resolves_configured_cache_home_before_environment() {
    let e = env(DSH_HOME_ENV, "~/env-dsh");
    assert_eq!(
        dsh_cache_path(Some("~/explicit-dsh"), &[], Some(&e)).unwrap(),
        home().join("explicit-dsh/cache")
    );
    assert_eq!(
        dsh_cache_path(
            Some("./explicit-dsh"),
            &["attachments", "request-images"],
            Some(&e)
        )
        .unwrap(),
        std::env::current_dir()
            .unwrap()
            .join("explicit-dsh/cache/attachments/request-images")
    );
    assert_eq!(
        dsh_cache_path(None, &["attachments"], Some(&e)).unwrap(),
        home().join("env-dsh/cache/attachments")
    );
}
#[test]
fn resolves_category_paths_with_all_environment_values() {
    for value in [None, Some(""), Some("~/env-dsh"), Some("./relative-dsh")] {
        let e = value.map(|v| env(DSH_HOME_ENV, v)).unwrap_or_default();
        let expected = match value {
            Some("~/env-dsh") => home().join("env-dsh"),
            Some("./relative-dsh") => std::env::current_dir().unwrap().join("relative-dsh"),
            _ => default_dsh_home().unwrap(),
        };
        for (category, name) in [
            (DshHomeCategory::Config, "config"),
            (DshHomeCategory::Data, "data"),
            (DshHomeCategory::State, "state"),
            (DshHomeCategory::Cache, "cache"),
            (DshHomeCategory::Backup, "backup"),
        ] {
            assert_eq!(
                dsh_path(category, &[], Some(&e)).unwrap(),
                expected.join(name)
            );
            assert_eq!(
                dsh_path(category, &["owner", "file.json"], Some(&e)).unwrap(),
                expected.join(name).join("owner/file.json")
            );
        }
    }
}
#[test]
fn resolves_configured_category_home_before_environment() {
    let e = env(DSH_HOME_ENV, "~/env-dsh");
    assert_eq!(
        dsh_path(
            DshPathLocation {
                category: DshHomeCategory::State,
                dsh_home: Some("~/explicit-dsh".into())
            },
            &["logs"],
            Some(&e)
        )
        .unwrap(),
        home().join("explicit-dsh/state/logs")
    );
    assert_eq!(
        dsh_path(
            DshPathLocation {
                category: DshHomeCategory::Data,
                dsh_home: None
            },
            &[],
            Some(&e)
        )
        .unwrap(),
        home().join("env-dsh/data")
    );
}
#[tokio::test]
async fn canonicalizes_watcher_ancestor_preserving_missing_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let alias = dir.path().join("alias");
    tokio::fs::create_dir(&target).await.unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &alias).unwrap();
    let real = tokio::fs::canonicalize(&target).await.unwrap();
    assert_eq!(canonicalize_watch_path(&alias).await.unwrap(), real);
    assert_eq!(
        canonicalize_watch_path(alias.join("later/config.yml"))
            .await
            .unwrap(),
        real.join("later/config.yml")
    );
    let file = dir.path().join("file");
    tokio::fs::write(&file, "not a directory").await.unwrap();
    assert_eq!(
        canonicalize_watch_path(file.join("child"))
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotADirectory
    );
}
