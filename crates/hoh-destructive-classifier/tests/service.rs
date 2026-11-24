//! The service's own entry points: commands, deletions and overwrites against a real folder.

use std::sync::Arc;

use hoh_destructive_classifier::{DestructiveClassifierService, RealFs, Risk};

fn folder() -> tempfile::TempDir {
    std::fs::create_dir_all("/dev/shm/hoh").expect("scratch root");
    let dir = tempfile::Builder::new()
        .prefix("o1-classifier-")
        .tempdir_in("/dev/shm/hoh")
        .expect("temp dir");
    std::fs::write(dir.path().join("old.txt"), "keep").expect("file");
    dir
}

fn service() -> DestructiveClassifierService {
    DestructiveClassifierService::new(Arc::new(RealFs))
}

#[tokio::test]
async fn a_command_is_judged_like_the_window_guard() {
    let dir = folder();
    let cwd = Some(dir.path().to_path_buf());
    assert!(service().command("ls -la", cwd.clone()).await.is_none());
    let asked = service()
        .command("rm old.txt", cwd.clone())
        .await
        .expect("a removal is asked");
    assert!(asked.details.contains("rm old.txt"), "{}", asked.details);
    // The whole folder path, not its last two names (QA P2: «Documents/HoH»).
    let folder = format!("(folder {})", dir.path().display());
    assert!(asked.headline.contains(&folder), "{}", asked.headline);
    let risks = service().command_risks("rm old.txt", cwd).await;
    assert!(
        matches!(risks.as_slice(), [Risk::Delete { .. }]),
        "{risks:?}"
    );
}

#[test]
fn a_deletion_is_always_asked_and_lists_the_files() {
    let dir = folder();
    assert!(service().deletion(&[], Some(dir.path())).is_none());
    let paths = ["old.txt".to_owned(), "missing.txt".to_owned()];
    let asked = service().deletion(&paths, Some(dir.path())).expect("asked");
    assert_eq!(asked.details, "Files:\nold.txt\nmissing.txt");
    assert!(!asked.headline.is_empty());
}

#[test]
fn an_overwrite_is_asked_only_for_existing_files() {
    let dir = folder();
    let new = ["new.txt".to_owned()];
    assert!(service().overwrite(&new, Some(dir.path())).is_none());
    let both = ["new.txt".to_owned(), "old.txt".to_owned()];
    let asked = service().overwrite(&both, Some(dir.path())).expect("asked");
    assert_eq!(asked.details, "Files:\nold.txt");
    let absolute = [dir.path().join("old.txt").to_string_lossy().into_owned()];
    assert!(service().overwrite(&absolute, None).is_some());
}

#[tokio::test]
async fn git_approval_names_the_source_destination_and_local_changes_at_risk() {
    let dir = folder();
    let classifier = service();
    let cases: &[(&str, &[&str])] = &[
        (
            "git restore -- config.toml",
            &[
                "from the Git index",
                "last commit if those entries match HEAD",
                "overwriting the working copies: config.toml",
            ],
        ),
        (
            "git restore --source=HEAD~ -- config.toml",
            &[
                "from the specified Git tree",
                "in the working tree: config.toml",
                "overwritten or removed",
            ],
        ),
        (
            "git restore -s HEAD~ config.toml",
            &[
                "from the specified Git tree",
                "in the working tree: config.toml",
            ],
        ),
        (
            "git restore --staged --worktree -- config.toml",
            &[
                "from HEAD (the current commit)",
                "in the index and working tree: config.toml",
            ],
        ),
        (
            "git restore -SW -sHEAD~ -- config.toml",
            &[
                "from the specified Git tree",
                "in the index and working tree: config.toml",
            ],
        ),
        (
            "git restore -S -- config.toml",
            &["from HEAD (the current commit) in the index: config.toml"],
        ),
        (
            "git restore -- --staged",
            &[
                "from the Git index",
                "overwriting the working copies: --staged",
            ],
        ),
        (
            "git checkout -- config.toml",
            &[
                "from the Git index: config.toml",
                "overwriting the working copies",
            ],
        ),
        (
            "git checkout HEAD~ -- config.toml",
            &[
                "from the specified Git tree: config.toml",
                "index entries and working copies",
            ],
        ),
        (
            "git checkout -f other",
            &[
                "may switch branches",
                "discard local changes",
                "may remove untracked paths in the way",
            ],
        ),
        (
            "git reset --hard HEAD~",
            &[
                "chosen commit (HEAD by default)",
                "index, and the working tree",
                "Discard staged and working-tree changes",
                "Untracked paths in the way may be overwritten or removed",
            ],
        ),
        (
            "git clean -fd",
            &[
                "selected untracked paths",
                "Ignored paths are normally spared",
            ],
        ),
        (
            "git clean -fx",
            &["selected untracked paths", "can include ignored paths"],
        ),
        ("git clean -fX", &["selected ignored paths"]),
        (
            "git rm config.toml",
            &["from the Git index and working tree: config.toml"],
        ),
    ];
    for &(command, fragments) in cases {
        let question = classifier
            .command(command, Some(dir.path().into()))
            .await
            .expect(command);
        for fragment in fragments {
            assert!(
                question.headline.contains(fragment),
                "{command}: {}",
                question.headline
            );
        }
    }
    let question = classifier
        .command(
            "git rm --pathspec-from-file list.txt",
            Some(dir.path().into()),
        )
        .await
        .expect("git rm reads the paths to remove from the file");
    assert!(
        question
            .headline
            .contains("from the Git index and working tree")
    );
    assert!(!question.headline.contains("list.txt"));
}

#[tokio::test]
async fn shell_approval_distinguishes_remove_move_resize_and_unknown_code() {
    let dir = folder();
    let classifier = service();
    let cases: &[(&str, &[&str])] = &[
        (
            "rm old.txt",
            &[
                "Delete 1 file: old.txt",
                "bypass the trash and may be unrecoverable",
            ],
        ),
        (
            "mv replacement.txt old.txt",
            &[
                "Move replacement.txt to a new location",
                "overwrite file old.txt",
                "Previous contents may be lost",
            ],
        ),
        (
            "truncate -s 64 old.txt",
            &["Resize old.txt", "If the files shrink"],
        ),
        (
            "rsync --remove-source-files src/ dst/",
            &["successfully duplicated files from the sender"],
        ),
        (
            "rsync --delete src/ dst/",
            &["may remove destination paths"],
        ),
        (
            "tar xf archive.tar",
            &["Extract archive archive.tar", "may be replaced"],
        ),
        (
            "git apply fix.patch",
            &["Apply a patch", "may change or remove existing files"],
        ),
        (
            "sh -c 'echo hello'",
            &[
                "command string passed to a shell",
                "may delete or overwrite files",
            ],
        ),
        (
            "bash update.sh",
            &[
                "execution of a shell script",
                "may delete or overwrite files",
            ],
        ),
    ];
    for &(command, fragments) in cases {
        let question = classifier
            .command(command, Some(dir.path().into()))
            .await
            .expect(command);
        for fragment in fragments {
            assert!(
                question.headline.contains(fragment),
                "{command}: {}",
                question.headline
            );
        }
        assert!(
            !question
                .headline
                .chars()
                .any(|ch| ('\u{0400}'..='\u{04ff}').contains(&ch))
        );
    }
}
