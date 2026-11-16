use hoh_util_workspace_path::file_address::{
    FileAddress, absolute_file_address, parse_file_address, session_file_address,
};

fn session(session_id: &str, path: &str) -> Option<FileAddress> {
    Some(FileAddress::Session {
        session_id: session_id.into(),
        path: path.into(),
    })
}

fn absolute(path: &str) -> Option<FileAddress> {
    Some(FileAddress::Absolute { path: path.into() })
}

#[test]
fn round_trips_session_relative_path_with_encoded_segments() {
    let address = session_file_address("s 1", "w/a b#c?.txt");
    assert_eq!(
        address,
        "dsh-resource://file/session/s%201/w/a%20b%23c%3F.txt"
    );
    assert_eq!(parse_file_address(&address), session("s 1", "w/a b#c?.txt"));
}

#[test]
fn drops_leading_dot_slash_and_names_root_with_empty_path() {
    assert_eq!(
        session_file_address("s", "./src/a.ts"),
        "dsh-resource://file/session/s/src/a.ts"
    );
    assert_eq!(
        session_file_address("s", "././../outside/a.txt"),
        "dsh-resource://file/session/s/../outside/a.txt"
    );
    assert_eq!(
        session_file_address("s", r"src\a.ts"),
        "dsh-resource://file/session/s/src/a.ts"
    );
    assert_eq!(
        session_file_address("s", ""),
        "dsh-resource://file/session/s/"
    );
    assert_eq!(
        parse_file_address("dsh-resource://file/session/s/"),
        session("s", "")
    );
}

#[test]
fn round_trips_absolute_path_in_session_address() {
    for (input, path, expected) in [
        (
            "/etc/hosts",
            "/etc/hosts",
            "dsh-resource://file/session/s//etc/hosts",
        ),
        ("/", "/", "dsh-resource://file/session/s//"),
        (
            r"C:\w\x.ts",
            "C:/w/x.ts",
            "dsh-resource://file/session/s/C:/w/x.ts",
        ),
        (
            r"\\server\share\x.ts",
            "//server/share/x.ts",
            "dsh-resource://file/session/s///server/share/x.ts",
        ),
        (
            "/a b#c?%.txt",
            "/a b#c?%.txt",
            "dsh-resource://file/session/s//a%20b%23c%3F%25.txt",
        ),
    ] {
        let address = session_file_address("s", input);
        assert_eq!(address, expected);
        assert_eq!(parse_file_address(&address), session("s", path));
    }
}

#[test]
fn gives_same_absolute_path_different_addresses_in_different_sessions() {
    assert_eq!(
        session_file_address("s1", "/etc/hosts"),
        "dsh-resource://file/session/s1//etc/hosts"
    );
    assert_eq!(
        session_file_address("s2", "/etc/hosts"),
        "dsh-resource://file/session/s2//etc/hosts"
    );
}

#[test]
fn keeps_session_id_and_unresolved_path() {
    for path in [
        "../outside/a.txt",
        "dir/../outside/a.txt",
        "dir/./a.txt",
        "/workspace/../outside/a.txt",
        "C:/workspace/../outside/a.txt",
        "//server/share/workspace/../outside/a.txt",
    ] {
        assert_eq!(
            parse_file_address(&session_file_address("s 1", path)),
            session("s 1", path)
        );
    }
}

#[test]
fn keeps_unresolved_absolute_path_without_session() {
    for path in [
        "/workspace/../outside/a.txt",
        "C:/workspace/../outside/a.txt",
        "//server/share/workspace/../outside/a.txt",
    ] {
        assert_eq!(
            parse_file_address(&absolute_file_address(path)),
            absolute(path)
        );
    }
}

#[test]
fn decodes_once_without_resolving_dot_segments() {
    for (encoded, path) in [
        ("./a.txt", "./a.txt"),
        ("%2e%2E/outside/a.txt", "../outside/a.txt"),
        ("dir/%2E/a.txt", "dir/./a.txt"),
        ("%252e%252e/a.txt", "%2e%2e/a.txt"),
        ("a%2Fb%5Cc.txt", "a/b\\c.txt"),
    ] {
        assert_eq!(
            parse_file_address(&format!("dsh-resource://file/session/s%2F1/{encoded}")),
            session("s/1", path)
        );
    }
}

#[test]
fn ignores_literal_suffix_without_consuming_encoded_filename_characters() {
    for suffix in [
        "?download=1#section",
        "#section?download=1",
        "?ignored=%E0%A4%A",
        "#ignored=%ZZ",
    ] {
        assert_eq!(
            parse_file_address(&format!(
                "dsh-resource://file/session/s/dir/../a%3Fb%23c%25.txt{suffix}"
            )),
            session("s", "dir/../a?b#c%.txt")
        );
        assert_eq!(
            parse_file_address(&format!(
                "dsh-resource://file/absolute/workspace/../a%3Fb%23c%25.txt{suffix}"
            )),
            absolute("/workspace/../a?b#c%.txt")
        );
    }
}

#[test]
fn round_trips_absolute_posix_path_with_leading_slash_dropped() {
    let address = absolute_file_address("/home/me/notes.md");
    assert_eq!(address, "dsh-resource://file/absolute/home/me/notes.md");
    assert_eq!(parse_file_address(&address), absolute("/home/me/notes.md"));
}

#[test]
fn round_trips_absolute_windows_drive_path_with_backslashes_normalized_and_colon_literal() {
    let address = absolute_file_address(r"C:\w\x.ts");
    assert_eq!(address, "dsh-resource://file/absolute/C:/w/x.ts");
    assert_eq!(parse_file_address(&address), absolute("C:/w/x.ts"));
}

#[test]
fn keeps_unc_path_identity_in_absolute_address() {
    let address = absolute_file_address(r"\\server\share\x.ts");
    assert_eq!(address, "dsh-resource://file/absolute//server/share/x.ts");
    assert_eq!(
        parse_file_address(&address),
        absolute("//server/share/x.ts")
    );
}

#[test]
fn decodes_either_spelling_of_path_segment() {
    assert!(
        matches!(parse_file_address("dsh-resource://file/session/s/w/a+b.txt"), Some(FileAddress::Session { path, .. }) if path == "w/a+b.txt")
    );
    assert!(
        matches!(parse_file_address("dsh-resource://file/session/s/w/a%2Bb.txt"), Some(FileAddress::Session { path, .. }) if path == "w/a+b.txt")
    );
    assert!(
        matches!(parse_file_address("dsh-resource://file/absolute/w/a%2Bb.txt"), Some(FileAddress::Absolute { path }) if path == "/w/a+b.txt")
    );
}

#[test]
fn rejects_invalid_file_addresses() {
    for (case, address) in [
        (
            "another resource type",
            "dsh-resource://terminal/session/s/1",
        ),
        (
            "a type spelled with another case",
            "dsh-resource://File/session/s/w/x.ts",
        ),
        (
            "a scheme spelled with another case",
            "DSH-RESOURCE://file/session/s/w/x.ts",
        ),
        ("an unknown scope", "dsh-resource://file/shared/s/w/x"),
        ("the retired file:// grammar", "file://sessions/s/w/x.ts"),
        ("a host-less file URL", "file:///w/x.ts"),
        (
            "a session address with no path",
            "dsh-resource://file/session/s",
        ),
        (
            "a session address with no id",
            "dsh-resource://file/session",
        ),
        (
            "a session address with an empty id",
            "dsh-resource://file/session//a.txt",
        ),
        (
            "an absolute address with no path",
            "dsh-resource://file/absolute",
        ),
        (
            "an absolute address with an empty path",
            "dsh-resource://file/absolute/",
        ),
        (
            "a UNC marker with no host behind it",
            "dsh-resource://file/absolute//",
        ),
        ("no scope", "dsh-resource://file"),
        ("another scheme", "sidebar://files"),
        ("not a URL", "notes.txt"),
        (
            "a malformed escape",
            "dsh-resource://file/session/s/%E0%A4%A",
        ),
        (
            "a malformed id escape",
            "dsh-resource://file/session/%/a.txt",
        ),
        (
            "a malformed absolute escape",
            "dsh-resource://file/absolute/a%ZZ.txt",
        ),
    ] {
        assert_eq!(parse_file_address(address), None, "{case}");
    }
}
