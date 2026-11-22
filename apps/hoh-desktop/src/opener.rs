//! Opening what a chat answer links to: an http(s) address in the system browser, a file inside the
//! session's working folder with the system program, or its folder. Every check is here and pure,
//! so it is tested without a window; the Tauri command in `native` only asks the engine for the
//! session's folder and calls these. No shell is involved: `xdg-open` gets one argument.
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// What the person asked to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Url,
    File,
    Reveal,
}

impl Kind {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "url" => Ok(Self::Url),
            "file" => Ok(Self::File),
            "reveal" => Ok(Self::Reveal),
            _ => Err("Неизвестное действие".into()),
        }
    }
}

/// Only `http` and `https` addresses with a host; no credentials, spaces or control characters.
pub fn check_url(target: &str) -> Result<String, String> {
    let refused = || "Открыть можно только адрес http или https".to_owned();
    if target.len() > 2048 || target.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(refused());
    }
    let lower = target.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or_else(refused)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') || authority.starts_with(['-', ':']) {
        return Err(refused());
    }
    Ok(target.to_owned())
}

/// The canonical path of `target` when it is an existing file or folder inside `folder`.
/// `..` is refused outright, and symlinks are resolved before the containment check.
pub fn check_file(folder: &Path, target: &str) -> Result<PathBuf, String> {
    let outside = || "Файл вне рабочей папки: открыть его нельзя".to_owned();
    if target.is_empty()
        || target.contains('\0')
        || target.to_ascii_lowercase().starts_with("file:")
    {
        return Err(outside());
    }
    let path = Path::new(target);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(outside());
    }
    let root = folder
        .canonicalize()
        .map_err(|_| "Рабочая папка не найдена".to_owned())?;
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let real = match full.canonicalize() {
        Ok(real) => real,
        // The window shows only a file's name; a bare name that is not in the folder itself is looked up below it.
        Err(_) if path.components().count() == 1 => find_by_name(&root, target)?,
        Err(_) => return Err("Файл не найден".into()),
    };
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(outside())
    }
}

const SEARCH_DEPTH: usize = 6;
const SEARCH_ENTRIES: usize = 20_000;

/// The one file or folder called `name` below `root`. Symlinks are not followed while searching,
/// the search is bounded, and an ambiguous name is an error instead of a guess.
fn find_by_name(root: &Path, name: &str) -> Result<PathBuf, String> {
    let (mut level, mut seen, mut found) = (vec![root.to_owned()], 0usize, Vec::new());
    for _ in 0..SEARCH_DEPTH {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                seen += 1;
                if seen > SEARCH_ENTRIES {
                    return Err("Файл не найден".into());
                }
                let path = entry.path();
                if entry.file_name() == OsStr::new(name) {
                    found.push(path.clone());
                }
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    next.push(path);
                }
            }
        }
        if found.len() > 1 {
            return Err(
                "В папке несколько файлов с таким именем: откройте нужный из списка файлов".into(),
            );
        }
        if let Some(only) = found.pop() {
            return only.canonicalize().map_err(|_| "Файл не найден".to_owned());
        }
        level = next;
    }
    Err("Файл не найден".into())
}

/// The folder that holds a checked path (the path itself when it is a folder).
pub fn containing_folder(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_owned()
    } else {
        path.parent().unwrap_or(path).to_owned()
    }
}

/// A link the person clicked in the window that asks for a new window (`target="_blank"`, `window.open` inside a
/// click handler of the T3 client): the system browser gets it when it is a checked http(s) address. WebKit tells
/// whether a click or a key press caused the request; a script on its own opens nothing. The window never opens
/// a second page (native.rs, `main_security_plugin`).
pub fn clicked_link(url: &str, user_gesture: bool) -> Option<std::ffi::OsString> {
    if !user_gesture {
        return None;
    }
    check_url(url).ok().map(Into::into)
}

/// What to hand to `xdg-open` for a request, after every check.
pub fn resolve(kind: Kind, target: &str, folder: &Path) -> Result<std::ffi::OsString, String> {
    match kind {
        Kind::Url => check_url(target).map(Into::into),
        Kind::File => check_file(folder, target).map(PathBuf::into_os_string),
        Kind::Reveal => check_file(folder, target).map(|p| containing_folder(&p).into_os_string()),
    }
}

/// `xdg-open <argument>` without a shell; the argument is an absolute path or a checked http(s) address.
pub fn open(argument: &OsStr) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| {
            // Reap it in the background so no zombie is left; the person's program lives on its own.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
        .map_err(|_| "Не удалось открыть: в системе нет программы для этого".to_owned())
}
