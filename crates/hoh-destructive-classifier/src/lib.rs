//! The shared classifier of dangerous actions: which shell commands and file changes can delete or
//! overwrite an existing file, and the Russian question the window shows before such an action.
//! There is no dsh counterpart (see `PORT_NOTES.md`).
//!
//! The logic was moved here unchanged from `hoh-window-destructive-guard`, which keeps asking
//! before a dangerous `bash` call of the own agent. Callers outside the tool pipeline — the
//! external harness slot (`hoh-external-harness`) answering a native permission request of Claude
//! or Codex — use the same logic through the [`DestructiveClassifier`] service.

mod classify;
mod fs;
mod reach;
mod shell;
mod text;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, NoConfig, Plugin};

pub use classify::{GitKind, Risk, Source, assess, assess_blocking, assess_with};
pub use fs::{Fs, RealFs};
pub use reach::{CommandReach, command_reach};
pub use text::{Question, escalation_question, question};

/// The question for one shell command run in `cwd`: `None` when nothing in it can delete or
/// overwrite an existing file. The file system is only touched on a blocking thread.
pub async fn decide_command(
    command: &str,
    cwd: Option<PathBuf>,
    fs: Arc<dyn Fs>,
) -> Option<Question> {
    let risks = assess_blocking(command.to_owned(), cwd.clone(), fs).await;
    if risks.is_empty() {
        return None;
    }
    Some(question(&risks, command, cwd.as_deref()))
}

/// The classifier as a kernel service (`destructiveClassifier`).
pub struct DestructiveClassifierService {
    fs: Arc<dyn Fs>,
}

hoh_kernel::service!(pub DestructiveClassifier: DestructiveClassifierService = "destructiveClassifier");

impl DestructiveClassifierService {
    pub fn new(fs: Arc<dyn Fs>) -> Self {
        Self { fs }
    }

    /// Every risk of a shell command run in `cwd` (`None`: the folder is not known).
    pub async fn command_risks(&self, command: &str, cwd: Option<PathBuf>) -> Vec<Risk> {
        assess_blocking(command.to_owned(), cwd, self.fs.clone()).await
    }

    /// The question for a shell command run in `cwd`; `None` when it is harmless.
    pub async fn command(&self, command: &str, cwd: Option<PathBuf>) -> Option<Question> {
        decide_command(command, cwd, self.fs.clone()).await
    }

    /// The question for a file change that removes `paths` (relative to `cwd` or absolute);
    /// `None` for an empty list. Removing files cannot be undone, so it is always asked.
    pub fn deletion(&self, paths: &[String], cwd: Option<&Path>) -> Option<Question> {
        if paths.is_empty() {
            return None;
        }
        let risks = [Risk::Delete {
            names: paths.to_vec(),
            folders: false,
            source: Source::Listed,
        }];
        let mut asked = question(&risks, "", cwd);
        asked.details = file_list(paths);
        Some(asked)
    }

    /// The question for a file change that replaces existing files at `paths`; `None` when none
    /// of them exists yet (creating a new file is not dangerous).
    pub fn overwrite(&self, paths: &[String], cwd: Option<&Path>) -> Option<Question> {
        let existing: Vec<String> = paths
            .iter()
            .filter(|path| {
                let path = Path::new(path.as_str());
                let full = match cwd {
                    Some(cwd) if path.is_relative() => cwd.join(path),
                    _ => path.to_path_buf(),
                };
                self.fs.exists(&full)
            })
            .cloned()
            .collect();
        if existing.is_empty() {
            return None;
        }
        let risks = [Risk::Overwrite {
            names: existing.clone(),
            checked: true,
        }];
        let mut asked = question(&risks, "", cwd);
        asked.details = file_list(&existing);
        Some(asked)
    }
}

/// `details` of a file-change question: the paths, one per line, as the harness wrote them.
fn file_list(paths: &[String]) -> String {
    const MAX_DETAILS_CHARS: usize = 2000;
    let text = format!("Files:\n{}", paths.join("\n"));
    match text.char_indices().nth(MAX_DETAILS_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

/// Plugin that provides [`DestructiveClassifier`] over the real file system.
pub struct DestructiveClassifierPlugin;

impl Plugin for DestructiveClassifierPlugin {
    const NAME: &'static str = "DestructiveClassifier";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        ctx.provide::<DestructiveClassifier>(Arc::new(DestructiveClassifierService::new(
            Arc::new(RealFs),
        )))
        .await?;
        Ok(())
    }
}
