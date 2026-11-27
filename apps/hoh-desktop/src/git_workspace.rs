//! Local Git data for the T3 Changes panel. Only inspect the selected checkout.

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitQuery {
    Status,
    ListRefs {
        query: Option<String>,
        cursor: Option<usize>,
        limit: Option<usize>,
        ref_kind: Option<RefKind>,
    },
    DiffPreview {
        base_ref: Option<String>,
        commit_sha: Option<String>,
        ignore_whitespace: Option<bool>,
        source_kind: Option<DiffKind>,
        tracked_only: Option<bool>,
        metadata_only: Option<bool>,
        file: Option<DiffFile>,
    },
    BranchCommits {
        base_ref: Option<String>,
    },
    DiffFileContents {
        source_kind: DiffKind,
        change_type: ChangeType,
        base_ref: Option<String>,
        head_ref: Option<String>,
        old_path: String,
        new_path: String,
    },
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RefKind {
    All,
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DiffKind {
    WorkingTree,
    BranchRange,
    Commit,
}

#[derive(Clone, Copy, Deserialize)]
pub enum ChangeType {
    #[serde(rename = "new")]
    New,
    #[serde(rename = "deleted")]
    Deleted,
    #[serde(rename = "change", alias = "rename-changed", alias = "rename-pure")]
    Changed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    path: String,
    previous_path: Option<String>,
    source_kind: DiffKind,
}

fn not_a_repo() -> Value {
    json!({
        "isRepo": false, "refName": null, "isDefaultRef": false, "hasPrimaryRemote": false,
        "hasWorkingTreeChanges": false, "workingTree": { "files": [], "insertions": 0, "deletions": 0 },
        "hasUpstream": false, "aheadCount": 0, "behindCount": 0, "pr": null
    })
}

fn no_repository(query: GitQuery, cwd: &str) -> Result<Value, String> {
    match query {
        GitQuery::Status => Ok(not_a_repo()),
        GitQuery::ListRefs { .. } => Ok(json!({
            "isRepo": false, "hasPrimaryRemote": false, "refs": [], "nextCursor": null, "totalCount": 0
        })),
        GitQuery::DiffPreview { .. } => Ok(json!({ "cwd": cwd, "sources": [] })),
        GitQuery::BranchCommits { .. } => Ok(json!({ "commits": [], "truncated": false })),
        GitQuery::DiffFileContents { .. } => {
            Err("The project folder is not a Git checkout.".into())
        }
    }
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn project_git(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, crate::native::Host>,
    cwd: String,
    query: GitQuery,
) -> Result<Value, String> {
    if window.label() != "main" {
        return Err("Git is available in the main window only.".into());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = state;
        no_repository(query, &cwd)
    }
    #[cfg(target_os = "linux")]
    {
        let engine = state.engine.clone();
        let private = state.chat_secret_directory().to_owned();
        tauri::async_runtime::spawn_blocking(move || {
            let (root, identity) = crate::project_files::project_root(&engine, &cwd)?;
            // Bulk diff and history queries can read the entire checkout before any Rust file open.
            if crate::project_files::PrivateDirectory::new(&private).overlaps(&root) {
                return Err(
                    "Git preview is unavailable in a folder containing HoH private data.".into(),
                );
            }
            let directory = crate::image_asset::open_project_directory(&root, identity)?;
            local::query(directory, &root, query)
        })
        .await
        .map_err(|_| "The Git operation was interrupted.".to_owned())?
    }
}

#[cfg(target_os = "linux")]
mod local;
