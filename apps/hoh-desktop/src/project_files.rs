//! Native file and editor actions for a project authorized by the HoH engine.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    path: String,
    kind: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntries {
    entries: Vec<ProjectEntry>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectText {
    relative_path: String,
    contents: String,
    byte_length: u64,
    truncated: bool,
    exists: bool,
}

#[derive(Debug, Serialize)]
pub struct FileError {
    pub(crate) failure: &'static str,
    pub(crate) message: String,
}

impl FileError {
    pub(crate) fn operation(message: impl Into<String>) -> Self {
        Self {
            failure: "operation_failed",
            message: message.into(),
        }
    }
}

#[derive(Deserialize)]
pub struct EditorPosition {
    line: Option<u32>,
    column: Option<u32>,
}

#[cfg(any(target_os = "linux", all(target_os = "macos", feature = "desktop")))]
pub(crate) use unix::PrivateDirectory;
#[cfg(target_os = "linux")]
pub(crate) use unix::open_project_file;
#[cfg(all(any(target_os = "linux", target_os = "macos"), feature = "desktop"))]
pub(crate) use unix::{available_editors, list, open_editor, project_root, read_text};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix {
    #[cfg(target_os = "macos")]
    use super::macos::{
        descriptor_path, open_project_directory, open_project_entry, visit_directory,
    };
    use super::{EditorPosition, FileError, ProjectEntries, ProjectEntry, ProjectText};
    use crate::engine::Engine;
    use crate::image_asset::ProjectRootIdentity;
    #[cfg(target_os = "linux")]
    use crate::image_asset::{descriptor_path, open_project_directory, open_project_entry};
    use hoh_protocol::WorkspaceResolveResult;
    use serde_json::json;
    use std::ffi::OsString;
    use std::fs::{self, File, Metadata};
    use std::io::{self, Read};
    #[cfg(target_os = "linux")]
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Component, Path, PathBuf};
    use std::process::{Command, Stdio};

    const MAX_PATH_BYTES: usize = 4096;
    const MAX_TEXT_BYTES: u64 = 1024 * 1024;
    const MAX_DIRECTORY_ENTRIES: usize = 4096;

    /// The desktop's temporary secret directory may be nested under a user-selected workspace.
    #[derive(Default)]
    pub(crate) struct PrivateDirectory {
        paths: Vec<PathBuf>,
    }

    impl PrivateDirectory {
        pub(crate) fn new(configured: &Path) -> Self {
            let resolved = fs::canonicalize(configured).ok().or_else(|| {
                // The secret directory is created on the first write.
                let parent = fs::canonicalize(configured.parent()?).ok()?;
                Some(parent.join(configured.file_name()?))
            });
            let mut paths = vec![configured.to_owned()];
            if let Some(resolved) = resolved
                && resolved != configured
            {
                paths.push(resolved);
            }
            Self { paths }
        }

        pub(crate) fn contains(&self, path: &Path) -> bool {
            self.paths
                .iter()
                .any(|directory| path_starts_with(path, directory))
        }

        pub(crate) fn overlaps(&self, path: &Path) -> bool {
            self.contains(path)
                || self
                    .paths
                    .iter()
                    .any(|directory| path_starts_with(directory, path))
        }
    }

    fn path_starts_with(path: &Path, prefix: &Path) -> bool {
        if cfg!(target_os = "macos") {
            let mut parts = path.components();
            prefix.components().all(|other| {
                parts.next().is_some_and(|part| {
                    part.as_os_str()
                        .as_encoded_bytes()
                        .eq_ignore_ascii_case(other.as_os_str().as_encoded_bytes())
                })
            })
        } else {
            path.starts_with(prefix)
        }
    }

    /// The input is a lookup key. The engine returns the resolved path and the identity
    /// that must match the directory opened by the desktop host.
    pub(crate) fn project_root(
        engine: &Engine,
        cwd: &str,
    ) -> Result<(PathBuf, ProjectRootIdentity), String> {
        if cwd.len() > MAX_PATH_BYTES || !Path::new(cwd).is_absolute() {
            return Err("The project folder must be an absolute path.".into());
        }
        let answer: WorkspaceResolveResult =
            serde_json::from_value(engine.request("hoh/workspace/resolve", json!({ "cwd": cwd }))?)
                .map_err(|_| "The project folder identity is unknown.".to_owned())?;
        let root = PathBuf::from(answer.cwd);
        if root != Path::new(cwd) {
            return Err("The project folder has changed. Open the project again.".into());
        }
        if root
            .components()
            .any(|part| matches!(part, Component::Normal(name) if hidden_component(name)))
        {
            return Err("The project folder is protected.".into());
        }
        let parse = |part: &str| {
            part.parse::<u64>()
                .map_err(|_| "The project folder identity is invalid.".to_owned())
        };
        let identity = ProjectRootIdentity {
            device: parse(&answer.root_identity.device)?,
            inode: parse(&answer.root_identity.inode)?,
        };
        Ok((root, identity))
    }

    fn hidden_component(component: &std::ffi::OsStr) -> bool {
        [".git", ".claude", ".codex", ".ssh", ".gnupg"]
            .iter()
            .any(|name| {
                if cfg!(target_os = "macos") {
                    component
                        .as_encoded_bytes()
                        .eq_ignore_ascii_case(name.as_bytes())
                } else {
                    component == *name
                }
            })
    }

    fn relative_path(
        root: &Path,
        path: &str,
        private: &PrivateDirectory,
    ) -> Result<PathBuf, FileError> {
        let refused = || FileError {
            failure: "workspace_path_outside_root",
            message: "The path must refer to a file or folder inside the project.".into(),
        };
        if path.len() > MAX_PATH_BYTES || path.contains(['\0', '\\']) {
            return Err(refused());
        }
        let path = Path::new(path);
        let path = if path.is_absolute() {
            path.strip_prefix(root).map_err(|_| refused())?
        } else {
            path
        };
        let mut relative = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(name) if !hidden_component(name) => relative.push(name),
                _ => return Err(refused()),
            }
        }
        if private.contains(&root.join(&relative)) {
            return Err(refused());
        }
        Ok(relative)
    }

    #[cfg(target_os = "linux")]
    fn fd_path(handle: &File) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", handle.as_raw_fd()))
    }

    fn matches_path(handle: &File, expected: &Path) -> Result<(), FileError> {
        if descriptor_path(handle, "project entry").ok().as_deref() != Some(expected) {
            return Err(FileError::operation(
                "The project entry changed while it was open.",
            ));
        }
        Ok(())
    }

    fn entry_error(error: io::Error) -> FileError {
        if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::EXDEV)) {
            return FileError {
                failure: "resolved_path_outside_root",
                message: "The path crosses a symbolic link or filesystem boundary.".into(),
            };
        }
        FileError::operation(format!("The project entry could not be opened: {error}"))
    }

    fn open_entry(directory: &File, relative: &Path, root: &Path) -> io::Result<File> {
        let lookup = if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        };
        let entry = open_project_entry(directory, lookup)?;
        matches_path(&entry, &root.join(relative))
            .map_err(|error| io::Error::other(error.message))?;
        Ok(entry)
    }

    #[cfg(target_os = "linux")]
    fn visit_directory(
        directory: &File,
        limit: usize,
        mut visit: impl FnMut(OsString, &'static str),
    ) -> io::Result<bool> {
        for (index, child) in fs::read_dir(fd_path(directory))?.enumerate() {
            if index == limit {
                return Ok(true);
            }
            let Ok(child) = child else { continue };
            let Ok(kind) = child.file_type() else {
                continue;
            };
            if kind.is_dir() {
                visit(child.file_name(), "directory");
            } else if kind.is_file() {
                visit(child.file_name(), "file");
            }
        }
        Ok(false)
    }

    pub(crate) fn list(
        root: &Path,
        identity: ProjectRootIdentity,
        path: &str,
        private: &PrivateDirectory,
    ) -> Result<ProjectEntries, FileError> {
        let relative = relative_path(root, path, private)?;
        let directory = open_project_directory(root, identity).map_err(FileError::operation)?;
        let entry = open_entry(&directory, &relative, root).map_err(entry_error)?;
        let mut entries = Vec::new();
        let truncated = visit_directory(&entry, MAX_DIRECTORY_ENTRIES, |name, kind| {
            if hidden_component(&name) {
                return;
            }
            let path = relative.join(name);
            if private.contains(&root.join(&path)) {
                return;
            }
            let Some(path) = path.to_str().filter(|path| {
                path.trim() == *path && !path.contains('\\') && path.len() <= MAX_PATH_BYTES
            }) else {
                return;
            };
            entries.push(ProjectEntry {
                path: path.into(),
                kind,
            });
        })
        .map_err(|error| {
            FileError::operation(format!("The project folder could not be read: {error}"))
        })?;
        matches_path(&directory, root)?;
        matches_path(&entry, &root.join(relative))?;
        entries.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        Ok(ProjectEntries { entries, truncated })
    }

    fn unchanged(before: &Metadata, after: &Metadata) -> bool {
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.nlink() == after.nlink()
            && before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }

    fn open_regular(handle: &File, metadata: &Metadata) -> io::Result<File> {
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the project entry is not a single-link regular file",
            ));
        }
        #[cfg(target_os = "linux")]
        let file = File::open(fd_path(handle))?;
        #[cfg(target_os = "macos")]
        let file = handle.try_clone()?;
        if !unchanged(metadata, &file.metadata()?) {
            return Err(io::Error::other(
                "the project file changed while it was opened",
            ));
        }
        Ok(file)
    }

    /// Open a regular file below an already authorized project descriptor.
    #[cfg(target_os = "linux")]
    pub(crate) fn open_project_file(directory: &File, relative: &Path) -> io::Result<File> {
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(name) if !hidden_component(name)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid project file path",
            ));
        }
        let entry = open_project_entry(directory, relative)?;
        open_regular(&entry, &entry.metadata()?)
    }

    pub(crate) fn read_text(
        root: &Path,
        identity: ProjectRootIdentity,
        path: &str,
        allow_missing: bool,
        private: &PrivateDirectory,
    ) -> Result<ProjectText, FileError> {
        let relative = relative_path(root, path, private)?;
        let relative_path = if relative.as_os_str().is_empty() {
            ".".into()
        } else {
            relative.to_string_lossy().into_owned()
        };
        let directory = open_project_directory(root, identity).map_err(FileError::operation)?;
        let handle = match open_entry(&directory, &relative, root) {
            Ok(handle) => handle,
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
                return Ok(ProjectText {
                    relative_path,
                    contents: String::new(),
                    byte_length: 0,
                    truncated: false,
                    exists: false,
                });
            }
            Err(error) => return Err(entry_error(error)),
        };
        let before = handle.metadata().map_err(entry_error)?;
        if !before.is_file() {
            return Err(FileError {
                failure: "path_not_file",
                message: "The project entry is not a regular file.".into(),
            });
        }
        if before.nlink() != 1 {
            return Err(FileError::operation(
                "Files with multiple hard links cannot be previewed.",
            ));
        }
        let mut bytes = Vec::new();
        let file = open_regular(&handle, &before).map_err(entry_error)?;
        (&file)
            .take(MAX_TEXT_BYTES)
            .read_to_end(&mut bytes)
            .map_err(entry_error)?;
        let after = file.metadata().map_err(entry_error)?;
        let truncated = after.len() > MAX_TEXT_BYTES;
        if !unchanged(&before, &after) || bytes.len() as u64 != after.len().min(MAX_TEXT_BYTES) {
            return Err(FileError::operation(
                "The project file changed while it was being read.",
            ));
        }
        matches_path(&directory, root)?;
        matches_path(&handle, &root.join(relative))?;
        let binary = || FileError {
            failure: "binary_file",
            message: "This file cannot be previewed as UTF-8 text.".into(),
        };
        if bytes.contains(&0) {
            return Err(binary());
        }
        let contents = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) if truncated && error.utf8_error().error_len().is_none() => {
                let end = error.utf8_error().valid_up_to();
                let mut bytes = error.into_bytes();
                bytes.truncate(end);
                String::from_utf8(bytes).map_err(|_| binary())?
            }
            Err(_) => return Err(binary()),
        };
        Ok(ProjectText {
            relative_path,
            contents,
            byte_length: before.len(),
            truncated,
            exists: true,
        })
    }

    #[derive(Clone, Copy)]
    enum Launch {
        Path,
        Goto,
        LineColumn,
        Folder,
    }

    struct Editor {
        id: &'static str,
        commands: &'static [&'static str],
        base_args: &'static [&'static str],
        launch: Launch,
    }

    impl Editor {
        const fn simple(
            id: &'static str,
            commands: &'static [&'static str],
            launch: Launch,
        ) -> Self {
            Self {
                id,
                commands,
                base_args: &[],
                launch,
            }
        }
    }

    // IDs and launch shapes correspond to the installed T3 Open In menu contracts.
    const EDITORS: &[Editor] = &[
        Editor {
            id: "cursor",
            commands: &["cursor"],
            base_args: &["--classic"],
            launch: Launch::Goto,
        },
        Editor::simple("trae", &["trae"], Launch::Goto),
        Editor {
            id: "kiro",
            commands: &["kiro"],
            base_args: &["ide"],
            launch: Launch::Goto,
        },
        Editor::simple("vscode", &["code"], Launch::Goto),
        Editor::simple("vscode-insiders", &["code-insiders"], Launch::Goto),
        Editor::simple("vscodium", &["codium"], Launch::Goto),
        Editor::simple("zed", &["zed", "zeditor"], Launch::Path),
        Editor::simple("antigravity", &["antigravity-ide", "agy-ide"], Launch::Goto),
        Editor::simple("idea", &["idea"], Launch::LineColumn),
        Editor::simple("aqua", &["aqua"], Launch::LineColumn),
        Editor::simple("clion", &["clion"], Launch::LineColumn),
        Editor::simple("datagrip", &["datagrip"], Launch::LineColumn),
        Editor::simple("dataspell", &["dataspell"], Launch::LineColumn),
        Editor::simple("goland", &["goland"], Launch::LineColumn),
        Editor::simple("phpstorm", &["phpstorm"], Launch::LineColumn),
        Editor::simple("pycharm", &["pycharm"], Launch::LineColumn),
        Editor::simple("rider", &["rider"], Launch::LineColumn),
        Editor::simple("rubymine", &["rubymine"], Launch::LineColumn),
        Editor::simple("rustrover", &["rustrover"], Launch::LineColumn),
        Editor::simple("webstorm", &["webstorm"], Launch::LineColumn),
        #[cfg(target_os = "linux")]
        Editor::simple("file-manager", &["xdg-open"], Launch::Folder),
        #[cfg(target_os = "macos")]
        Editor::simple("file-manager", &["/usr/bin/open"], Launch::Folder),
    ];

    fn executable(commands: &[&str]) -> Option<PathBuf> {
        let available = |path: &Path| {
            fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.mode() & 0o111 != 0)
        };
        let search = std::env::var_os("PATH").unwrap_or_default();
        commands.iter().find_map(|command| {
            let path = Path::new(command);
            if path.is_absolute() {
                return available(path).then(|| path.to_owned());
            }
            let paths = std::env::split_paths(&search);
            // Finder launches with a short PATH. Use the same package-manager locations
            // that make provider binaries visible when HoH is launched as an app bundle.
            #[cfg(target_os = "macos")]
            let paths = paths.chain([
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ]);
            paths
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(command))
                .find(|path| available(path))
        })
    }

    pub(crate) fn available_editors() -> Vec<&'static str> {
        EDITORS
            .iter()
            .filter(|editor| executable(editor.commands).is_some())
            .map(|editor| editor.id)
            .collect()
    }

    fn diagnostic_target(path: &str) -> Option<(&str, EditorPosition)> {
        let positive = |text: &str| text.parse::<u32>().ok().filter(|value| *value > 0);
        let (prefix, last) = path.rsplit_once(':')?;
        let (path, line, column) = if let Some((first, end)) = last.split_once('-') {
            positive(end)?;
            (prefix, positive(first)?, None)
        } else if let Some((path, line)) = prefix.rsplit_once(':')
            && let Some(line) = positive(line)
        {
            (path, line, Some(positive(last)?))
        } else {
            (prefix, positive(last)?, None)
        };
        Some((
            path,
            EditorPosition {
                line: Some(line),
                column,
            },
        ))
    }

    fn editor_args(
        editor: &Editor,
        root: &Path,
        target: &Path,
        is_folder: bool,
        position: Option<EditorPosition>,
    ) -> Vec<OsString> {
        let mut args: Vec<OsString> = editor.base_args.iter().map(OsString::from).collect();
        let arg = target.as_os_str().to_owned();
        if matches!(editor.launch, Launch::Folder) {
            args.push(if is_folder {
                arg
            } else {
                target.parent().unwrap_or(root).as_os_str().to_owned()
            });
        } else if is_folder {
            if matches!(editor.launch, Launch::Goto) {
                args.push("--new-window".into());
            }
            args.push(arg);
        } else if let Some(line) = position
            .as_ref()
            .and_then(|position| position.line)
            .filter(|line| *line > 0)
        {
            let column = position
                .and_then(|position| position.column)
                .filter(|column| *column > 0);
            match editor.launch {
                Launch::Goto
                    if target
                        .to_string_lossy()
                        .split(':')
                        .skip(1)
                        .any(|part| part.parse::<u32>().is_ok()) =>
                {
                    // VS Code cannot distinguish a numeric colon segment in a legal filename from a position.
                    args.push(arg);
                }
                Launch::Goto | Launch::Path => {
                    let mut located = arg;
                    located.push(format!(":{line}"));
                    if let Some(column) = column {
                        located.push(format!(":{column}"));
                    }
                    if matches!(editor.launch, Launch::Goto) {
                        args.push("--goto".into());
                    }
                    args.push(located);
                }
                Launch::LineColumn => {
                    args.push(root.as_os_str().to_owned());
                    args.extend(["--line".into(), line.to_string().into()]);
                    if let Some(column) = column {
                        args.extend(["--column".into(), column.to_string().into()]);
                    }
                    args.push(arg);
                }
                Launch::Folder => unreachable!("the file manager only opens folders"),
            }
        } else {
            args.push(arg);
        }
        args
    }

    pub(crate) fn open_editor(
        root: &Path,
        identity: ProjectRootIdentity,
        path: &str,
        editor_id: &str,
        position: Option<EditorPosition>,
        private: &PrivateDirectory,
    ) -> Result<(), String> {
        let editor = EDITORS
            .iter()
            .find(|editor| editor.id == editor_id)
            .ok_or("Unknown editor.")?;
        let command = executable(editor.commands).ok_or("The editor is not installed.")?;
        let directory = open_project_directory(root, identity)?;
        let relative = relative_path(root, path, private).map_err(|error| error.message)?;
        let (entry, relative, position) = match open_entry(&directory, &relative, root) {
            Ok(entry) => (entry, relative, position),
            Err(error) if error.kind() == io::ErrorKind::NotFound && position.is_none() => {
                let (path, position) =
                    diagnostic_target(path).ok_or_else(|| entry_error(error).message)?;
                let relative = relative_path(root, path, private).map_err(|error| error.message)?;
                let entry = open_entry(&directory, &relative, root)
                    .map_err(|error| entry_error(error).message)?;
                (entry, relative, Some(position))
            }
            Err(error) => return Err(entry_error(error).message),
        };
        let target = root.join(relative);
        let metadata = entry
            .metadata()
            .map_err(|error| entry_error(error).message)?;
        if !metadata.is_dir() && !(metadata.is_file() && metadata.nlink() == 1) {
            return Err("The project entry is not a regular file or folder.".into());
        }
        if metadata.is_dir() && private.overlaps(&target) {
            return Err("The folder contains HoH private data.".into());
        }
        matches_path(&entry, &target).map_err(|error| error.message)?;
        let mut child = Command::new(command)
            .args(editor_args(
                editor,
                root,
                &target,
                metadata.is_dir(),
                position,
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("The editor could not be launched: {error}"))?;
        // Reap the launcher without holding the Tauri request open for the life of the editor.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);

        struct Fixture(PathBuf);

        impl Fixture {
            fn new() -> Self {
                let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
                    "hoh-project-files-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::create_dir(&directory).unwrap();
                Self(directory)
            }
            fn identity(path: &Path) -> ProjectRootIdentity {
                let metadata = fs::metadata(path).unwrap();
                ProjectRootIdentity {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                }
            }
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn opens_nested_project_paths_without_exposing_other_roots() {
            let fixture = Fixture::new();
            let root = fixture.0.join("project");
            fs::create_dir_all(root.join("src")).unwrap();
            fs::create_dir(root.join(".codex")).unwrap();
            fs::write(root.join(".codex/token"), "hidden").unwrap();
            #[cfg(target_os = "macos")]
            assert!(
                read_text(
                    &root,
                    Fixture::identity(&root),
                    ".CODEX/token",
                    false,
                    &PrivateDirectory::default()
                )
                .is_err()
            );
            fs::write(root.join("src/main.txt"), "привет").unwrap();
            fs::write(fixture.0.join("secret"), "secret").unwrap();
            fs::hard_link(fixture.0.join("secret"), root.join("hardlink")).unwrap();
            symlink(&fixture.0, root.join("escape")).unwrap();
            let identity = Fixture::identity(&root);
            let entries = list(&root, identity, "src", &PrivateDirectory::default()).unwrap();
            assert_eq!(entries.entries[0].path, "src/main.txt");
            assert_eq!(
                read_text(
                    &root,
                    identity,
                    "src/main.txt",
                    false,
                    &PrivateDirectory::default()
                )
                .unwrap()
                .contents,
                "привет"
            );
            for path in ["../secret", "escape/secret", "hardlink", ".codex/token"] {
                assert!(
                    read_text(&root, identity, path, true, &PrivateDirectory::default()).is_err(),
                    "accepted {path}"
                );
            }
            assert!(list(&root, identity, "escape", &PrivateDirectory::default()).is_err());
            assert!(
                !list(&root, identity, "", &PrivateDirectory::default())
                    .unwrap()
                    .entries
                    .iter()
                    .any(|entry| entry.path == ".codex" || entry.path == "escape")
            );
        }

        #[test]
        fn reports_missing_and_directory_results_and_caps_utf8_preview() {
            let fixture = Fixture::new();
            let root = fixture.0.as_path();
            let identity = Fixture::identity(root);
            let mut content = vec![b'a'; MAX_TEXT_BYTES as usize - 1];
            content.extend_from_slice("я".as_bytes());
            fs::write(root.join("large"), &content).unwrap();
            fs::write(root.join("empty"), "").unwrap();
            fs::write(root.join("binary"), [0x00, 0xff]).unwrap();
            assert!(
                read_text(root, identity, "empty", false, &PrivateDirectory::default())
                    .unwrap()
                    .contents
                    .is_empty()
            );
            assert_eq!(
                read_text(
                    root,
                    identity,
                    "binary",
                    false,
                    &PrivateDirectory::default()
                )
                .unwrap_err()
                .failure,
                "binary_file"
            );
            assert_eq!(
                read_text(root, identity, ".", false, &PrivateDirectory::default())
                    .unwrap_err()
                    .failure,
                "path_not_file"
            );
            assert!(
                !read_text(
                    root,
                    identity,
                    "missing",
                    true,
                    &PrivateDirectory::default()
                )
                .unwrap()
                .exists
            );
            let text =
                read_text(root, identity, "large", false, &PrivateDirectory::default()).unwrap();
            assert!(text.truncated);
            assert_eq!(text.byte_length, content.len() as u64);
            assert_eq!(text.contents.len(), MAX_TEXT_BYTES as usize - 1);
        }

        #[test]
        fn excludes_the_configured_private_directory_inside_a_project() {
            let fixture = Fixture::new();
            let root = fixture.0.as_path();
            let runtime = root.join("runtime");
            let secrets = runtime.join("private");
            fs::create_dir_all(&secrets).unwrap();
            fs::write(secrets.join("secret"), "fixture private data").unwrap();
            fs::write(runtime.join("private-notes.txt"), "fixture notes").unwrap();

            // Resolve configured runtime directory aliases before comparing against the project.
            symlink(&runtime, root.join("runtime-alias")).unwrap();
            let private = PrivateDirectory::new(&root.join("runtime-alias/private"));
            let identity = Fixture::identity(root);
            let observed = list(root, identity, "runtime", &private).unwrap();
            assert_eq!(observed.entries.len(), 1);
            assert_eq!(observed.entries[0].path, "runtime/private-notes.txt");
            assert!(list(root, identity, "runtime/private", &private).is_err());
            assert!(read_text(root, identity, "runtime/private/secret", false, &private).is_err());
            assert_eq!(
                read_text(root, identity, "runtime/private-notes.txt", false, &private)
                    .unwrap()
                    .contents,
                "fixture notes"
            );
            assert!(private.overlaps(root));
            assert!(private.overlaps(&secrets));
            assert!(!private.overlaps(&root.join("project")));
        }

        #[test]
        fn binds_reads_to_the_authorized_root_identity() {
            let fixture = Fixture::new();
            let root = fixture.0.join("project");
            fs::create_dir(&root).unwrap();
            let identity = Fixture::identity(&root);
            fs::rename(&root, fixture.0.join("old-project")).unwrap();
            fs::create_dir(&root).unwrap();
            fs::write(root.join("newfile"), "private").unwrap();
            assert!(list(&root, identity, "", &PrivateDirectory::default()).is_err());
            assert!(
                read_text(
                    &root,
                    identity,
                    "newfile",
                    false,
                    &PrivateDirectory::default()
                )
                .is_err()
            );
        }

        #[test]
        fn constructs_editor_arguments_for_spaces_and_numeric_colons() {
            let vs_code = EDITORS.iter().find(|editor| editor.id == "vscode").unwrap();
            assert_eq!(vs_code.commands, &["code"]);
            let root = Path::new("/tmp/work area");
            let (_, position) = diagnostic_target("/tmp/work area/read me:41:7").unwrap();
            assert_eq!(
                editor_args(vs_code, root, &root.join("read me"), false, Some(position)),
                ["--goto", "/tmp/work area/read me:41:7"]
            );
            let (_, position) = diagnostic_target("/tmp/work area/read me:10-15").unwrap();
            assert_eq!(
                editor_args(
                    vs_code,
                    root,
                    &root.join("legal:2026"),
                    false,
                    Some(position)
                ),
                ["/tmp/work area/legal:2026"]
            );
            assert_eq!(
                editor_args(vs_code, root, &root.join("legal:2026"), true, None),
                ["--new-window", "/tmp/work area/legal:2026"]
            );
        }
    }
}
