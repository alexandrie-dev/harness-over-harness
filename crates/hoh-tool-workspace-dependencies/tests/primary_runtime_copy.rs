#![cfg(unix)]

use hoh_error::HarnessError;
use hoh_tool_workspace_dependencies::{
    PrimaryRuntimeManifest, WorkspaceDependencyError, copy_primary_runtime,
};
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    source: PathBuf,
    destination: PathBuf,
    bin: PathBuf,
    executable: String,
    manifest: PrimaryRuntimeManifest,
}

async fn fixture(platform: &str) -> Fixture {
    let root = TempDir::new().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    let bin = if platform == "win32" {
        source.join("dependencies/python")
    } else {
        source.join("dependencies/python/bin")
    };
    tokio::fs::create_dir_all(&bin).await.unwrap();
    let executable = if platform == "win32" {
        "python.exe"
    } else {
        "python3.12"
    }
    .to_owned();
    tokio::fs::write(bin.join(&executable), "interpreter bytes")
        .await
        .unwrap();
    Fixture {
        _root: root,
        source,
        destination,
        bin,
        executable,
        manifest: PrimaryRuntimeManifest {
            desktop_version: "1".into(),
            platform: platform.into(),
            arch: "x64".into(),
            payload_digest: None,
            python: "3.12.14".into(),
            node: None,
            pnpm: None,
            python_packages: IndexMap::new(),
        },
    }
}

async fn aliases(bin: &Path, target: &Path) {
    for name in ["python", "python3"] {
        tokio::fs::symlink(target, bin.join(name)).await.unwrap();
    }
}

#[tokio::test]
async fn copies_each_declared_layout_and_preserves_only_validated_aliases() {
    for platform in ["linux", "darwin", "win32"] {
        let item = fixture(platform).await;
        if platform != "win32" {
            aliases(&item.bin, Path::new(&item.executable)).await;
        }
        copy_primary_runtime(
            item.source.to_str().unwrap(),
            item.destination.to_str().unwrap(),
            &item.manifest,
        )
        .await
        .unwrap();
        tokio::fs::remove_dir_all(&item.source).await.unwrap();
        let installed_bin = if platform == "win32" {
            item.destination.join("dependencies/python")
        } else {
            item.destination.join("dependencies/python/bin")
        };
        assert_eq!(
            tokio::fs::read_to_string(installed_bin.join(&item.executable))
                .await
                .unwrap(),
            "interpreter bytes"
        );
        if platform != "win32" {
            for name in ["python", "python3"] {
                assert_eq!(
                    tokio::fs::read_link(installed_bin.join(name))
                        .await
                        .unwrap(),
                    PathBuf::from("python3.12")
                );
            }
        }
    }
}

#[tokio::test]
async fn rejects_each_invalid_alias_before_creating_the_destination() {
    for kind in ["absolute", "escaping", "chained", "dangling", "directory"] {
        let item = fixture("linux").await;
        let target = match kind {
            "absolute" => item.bin.join(&item.executable),
            "escaping" => PathBuf::from("../outside"),
            "chained" => PathBuf::from("python3"),
            _ => PathBuf::from(&item.executable),
        };
        if kind == "dangling" || kind == "directory" {
            tokio::fs::remove_file(item.bin.join(&item.executable))
                .await
                .unwrap();
        }
        if kind == "directory" {
            tokio::fs::create_dir(item.bin.join(&item.executable))
                .await
                .unwrap();
        }
        aliases(&item.bin, &target).await;
        let error = copy_primary_runtime(
            item.source.to_str().unwrap(),
            item.destination.to_str().unwrap(),
            &item.manifest,
        )
        .await
        .unwrap_err();
        if kind == "dangling" {
            assert!(error.to_string().contains("ENOENT"));
        } else {
            assert!(error.to_string().contains("invalid Python alias"));
            assert!(
                HarnessError::of(error.as_ref())
                    .is_some_and(|error| error.is_named(WorkspaceDependencyError::NAME))
            );
        }
        assert!(!item.destination.exists());
    }
}

#[tokio::test]
async fn copies_legacy_regular_python_commands_without_an_optional_alias() {
    let item = fixture("linux").await;
    tokio::fs::write(item.bin.join("python3"), "legacy interpreter")
        .await
        .unwrap();
    copy_primary_runtime(
        item.source.to_str().unwrap(),
        item.destination.to_str().unwrap(),
        &item.manifest,
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::fs::read_to_string(item.destination.join("dependencies/python/bin/python3"))
            .await
            .unwrap(),
        "legacy interpreter"
    );
    assert!(
        tokio::fs::symlink_metadata(item.destination.join("dependencies/python/bin/python"))
            .await
            .is_err()
    );
}
