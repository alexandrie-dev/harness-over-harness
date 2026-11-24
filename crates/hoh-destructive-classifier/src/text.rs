//! Approval wording for the classified action, its targets, and any irreversible consequences.
//! The headline fits the host protocol. The details carry the command and any model justification.

use std::path::Path;

use crate::classify::{GitKind, Risk, Source};

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
/// After a question that deletes files: they cannot be brought back.
const NO_UNDO: &str = ". Deleted files won’t go to the trash and cannot be recovered";

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
                    kind: GitKind::Clean | GitKind::Remove,
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
            format!("{OUTSIDE}: it can read and change any file on this computer")
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
/// first, then the folder it runs in, then that deleted files cannot be brought back.
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
        Risk::Truncate { names } => match names.len() {
            0 => "empty files and discard their contents".to_owned(),
            _ => format!(
                "empty {} and discard the previous contents",
                list(names, limit)
            ),
        },
        Risk::Overwrite { names, checked } => overwrite_phrase(names, *checked, limit),
        Risk::Patch { names } => match names.len() {
            0 => "apply a patch and overwrite the affected files, discarding their previous contents".to_owned(),
            1 => format!("apply a patch and overwrite {}, discarding its previous contents", list(names, limit)),
            _ => format!("apply a patch and overwrite {}, discarding their previous contents", list(names, limit)),
        },
        Risk::Extract { archive } => match archive {
            Some(name) => format!(
                "extract archive {} and overwrite files with matching names, discarding their previous contents",
                list(std::slice::from_ref(name), limit)
            ),
            None => "extract an archive and overwrite files with matching names, discarding their previous contents".to_owned(),
        },
        Risk::Git { kind, names } => git_phrase(*kind, names, limit),
        Risk::Opaque(what) => format!(
            "run an action that cannot be checked in advance ({what}). It may delete or overwrite files"
        ),
        Risk::Unparsable => {
            "run a command that could not be parsed. It may delete or overwrite files"
                .to_owned()
        }
    }
}

fn delete_phrase(names: &[String], folders: bool, source: Source, limit: usize) -> String {
    match source {
        Source::Find => "delete all files found by find".to_owned(),
        Source::Xargs => "delete files listed by xargs".to_owned(),
        Source::Sync => {
            "delete destination files missing from the source (rsync --delete)".to_owned()
        }
        Source::Listed if names.is_empty() => "delete files".to_owned(),
        Source::Listed => {
            let patterns = names.iter().any(|n| n.contains(['*', '?', '[', '(']));
            if patterns {
                format!(
                    "delete files matching names and patterns: {}",
                    list(names, limit)
                )
            } else if folders {
                format!(
                    "delete {} {} and their contents: {}",
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
        return "overwrite files whose names are only known at runtime".to_owned();
    }
    let noun = if names.len() == 1 { "file" } else { "files" };
    if checked {
        format!(
            "overwrite {noun} {} and discard the previous contents",
            list(names, limit)
        )
    } else {
        format!(
            "overwrite {noun} {} if already present and discard the previous contents. Existence could not be checked",
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
        GitKind::Clean => "delete untracked Git files".to_owned(),
        GitKind::ResetHard => "reset to the last commit and discard uncommitted changes".to_owned(),
        GitKind::Checkout | GitKind::Restore => {
            format!("restore the committed version of files{files} and discard uncommitted changes")
        }
        GitKind::Remove => format!("delete files from the folder and Git{files}"),
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
