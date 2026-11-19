//! The question shown to the user, in Russian, as the window's approval screen words it:
//! what will be done, to which files, and why it cannot be undone. The headline is one line
//! without control characters (the host protocol rejects anything else). The `explanation`
//! (the window's "Объяснение Помощника") is plain Russian with the full list of files and never
//! carries the raw command: that goes to `details`, folded in the window.

use std::path::Path;

use crate::classify::{GitKind, Risk, Source};

const MAX_NAMES: usize = 4;
const MAX_EXPLAINED_NAMES: usize = 20;
const MAX_NAME_CHARS: usize = 60;
const MAX_HEADLINE_CHARS: usize = 600;
const MAX_EXPLANATION_CHARS: usize = 2000;
const MAX_COMMAND_CHARS: usize = 2000;
const MAX_JUSTIFICATION_CHARS: usize = 1500;

pub struct Question {
    /// Becomes `humanText`: one line, what will happen and what cannot be undone.
    pub headline: String,
    /// Becomes `reason`: what will happen to which files, in words; no raw command.
    pub explanation: String,
    /// Becomes `details` (folded in the window): the command as the assistant wrote it
    /// (`Команда: …`) and, for an escalation, the assistant's justification after it. At most
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
/// "danger-full-access"`): the same wording as [`question`], saying that nothing confines the
/// command. `risks` may be empty: then the command is simply unconfined.
pub fn escalation_question(risks: &[Risk], command: &str, justification: &str) -> Question {
    let wording = |limit: usize| {
        if risks.is_empty() {
            "она сможет читать и менять любые файлы на этом компьютере".to_owned()
        } else {
            phrases(risks, limit).join("; ")
        }
    };
    let mut headline = format!(
        "Помощник просит выполнить команду без ограничений песочницы: {}",
        wording(MAX_NAMES)
    );
    let mut explanation = format!(
        "Помощник просит выполнить команду без ограничений песочницы: {}",
        wording(MAX_EXPLAINED_NAMES)
    );
    if drops_files(risks) {
        headline.push_str(". Файлы не попадут в корзину");
        explanation.push_str(". Файлы не попадут в корзину: вернуть их будет нельзя");
    }
    // The model's own words never go into the explanation (they may quote the command in any
    // spelling): they sit with the command, in the details.
    explanation.push_str(". Обоснование Помощника — в подробностях");
    Question {
        headline: truncate(&clean(&headline), MAX_HEADLINE_CHARS),
        explanation: truncate(&clean(&explanation), MAX_EXPLANATION_CHARS),
        details: format!(
            "{}\nОбоснование Помощника: {}",
            command_line(command),
            truncate(justification, MAX_JUSTIFICATION_CHARS)
        ),
    }
}

pub fn question(risks: &[Risk], command: &str, cwd: Option<&Path>) -> Question {
    let only_overwrites = risks.iter().all(|r| {
        matches!(
            r,
            Risk::Overwrite { .. }
                | Risk::Truncate { .. }
                | Risk::Extract { .. }
                | Risk::Patch { .. }
        )
    });
    let folder = cwd.and_then(folder_name);
    let wording = |limit: usize| {
        let phrases = phrases(risks, limit);
        if only_overwrites {
            let mut text = capitalize(&phrases.join("; "));
            if let Some(folder) = &folder {
                text.push_str(&format!(" (папка {folder})"));
            }
            text
        } else {
            let place = folder
                .as_ref()
                .map_or(String::new(), |f| format!(" в папке {f}"));
            format!("Выполнить команду{place}: {}", phrases.join("; "))
        }
    };
    let mut headline = wording(MAX_NAMES);
    let mut explanation = wording(MAX_EXPLAINED_NAMES);
    if drops_files(risks) {
        headline.push_str(". Файлы не попадут в корзину");
        explanation.push_str(". Файлы не попадут в корзину: вернуть их будет нельзя");
    }
    Question {
        headline: truncate(&clean(&headline), MAX_HEADLINE_CHARS),
        explanation: truncate(&clean(&explanation), MAX_EXPLANATION_CHARS),
        details: command_line(command),
    }
}

fn command_line(command: &str) -> String {
    truncate(&format!("Команда: {command}"), MAX_COMMAND_CHARS)
}

fn phrase(risk: &Risk, limit: usize) -> String {
    match risk {
        Risk::Delete {
            names,
            folders,
            source,
        } => delete_phrase(names, *folders, *source, limit),
        Risk::Truncate { names } => match names.len() {
            0 => "очистить файлы: прежнее содержимое будет потеряно".to_owned(),
            _ => format!(
                "очистить {}: прежнее содержимое будет потеряно",
                list(names, limit)
            ),
        },
        Risk::Overwrite { names, checked } => overwrite_phrase(names, *checked, limit),
        Risk::Patch { names } => match names.len() {
            0 => "применить патч и перезаписать файлы, которые он меняет: прежнее содержимое будет потеряно".to_owned(),
            1 => format!("применить патч и перезаписать файл {}: прежнее содержимое будет потеряно", list(names, limit)),
            _ => format!("применить патч и перезаписать файлы {}: прежнее содержимое будет потеряно", list(names, limit)),
        },
        Risk::Extract { archive } => match archive {
            Some(name) => format!(
                "распаковать архив {} и перезаписать файлы с теми же именами: прежнее содержимое будет потеряно",
                list(std::slice::from_ref(name), limit)
            ),
            None => "распаковать архив и перезаписать файлы с теми же именами: прежнее содержимое будет потеряно".to_owned(),
        },
        Risk::Git { kind, names } => git_phrase(*kind, names, limit),
        Risk::Opaque(what) => format!(
            "запустить то, что нельзя проверить заранее ({what}); оно может удалить или перезаписать файлы"
        ),
        Risk::Unparsable => {
            "запустить команду, которую не удалось разобрать; она может удалить или перезаписать файлы"
                .to_owned()
        }
    }
}

fn delete_phrase(names: &[String], folders: bool, source: Source, limit: usize) -> String {
    match source {
        Source::Find => "удалить все файлы, которые найдёт команда find".to_owned(),
        Source::Xargs => "удалить файлы из списка, переданного команде xargs".to_owned(),
        Source::Sync => {
            "удалить в папке назначения файлы, которых нет в источнике (rsync --delete)".to_owned()
        }
        Source::Listed if names.is_empty() => "удалить файлы".to_owned(),
        Source::Listed => {
            let patterns = names.iter().any(|n| n.contains(['*', '?', '[', '(']));
            if patterns {
                format!("удалить файлы по именам и маскам: {}", list(names, limit))
            } else if folders {
                format!(
                    "удалить {} {} вместе со всем содержимым: {}",
                    names.len(),
                    plural(names.len(), "элемент", "элемента", "элементов"),
                    list(names, limit)
                )
            } else {
                format!(
                    "удалить {} {}: {}",
                    names.len(),
                    plural(names.len(), "файл", "файла", "файлов"),
                    list(names, limit)
                )
            }
        }
    }
}

fn overwrite_phrase(names: &[String], checked: bool, limit: usize) -> String {
    if names.is_empty() {
        return "перезаписать файлы, список которых станет известен только при запуске".to_owned();
    }
    let noun = if names.len() == 1 {
        "файл"
    } else {
        "файлы"
    };
    if checked {
        format!(
            "перезаписать {noun} {}: прежнее содержимое будет потеряно",
            list(names, limit)
        )
    } else {
        format!(
            "перезаписать {noun} {}, если он уже существует (проверить заранее нельзя): прежнее содержимое будет потеряно",
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
        GitKind::Clean => "удалить файлы, которые git не отслеживает".to_owned(),
        GitKind::ResetHard => {
            "сбросить все несохранённые изменения до последнего коммита; изменения пропадут"
                .to_owned()
        }
        GitKind::Checkout | GitKind::Restore => {
            format!("вернуть файлы к сохранённой версии{files}; несохранённые изменения пропадут")
        }
        GitKind::Remove => format!("удалить файлы из папки и из git{files}"),
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
        text.push_str(&format!(" и ещё {}", names.len() - limit));
    }
    text
}

/// Russian plural form for `n`: 1 файл, 2 файла, 5 файлов.
fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    match (n % 100, n % 10) {
        (11..=14, _) => many,
        (_, 1) => one,
        (_, 2..=4) => few,
        _ => many,
    }
}

fn folder_name(cwd: &Path) -> Option<String> {
    let parts: Vec<String> = cwd
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        [] => None,
        [only] => Some(only.clone()),
        [.., parent, last] => Some(format!("{parent}/{last}")),
    }
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
        | '\u{17b4}'..='\u{17b5}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{3164}'
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
