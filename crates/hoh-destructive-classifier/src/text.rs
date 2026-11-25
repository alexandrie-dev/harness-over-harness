//! Approval wording for the classified action, its targets, and any irreversible consequences.
//! The headline fits the host protocol. The details carry the command and any model justification.

use std::path::Path;

use crate::classify::{GitKind, RestoreSource, Risk, Source};

const MAX_NAMES: usize = 4;
const MAX_EXPLAINED_NAMES: usize = 20;
const MAX_NAME_CHARS: usize = 60;
const MAX_HEADLINE_CHARS: usize = 600;
const MAX_EXPLANATION_CHARS: usize = 2000;
const MAX_COMMAND_CHARS: usize = 2000;
const MAX_JUSTIFICATION_CHARS: usize = 1500;

/// How an escalation starts: the assistant asks to work outside its folder.
const OUTSIDE: &str = "The assistant asks to work outside its folder";
/// Where an escalation's explanation sends the reader for the assistant's own reasons.
const JUSTIFICATION_HINT: &str = "See details for the assistant’s justification";
/// Shell and Git removal do not use the desktop trash.
const NO_UNDO: &str = ". Removed files bypass the trash and may be unrecoverable";

pub struct Question {
    /// Becomes `humanText`: one line, what will happen and what cannot be undone.
    pub headline: String,
    /// Becomes `reason`: what the headline does not say, in words; no raw command. A question
    /// whose headline already says everything has the headline here, word for word, so the window
    /// can show the text once.
    pub explanation: String,
    /// Becomes `details` (folded in the window): the command as the assistant wrote it
    /// (`Command: …`) and, for an escalation, the assistant's justification after it. At most
    /// 3600 characters, within the protocol's 8192 UTF-16 units.
    pub details: String,
}

fn phrases(risks: &[Risk], limit: usize) -> Vec<String> {
    let mut phrases: Vec<String> = Vec::new();
    for risk in risks {
        let phrase = phrase(risk, limit);
        if !phrases.contains(&phrase) {
            phrases.push(phrase);
        }
    }
    phrases
}

fn drops_files(risks: &[Risk]) -> bool {
    risks.iter().any(|r| {
        matches!(
            r,
            Risk::Delete { .. }
                | Risk::Git {
                    kind: GitKind::Clean
                        | GitKind::CleanIgnored
                        | GitKind::CleanWithIgnored
                        | GitKind::Remove,
                    ..
                }
        )
    })
}

/// The question for a call that asks to leave the sandbox (`sandbox_permissions:
/// "danger-full-access"`): what the command does, worded as in [`question`], or, when the
/// classifier finds nothing, that it may read and change any file. `risks` may be empty.
pub fn escalation_question(risks: &[Risk], command: &str, justification: &str) -> Question {
    let wording = |limit: usize| {
        let mut text = if risks.is_empty() {
            format!(
                "{OUTSIDE}: it may read and change files outside its folder with the current user’s permissions"
            )
        } else {
            format!("{OUTSIDE}: {}", phrases(risks, limit).join("; "))
        };
        if drops_files(risks) {
            text.push_str(NO_UNDO);
        }
        text
    };
    let headline = truncate(&clean(&wording(MAX_NAMES)), MAX_HEADLINE_CHARS);
    let full = truncate(&clean(&wording(MAX_EXPLAINED_NAMES)), MAX_EXPLANATION_CHARS);
    // The explanation does not repeat the headline: the whole list only when the headline had to
    // shorten it, then where the assistant's reasons are. The model's own words never go into it
    // (they may quote the command in any spelling): they sit with the command, in the details.
    let explanation = if full == headline {
        JUSTIFICATION_HINT.to_owned()
    } else {
        truncate(
            &format!("{full}. {JUSTIFICATION_HINT}"),
            MAX_EXPLANATION_CHARS,
        )
    };
    Question {
        headline,
        explanation,
        details: format!(
            "{}\nAssistant justification: {}",
            command_line(command),
            truncate(justification, MAX_JUSTIFICATION_CHARS)
        ),
    }
}

/// The question for a command (or a file change) that may delete or overwrite files: the action
/// first, then the folder it runs in and the risk of losing deleted files.
pub fn question(risks: &[Risk], command: &str, cwd: Option<&Path>) -> Question {
    let folder = cwd.and_then(folder_name);
    let wording = |limit: usize| {
        let mut text = capitalize(&phrases(risks, limit).join("; "));
        if let Some(folder) = &folder {
            text.push_str(&format!(" (folder {folder})"));
        }
        if drops_files(risks) {
            text.push_str(NO_UNDO);
        }
        text
    };
    // The explanation names every file (up to twenty) where the headline stops at four; with no
    // more to name, it is the headline itself.
    Question {
        headline: truncate(&clean(&wording(MAX_NAMES)), MAX_HEADLINE_CHARS),
        explanation: truncate(&clean(&wording(MAX_EXPLAINED_NAMES)), MAX_EXPLANATION_CHARS),
        details: command_line(command),
    }
}

fn command_line(command: &str) -> String {
    truncate(&format!("Command: {command}"), MAX_COMMAND_CHARS)
}

fn phrase(risk: &Risk, limit: usize) -> String {
    match risk {
        Risk::Delete {
            names,
            folders,
            source,
        } => delete_phrase(names, *folders, *source, limit),
        Risk::Truncate { names } => {
            let files = if names.is_empty() {
                "files".to_owned()
            } else {
                list(names, limit)
            };
            format!("Resize {files}. If the files shrink, data past the new size is discarded")
        }
        Risk::Overwrite { names, checked } => overwrite_phrase(names, *checked, limit),
        Risk::MoveOverwrite {
            sources,
            names,
            checked,
        } => {
            let source = if sources.is_empty() {
                "files".to_owned()
            } else {
                list(sources, limit)
            };
            format!(
                "Move {source} to a new location. This can {}",
                overwrite_phrase(names, *checked, limit)
            )
        }
        Risk::Patch { names } => {
            let files = if names.is_empty() {
                String::new()
            } else {
                format!(" to {}", list(names, limit))
            };
            format!(
                "Apply a patch{files}. It may change or remove existing files and discard their previous contents"
            )
        }
        Risk::Extract { archive } => {
            let archive = archive.as_ref().map_or_else(
                || "an archive".to_owned(),
                |name| format!("archive {}", list(std::slice::from_ref(name), limit)),
            );
            format!(
                "Extract {archive}. Existing destination files with matching paths may be replaced"
            )
        }
        Risk::Git { kind, names } => git_phrase(*kind, names, limit),
        Risk::Opaque(what) => format!(
            "run an action that cannot be checked in advance ({what}). It may delete or overwrite files"
        ),
        Risk::Unparsable => {
            "run a command that could not be parsed. It may delete or overwrite files".to_owned()
        }
    }
}

fn delete_phrase(names: &[String], folders: bool, source: Source, limit: usize) -> String {
    match source {
        Source::Find => "delete paths selected by the removal action in find".to_owned(),
        Source::Xargs => "delete paths passed to the removal command by xargs".to_owned(),
        Source::Sync => "sync files with rsync. The deletion options may remove destination paths".to_owned(),
        Source::SyncSender => "transfer files with rsync and remove successfully duplicated files from the sender".to_owned(),
        Source::SyncBoth => "sync files with rsync. This can remove successfully duplicated files from the sender and remove paths at the destination".to_owned(),
        Source::Listed if names.is_empty() => "delete files".to_owned(),
        Source::Listed => {
            let patterns = names.iter().any(|n| n.contains(['*', '?', '[', '(']));
            if patterns {
                format!(
                    "delete paths matching the listed names and patterns: {}",
                    list(names, limit)
                )
            } else if folders {
                format!(
                    "delete {} {}: {}. With recursive deletion, directories and their contents can be removed",
                    names.len(),
                    if names.len() == 1 { "entry" } else { "entries" },
                    list(names, limit)
                )
            } else {
                format!(
                    "delete {} {}: {}",
                    names.len(),
                    if names.len() == 1 { "file" } else { "files" },
                    list(names, limit)
                )
            }
        }
    }
}

fn overwrite_phrase(names: &[String], checked: bool, limit: usize) -> String {
    if names.is_empty() {
        return "overwrite existing destination files. Their names are determined at runtime"
            .to_owned();
    }
    let noun = if names.len() == 1 { "file" } else { "files" };
    if checked {
        format!(
            "overwrite {noun} {}. Previous contents may be lost",
            list(names, limit)
        )
    } else {
        format!(
            "overwrite {noun} {} if present. Previous contents may be lost",
            list(names, limit)
        )
    }
}

fn git_phrase(kind: GitKind, names: &[String], limit: usize) -> String {
    let files = if names.is_empty() {
        String::new()
    } else {
        format!(": {}", list(names, limit))
    };
    match kind {
        GitKind::Clean => format!("Clean the working tree by deleting selected untracked paths{files}. Ignored paths are normally spared"),
        GitKind::CleanIgnored => format!("Clean the working tree by deleting selected ignored paths{files}"),
        GitKind::CleanWithIgnored => format!("Clean the working tree by deleting selected untracked paths{files}, which can include ignored paths"),
        GitKind::ResetHard => "Reset the current branch or detached HEAD, the index, and the working tree to the chosen commit (HEAD by default). Discard staged and working-tree changes. Untracked paths in the way may be overwritten or removed".to_owned(),
        GitKind::ResetUnknown => "Run git reset with options determined at runtime. Depending on those options, it can move HEAD, change the index, and discard working-tree changes".to_owned(),
        GitKind::Checkout => format!("Check out selected paths from the Git index{files}, overwriting the working copies"),
        GitKind::CheckoutTree => format!("Check out selected paths from the specified Git tree{files}, replacing their index entries and working copies"),
        GitKind::CheckoutPaths => format!("Check out selected paths{files}. Depending on the source and options, this can replace working copies and index entries"),
        GitKind::CheckoutUnknown => format!("Run git checkout with the requested options{files}. It may switch branches or replace paths in the index or working tree and discard local changes. A forced switch may remove untracked paths in the way"),
        GitKind::SwitchDiscard => "Switch to the selected branch or commit and reset the index and working tree to match. Local changes are discarded, and untracked paths in the way may be removed".to_owned(),
        GitKind::Restore => format!("Restore selected paths from the Git index (the last commit if those entries match HEAD), overwriting the working copies{files}. Paths absent from the source may be removed"),
        GitKind::RestoreWithOptions { source, staged, worktree, patch, overlay } => {
            let source = match source {
                RestoreSource::Index => "the Git index",
                RestoreSource::Head => "HEAD (the current commit)",
                RestoreSource::SpecifiedTree => "the specified Git tree",
            };
            let target = match (staged, worktree) {
                (true, true) => "the index and working tree",
                (true, false) => "the index",
                _ => "the working tree",
            };
            let selected = if patch { "selected changes to paths" } else { "selected paths" };
            let effect = if overlay { "overwritten" } else { "overwritten or removed" };
            format!("Restore {selected} from {source} in {target}{files}. The affected contents can be {effect}")
        }
        GitKind::RestoreUnknown => format!("Run git restore with the requested source and destination options{files}. It may overwrite or remove paths in the Git index or working tree"),
        GitKind::Remove => format!("Remove selected paths from the Git index and working tree{files}"),
        GitKind::RemovePreview => format!("Preview the paths git rm would remove from the index and working tree{files}"),
    }
}

fn list(names: &[String], limit: usize) -> String {
    let shown: Vec<String> = names
        .iter()
        .take(limit)
        .map(|name| truncate(&clean(name), MAX_NAME_CHARS))
        .collect();
    let mut text = shown.join(", ");
    if names.len() > limit {
        text.push_str(&format!(" and {} more", names.len() - limit));
    }
    text
}

/// The folder as the question names it: the whole path, the home folder written as `~`
/// (`~/Documents/HoH`, not just `Documents/HoH`: two folders with the same last names must not
/// read the same).
fn folder_name(cwd: &Path) -> Option<String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    folder_display(cwd, home.as_deref())
}

fn folder_display(cwd: &Path, home: Option<&Path>) -> Option<String> {
    if cwd.as_os_str().is_empty() {
        return None;
    }
    let under_home = home
        .filter(|home| home.is_absolute() && home.parent().is_some())
        .and_then(|home| cwd.strip_prefix(home).ok());
    Some(match under_home {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => cwd.display().to_string(),
    })
}

/// Anything the protocol refuses in `humanText` becomes a space; the rest of the text is kept.
fn clean(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|c| {
            if c.is_control() || (c.is_whitespace() && c != ' ') || invisible(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    mapped
        .split(' ')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn invisible(c: char) -> bool {
    matches!(c, '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{115f}' | '\u{1160}'
        | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}' | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{2800}' | '\u{3164}'
        | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{ffa0}' | '\u{fff9}'..='\u{fffb}'
        | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
        | '\u{e0000}'..='\u{e007f}' | '\u{e0100}'..='\u{e01ef}')
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{clean, folder_display};
    use std::path::Path;

    #[test]
    fn a_character_the_protocol_refuses_becomes_a_space() {
        for c in [
            '\u{180b}', '\u{180f}', '\u{2800}', '\u{200b}', '\u{fe0f}', '\u{a0}',
        ] {
            assert_eq!(clean(&format!("rm a{c}b")), "rm a b", "{:?}", c);
        }
    }

    #[test]
    fn the_folder_is_named_by_its_whole_path_with_home_as_tilde() {
        let home = Some(Path::new("/home/u"));
        let shown = |cwd: &str| folder_display(Path::new(cwd), home);
        assert_eq!(
            shown("/home/u/Documents/HoH").as_deref(),
            Some("~/Documents/HoH")
        );
        assert_eq!(shown("/home/u").as_deref(), Some("~"));
        assert_eq!(
            shown("/home/u2/Documents/HoH").as_deref(),
            Some("/home/u2/Documents/HoH")
        );
        assert_eq!(
            shown("/srv/a/b/Documents/HoH").as_deref(),
            Some("/srv/a/b/Documents/HoH")
        );
        assert_eq!(shown("/").as_deref(), Some("/"));
        assert_eq!(shown(""), None);
        // A home of `/` would turn every path into `~/…`: it is not used.
        assert_eq!(
            folder_display(Path::new("/etc"), Some(Path::new("/"))).as_deref(),
            Some("/etc")
        );
    }
}
