//! Port of `tests/search.spec.ts`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hoh_file_reference::{FileReferenceCandidate, FileReferenceKind};
use hoh_file_reference_local::search::{
    DEFAULT_FILE_SEARCH_EXCLUDED_DIRECTORIES, FileSearchConfig, WorkspaceFileSearch,
    active_at_token, format_file_mention,
};
use hoh_timeout::{AbortController, AbortSignal};
use tempfile::TempDir;

fn file(path: &str) -> FileReferenceCandidate {
    FileReferenceCandidate {
        path: path.to_owned(),
        kind: FileReferenceKind::File,
    }
}

fn directory(path: &str) -> FileReferenceCandidate {
    FileReferenceCandidate {
        path: path.to_owned(),
        kind: FileReferenceKind::Directory,
    }
}

fn signal() -> AbortSignal {
    AbortController::new().signal()
}

fn write(path: impl AsRef<Path>, text: &str) {
    std::fs::write(path, text).unwrap();
}

struct Workspace {
    _dir: TempDir,
    root: PathBuf,
}

impl Workspace {
    fn text(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

fn workspace() -> Workspace {
    // The source test relies on `/tmp/...` containing the letters `t`, `u`, `i` in order, so that
    // `terminal-view.ts` matches the absolute query `.../src/tui` as a subsequence. Keep a `tmp`
    // component in the path wherever `TMPDIR` points.
    let parent = std::env::temp_dir().join("tmp");
    std::fs::create_dir_all(&parent).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("dsh-file-autocomplete-")
        .tempdir_in(parent)
        .unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/ignored-package")).unwrap();
    write(root.join("README.md"), "readme");
    write(root.join("src/tui.spec.ts"), "test");
    write(root.join("src/terminal-view.ts"), "view");
    write(root.join("docs/design notes.md"), "design");
    write(root.join(".hidden/secret.txt"), "hidden");
    write(
        root.join("node_modules/ignored-package/index.js"),
        "ignored",
    );
    std::os::unix::fs::symlink(root.join("src/tui.spec.ts"), root.join("linked-test.ts")).unwrap();
    Workspace { _dir: dir, root }
}

fn config(max_results: u64, max_entries: u64, excluded: &[&str]) -> FileSearchConfig {
    FileSearchConfig {
        max_results,
        max_entries,
        excluded_directories: excluded.iter().map(|name| (*name).to_owned()).collect(),
    }
}

fn default_config() -> FileSearchConfig {
    config(20, 10_000, &[".git", "node_modules"])
}

fn search(root: &str, config: FileSearchConfig) -> WorkspaceFileSearch {
    WorkspaceFileSearch::new(root, config).unwrap()
}

async fn eventually<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..1_000 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition did not hold within the wait");
}

#[test]
fn recognizes_boundary_and_quoted_mentions_without_treating_emails_as_references() {
    let token = active_at_token("@src/tu", 7).unwrap();
    assert_eq!(
        (token.prefix.as_str(), token.query.as_str(), token.quoted),
        ("@src/tu", "src/tu", false)
    );
    let token = active_at_token("read @\"docs/design n", 20).unwrap();
    assert_eq!(
        (token.prefix.as_str(), token.query.as_str(), token.quoted),
        ("@\"docs/design n", "docs/design n", true)
    );
    assert_eq!(active_at_token("mail a@b.test", 13), None);
    assert_eq!(active_at_token("done @src/x\" next", 17), None);
}

#[test]
fn formats_files_directories_quotes_and_rejects_unsafe_editor_values() {
    assert_eq!(
        format_file_mention(&file("src/index.ts"), false).as_deref(),
        Some("@src/index.ts")
    );
    assert_eq!(
        format_file_mention(&directory("src"), false).as_deref(),
        Some("@src/")
    );
    assert_eq!(
        format_file_mention(&file("docs/design notes.md"), false).as_deref(),
        Some("@\"docs/design notes.md\"")
    );
    assert_eq!(
        format_file_mention(&directory("docs/design notes"), false).as_deref(),
        Some("@\"docs/design notes/")
    );
    assert_eq!(
        format_file_mention(&file("README.md"), true).as_deref(),
        Some("@\"README.md\"")
    );
    assert_eq!(format_file_mention(&file("bad\nname"), false), None);
    assert_eq!(format_file_mention(&file("bad \"name\".md"), false), None);
    assert_eq!(format_file_mention(&file("bad\"name.md"), false), None);
}

#[tokio::test]
async fn lists_live_directory_levels_descends_quotes_spaces_and_filters_hidden_excluded_entries() {
    let workspace = workspace();
    let files = search(&workspace.text(), default_config());
    let signal = signal();

    assert_eq!(
        files.list("", &signal).await.unwrap(),
        vec![directory("docs"), directory("src"), file("README.md")]
    );
    assert_eq!(
        files.list("src/", &signal).await.unwrap(),
        vec![file("src/terminal-view.ts"), file("src/tui.spec.ts")]
    );
    assert_eq!(
        files.list("src/ts", &signal).await.unwrap(),
        vec![file("src/tui.spec.ts"), file("src/terminal-view.ts")]
    );
    assert_eq!(
        files.list("docs/design n", &signal).await.unwrap(),
        vec![file("docs/design notes.md")]
    );
    assert_eq!(files.list("node_modules/", &signal).await.unwrap(), vec![]);
    assert_eq!(
        files.list(".hidden/", &signal).await.unwrap(),
        vec![file(".hidden/secret.txt")]
    );
    let absolute_src = format!("{}/", workspace.root.join("src").to_string_lossy());
    assert_eq!(
        files
            .list(&format!("{absolute_src}tui"), &signal)
            .await
            .unwrap(),
        vec![
            file(&format!("{absolute_src}tui.spec.ts")),
            file(&format!("{absolute_src}terminal-view.ts")),
        ]
    );
    assert_eq!(
        files
            .list("~/.dsh-file-autocomplete-missing/", &signal)
            .await
            .unwrap(),
        vec![]
    );
    assert_eq!(files.list("../", &signal).await.unwrap(), vec![]);
    assert_eq!(files.list("README.md/", &signal).await.unwrap(), vec![]);
}

#[tokio::test]
async fn does_not_traverse_directory_symlinks_during_direct_completion() {
    let workspace = workspace();
    let outside = tempfile::Builder::new()
        .prefix("dsh-file-autocomplete-outside-")
        .tempdir()
        .unwrap();
    write(outside.path().join("outside-secret.txt"), "secret");
    std::os::unix::fs::symlink(outside.path(), workspace.root.join("escape")).unwrap();
    let files = search(&workspace.text(), default_config());
    let signal = signal();

    assert_eq!(files.list("escape/", &signal).await.unwrap(), vec![]);
    assert_eq!(files.list("escape/outside", &signal).await.unwrap(), vec![]);
}

#[tokio::test]
async fn ranks_basename_and_subsequence_fuzzy_matches_across_the_bounded_workspace_index() {
    let workspace = workspace();
    write(workspace.root.join("src/tspc-helper.ts"), "helper");
    let files = search(
        &workspace.text(),
        config(2, 10_000, &[".git", "node_modules"]),
    );
    let signal = signal();

    assert_eq!(
        files.list("tspc", &signal).await.unwrap(),
        vec![file("src/tspc-helper.ts"), file("src/tui.spec.ts")]
    );
    assert_eq!(
        files.list("README.md", &signal).await.unwrap(),
        vec![file("README.md")]
    );
    assert_eq!(
        files.list("terminal", &signal).await.unwrap(),
        vec![file("src/terminal-view.ts")]
    );
    assert_eq!(files.list("secret", &signal).await.unwrap(), vec![]);
    assert_eq!(
        files.list(".hidden", &signal).await.unwrap(),
        vec![directory(".hidden"), file(".hidden/secret.txt")]
    );
}

#[tokio::test]
async fn serves_an_invalidated_index_while_its_replacement_builds_then_swaps_it_in() {
    let workspace = workspace();
    let files = search(&workspace.text(), default_config());
    let signal = signal();
    assert_eq!(files.list("fresh-file", &signal).await.unwrap(), vec![]);
    write(workspace.root.join("fresh-file.ts"), "fresh");
    // No invalidation: the settled traversal is still the answer.
    assert_eq!(files.list("fresh-file", &signal).await.unwrap(), vec![]);
    files.invalidate();
    // The stale entries answer this query; the rebuild runs behind the caret.
    assert_eq!(files.list("fresh-file", &signal).await.unwrap(), vec![]);
    eventually(|| async {
        files.list("fresh-file", &signal).await.unwrap() == vec![file("fresh-file.ts")]
    })
    .await;
    files.dispose();
    assert_eq!(files.list("fresh-file", &signal).await.unwrap(), vec![]);
    files.dispose();
}

#[tokio::test]
async fn keeps_the_stale_entries_when_the_workspace_root_is_unreadable_and_retries_once_it_returns()
{
    let workspace = workspace();
    let root = workspace.root.clone();
    let files = search(&workspace.text(), default_config());
    let signal = signal();
    assert_eq!(
        files.list("README", &signal).await.unwrap(),
        vec![file("README.md")]
    );

    // A root that vanishes under a live index: an unreadable branch costs its
    // own candidates, but an unreadable root must not be published as an
    // empty workspace over entries that are still good.
    std::fs::remove_dir_all(&root).unwrap();
    files.invalidate();
    assert_eq!(
        files.list("README", &signal).await.unwrap(),
        vec![file("README.md")]
    );
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(
            files.list("README", &signal).await.unwrap(),
            vec![file("README.md")]
        );
    }

    // A retry can read the root as soon as it exists; publish its files together.
    std::fs::create_dir(&root).unwrap();
    write(root.join("restored.ts"), "restored");
    eventually(|| async {
        files.list("restored", &signal).await.unwrap() == vec![file("restored.ts")]
    })
    .await;
}

// chmod 0 can only deny directory reads to a non-root owner: root bypasses the
// bits, and the sealed candidate is then indexed, so the unreadable-branch
// behavior is pinned where the fixture really is unreadable.
#[tokio::test]
async fn lets_an_unreadable_subtree_cost_only_its_own_candidates() {
    let workspace = workspace();
    let locked = workspace.root.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    write(locked.join("sealed.ts"), "sealed");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = std::fs::read_dir(&locked).is_err();
    let files = search(&workspace.text(), default_config());
    let signal = signal();

    if unreadable {
        // The branch itself yields nothing, and the rest of the tree still does.
        assert_eq!(files.list("sealed", &signal).await.unwrap(), vec![]);
        assert_eq!(
            files.list("README", &signal).await.unwrap(),
            vec![file("README.md")]
        );
        // The directory is still offered: only reading through it fails.
        assert_eq!(
            files.list("locked", &signal).await.unwrap(),
            vec![directory("locked")]
        );
    }
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[tokio::test]
async fn enforces_the_entry_cap() {
    let workspace = workspace();
    let capped = search(&workspace.text(), config(20, 2, &[".git", "node_modules"]));
    assert_eq!(
        capped.list("README", &signal()).await.unwrap(),
        vec![file("README.md")]
    );
}

#[tokio::test]
async fn never_traverses_an_excluded_build_output_so_generated_twins_cannot_outrank_sources() {
    let workspace = workspace();
    std::fs::create_dir_all(workspace.root.join("dist")).unwrap();
    write(workspace.root.join("dist/terminal-view.js"), "built");
    let files = search(
        &workspace.text(),
        config(20, 10_000, &DEFAULT_FILE_SEARCH_EXCLUDED_DIRECTORIES),
    );
    assert_eq!(
        files.list("terminal-view", &signal()).await.unwrap(),
        vec![file("src/terminal-view.ts")]
    );
    assert_eq!(files.list("dist/", &signal()).await.unwrap(), vec![]);
}

#[tokio::test]
async fn still_offers_a_lib_tree_where_several_ecosystems_keep_their_sources() {
    let workspace = workspace();
    std::fs::create_dir_all(workspace.root.join("lib")).unwrap();
    write(workspace.root.join("lib/gem-entry.rb"), "source");
    let files = search(
        &workspace.text(),
        config(20, 10_000, &DEFAULT_FILE_SEARCH_EXCLUDED_DIRECTORIES),
    );
    assert_eq!(
        files.list("gem-entry", &signal()).await.unwrap(),
        vec![file("lib/gem-entry.rb")]
    );
}

#[tokio::test]
async fn cancels_individual_callers_skips_missing_directories_and_validates_limits() {
    let workspace = workspace();
    let root = workspace.text();
    let error = |config| {
        WorkspaceFileSearch::new(root.clone(), config)
            .err()
            .unwrap()
            .to_string()
    };
    assert!(error(config(0, 10_000, &[".git"])).contains("maxResults"));
    // A fractional `maxEntries` has no `u64` form: serde rejects it before the search is built.
    assert!(error(config(20, 0, &[".git"])).contains("maxEntries"));
    assert!(error(config(20, 10_000, &["nested/name"])).contains("basenames"));

    let files = search(&root, default_config());
    assert_eq!(files.list("missing/", &signal()).await.unwrap(), vec![]);

    let pre_aborted = AbortController::new();
    pre_aborted.abort_with("pre-aborted");
    assert_eq!(
        files
            .list("tui", &pre_aborted.signal())
            .await
            .unwrap_err()
            .to_string(),
        "pre-aborted"
    );

    files.invalidate();
    let running = AbortController::new();
    let pending = {
        let files = files.clone();
        let signal = running.signal();
        tokio::spawn(async move { files.list("tui", &signal).await.map_err(|e| e.to_string()) })
    };
    tokio::task::yield_now().await;
    running.abort_with("superseded");
    assert_eq!(pending.await.unwrap().unwrap_err(), "superseded");
}

#[tokio::test]
async fn orders_names_and_counts_gaps_in_utf16_code_units() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    // U+1F600 is the surrogates 0xD83D 0xDE00 and U+FF5A is 0xFF5A: JavaScript orders the
    // emoji first; code point or UTF-8 order would put it last.
    write(root.join("\u{1F600}.md"), "a");
    write(root.join("\u{FF5A}.md"), "b");
    let files = search(&root.to_string_lossy(), default_config());
    assert_eq!(
        files.list("", &signal()).await.unwrap(),
        vec![file("\u{1F600}.md"), file("\u{FF5A}.md")]
    );
}

#[tokio::test]
async fn measures_subsequence_gaps_in_utf16_code_units() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    // Query `a😀b` is a plain subsequence of both names. In UTF-16 units the gap to `b` is
    // 4 in each (the emoji in `a😀x😀b` counts 2), so the scores tie and the ordinary
    // tie-breakers decide: equal length, then code-unit order puts `x` before the surrogate.
    // A per-character count would give `a😀x😀b` the smaller gap and rank it first.
    write(root.join("a\u{1F600}x\u{1F600}b.txt"), "1");
    write(root.join("a\u{1F600}xxxb.txt"), "2");
    let files = search(&root.to_string_lossy(), default_config());
    assert_eq!(
        files.list("a\u{1F600}b", &signal()).await.unwrap(),
        vec![
            file("a\u{1F600}xxxb.txt"),
            file("a\u{1F600}x\u{1F600}b.txt")
        ]
    );
}
