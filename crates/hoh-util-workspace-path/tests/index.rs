use hoh_util_workspace_path::{
    FileAddress, PathParts, abbreviate_home_path, file_address_for, is_absolute_workspace_path,
    parse_file_address, path_parts_of, relativize_to_cwd, resolve_workspace_path,
    workspace_title_of,
};

#[test]
fn addresses_every_path_by_session_and_keeps_absolute_paths_outside_workspace() {
    for (cwd, path, expected) in [
        (
            Some("/w"),
            "src/a.ts",
            "dsh-resource://file/session/s/src/a.ts",
        ),
        (None, "src/a.ts", "dsh-resource://file/session/s/src/a.ts"),
        (
            Some("/w/"),
            "/w/src/a.ts",
            "dsh-resource://file/session/s/src/a.ts",
        ),
        (Some("/w"), "/w", "dsh-resource://file/session/s/"),
        (
            Some("/w"),
            "/work/a.ts",
            "dsh-resource://file/session/s//work/a.ts",
        ),
        (
            None,
            "/etc/hosts",
            "dsh-resource://file/session/s//etc/hosts",
        ),
        (
            Some(r"C:\w"),
            r"C:\w\x.ts",
            "dsh-resource://file/session/s/x.ts",
        ),
        (
            Some(r"C:\w"),
            r"D:\x.ts",
            "dsh-resource://file/session/s/D:/x.ts",
        ),
        (
            None,
            r"\\server\share\x.ts",
            "dsh-resource://file/session/s///server/share/x.ts",
        ),
        (
            Some(r"\\server\share"),
            r"\\server\share\x.ts",
            "dsh-resource://file/session/s/x.ts",
        ),
    ] {
        assert_eq!(
            file_address_for("s", cwd, path),
            expected,
            "{cwd:?}, {path}"
        );
    }
}

#[test]
fn round_trips_parent_relative_address_with_and_without_workspace_root() {
    for cwd in [Some("/workspace"), None] {
        let address = file_address_for("s", cwd, "../outside/a.txt");
        assert_eq!(address, "dsh-resource://file/session/s/../outside/a.txt");
        assert_eq!(
            parse_file_address(&address),
            Some(FileAddress::Session {
                session_id: "s".into(),
                path: "../outside/a.txt".into(),
            })
        );
    }
}

#[test]
fn classifies_posix_windows_drive_and_unc_paths_as_absolute_and_everything_else_as_relative() {
    for path in ["/a/b", r"C:\x\a.ts", "C:/x/a.ts", r"\\server\share"] {
        assert!(is_absolute_workspace_path(path), "{path}");
    }
    for path in ["src/a.ts", ""] {
        assert!(!is_absolute_workspace_path(path), "{path}");
    }
}

#[test]
fn resolves_relative_paths_without_changing_absolute_paths() {
    for (cwd, path, expected) in [
        (Some("/w"), "src/a.ts", "/w/src/a.ts"),
        (Some("/w"), "././src/a.ts", "/w/src/a.ts"),
        (Some(r"C:\w"), r".\src\a.ts", r"C:\w\src\a.ts"),
        (Some("/w"), "../a.ts", "/w/../a.ts"),
        (Some("/w/"), "/abs/a.ts", "/abs/a.ts"),
        (None, "src/a.ts", "src/a.ts"),
        (Some(""), "src/a.ts", "src/a.ts"),
        (Some("/w"), r"C:\x\a.ts", r"C:\x\a.ts"),
        (Some("/w"), r"\\server\share", r"\\server\share"),
    ] {
        assert_eq!(
            resolve_workspace_path(cwd, path),
            expected,
            "{cwd:?}, {path}"
        );
    }
}

#[test]
fn keeps_windows_drive_root_and_directory_joins_fully_qualified() {
    for (cwd, path, expected) in [
        (r"C:\", r"src\a.ts", r"C:\src\a.ts"),
        (r"C:\work\", r"src\a.ts", r"C:\work\src\a.ts"),
        ("C:/work/", "src/a.ts", "C:/work/src/a.ts"),
    ] {
        assert_eq!(resolve_workspace_path(Some(cwd), path), expected);
    }
}

#[test]
fn abbreviates_only_descendants_of_posix_home() {
    for (path, home, expected) in [
        ("/Users/u", Some("/Users/u"), "~"),
        ("/Users/u/", Some("/Users/u"), "~"),
        (
            "/Users/u/Documents/project",
            Some("/Users/u"),
            "~/Documents/project",
        ),
        ("/Users/u2/a.ts", Some("/Users/u"), "/Users/u2/a.ts"),
        ("/Users/u/a.ts", None, "/Users/u/a.ts"),
        ("/Users/u/a.ts", Some(""), "/Users/u/a.ts"),
        ("/etc/hosts", Some("/"), "/etc/hosts"),
        (
            r"C:\Users\u\project",
            Some(r"C:\Users\u"),
            r"C:\Users\u\project",
        ),
        (
            r"\\server\share\u",
            Some(r"\\server\share\u"),
            r"\\server\share\u",
        ),
    ] {
        assert_eq!(
            abbreviate_home_path(path, home),
            expected,
            "{path}, {home:?}"
        );
    }
}

#[test]
fn reads_final_path_segment_on_both_path_styles() {
    assert_eq!(workspace_title_of("/work/project/"), "project");
    assert_eq!(workspace_title_of(r"C:\work\project\"), "project");
    assert_eq!(workspace_title_of("/"), "");
}

#[test]
fn splits_path_after_last_separator_keeping_separator_with_directories() {
    for (path, directory, name) in [
        ("/work/project/notes.md", "/work/project/", "notes.md"),
        (r"C:\work\project\notes.md", r"C:\work\project\", "notes.md"),
        (r"\\host\share/notes.md", r"\\host\share/", "notes.md"),
        ("/work/project/", "/work/", "project"),
    ] {
        assert_eq!(
            path_parts_of(path),
            PathParts {
                directory: directory.into(),
                name: name.into()
            }
        );
    }
}

#[test]
fn makes_separator_less_or_separator_only_path_all_name() {
    for path in ["notes.md", "/"] {
        assert_eq!(
            path_parts_of(path),
            PathParts {
                directory: "".into(),
                name: path.into()
            }
        );
    }
}

#[test]
fn displays_path_relative_to_cwd_only_within_that_workspace() {
    for (path, cwd, label) in [
        ("/work/report.txt", Some("/work"), "report.txt"),
        (
            "/work/reports/result.pdf",
            Some("/work/"),
            "reports/result.pdf",
        ),
        (
            "/work-other/report.txt",
            Some("/work"),
            "/work-other/report.txt",
        ),
        ("/other/report.txt", Some("/work"), "/other/report.txt"),
        ("report.txt", Some("/work"), "report.txt"),
        ("/work/report.txt", None, "/work/report.txt"),
        ("/work/report.txt", Some(""), "/work/report.txt"),
        ("/report.txt", Some("/"), "report.txt"),
        (
            r"C:\work\reports\result.pdf",
            Some(r"C:\work"),
            r"reports\result.pdf",
        ),
        (
            r"\\server\share\report.txt",
            Some(r"\\server\share"),
            "report.txt",
        ),
    ] {
        assert_eq!(relativize_to_cwd(path, cwd), label, "{path}, {cwd:?}");
    }
}
