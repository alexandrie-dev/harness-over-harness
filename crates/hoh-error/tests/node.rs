#![cfg(target_os = "linux")]

use hoh_error::{
    node_error_code, node_error_description, node_fs_error_message, node_spawn_error_message,
};
use serde_json::Value;
use std::{fs, io, os::unix::fs::PermissionsExt, process::Command};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/node_system_errors.json")).unwrap()
}

#[test]
fn every_system_error_name_and_description() {
    let fixture = fixture();
    let rows = fixture["table"].as_array().unwrap();
    assert_eq!(rows.len(), 85);
    for row in rows {
        let code = row["code"].as_str().unwrap();
        assert_eq!(
            node_error_description(code),
            row["description"].as_str(),
            "{row}"
        );
        // These are libuv/resolver values, not Linux errno values.
        let virtual_code =
            code.starts_with("EAI_") || matches!(code, "ECHARSET" | "UNKNOWN" | "EOF" | "EFTYPE");
        let errno = -i32::try_from(row["errno"].as_i64().unwrap()).unwrap();
        let error = io::Error::from_raw_os_error(errno);
        assert_eq!(
            node_error_code(&error),
            (!virtual_code).then_some(code),
            "{row}"
        );
        if !virtual_code {
            let expected = if matches!(code, "EACCES" | "EAGAIN" | "EMFILE" | "ENFILE" | "ENOENT") {
                format!("spawn command {code}")
            } else {
                format!("spawn {code}")
            };
            assert_eq!(node_spawn_error_message(&error, "command"), expected);
        }
    }
    assert_eq!(node_error_description("not a code"), None);
}

#[test]
fn real_filesystem_failures_match_node() {
    let fixture = fixture();
    let root = tempfile::tempdir().unwrap();
    let root_text = root.path().to_str().unwrap();
    fs::write(root.path().join("file"), "contents").unwrap();
    fs::create_dir(root.path().join("dir")).unwrap();
    fs::create_dir(root.path().join("full")).unwrap();
    fs::write(root.path().join("full/child"), "contents").unwrap();

    let mut checked = 0;
    for (name, row) in fixture["fsMessages"].as_object().unwrap() {
        // Node's ERR_FS_EISDIR is a separate API error, not a system error.
        if name == "rm directory without recursive" {
            assert_eq!(row["code"], "ERR_FS_EISDIR");
            continue;
        }
        let expected = row["message"].as_str().unwrap();
        let paths: Vec<_> = expected
            .split('\'')
            .skip(1)
            .step_by(2)
            .map(|path| path.replace("<root>", root_text))
            .collect();
        let path = paths.first().map(String::as_str);
        let dest = paths.get(1).map(String::as_str);
        let missing = root.path().join("missing");
        let file = root.path().join("file");
        let dir = root.path().join("dir");
        let error = match name.as_str() {
            "readFile missing" => fs::read(&missing).unwrap_err(),
            "readFile directory" => fs::read(&dir).unwrap_err(),
            "readFile under a file" => fs::read(file.join("x")).unwrap_err(),
            "writeFile into missing directory" => fs::write(missing.join("x"), "").unwrap_err(),
            "writeFile onto directory" => fs::write(&dir, "").unwrap_err(),
            "writeFile exclusive onto file" => fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .unwrap_err(),
            "open missing" | "truncate missing" => fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&missing)
                .unwrap_err(),
            "mkdir existing" => fs::create_dir(&dir).unwrap_err(),
            "mkdir under missing" => fs::create_dir(missing.join("x")).unwrap_err(),
            "mkdir recursive under a file" => fs::create_dir_all(file.join("x/y")).unwrap_err(),
            "readdir missing" | "opendir missing" => fs::read_dir(&missing).unwrap_err(),
            "readdir file" => fs::read_dir(&file).unwrap_err(),
            "stat missing" => fs::metadata(&missing).unwrap_err(),
            "lstat missing" | "rm missing" => fs::symlink_metadata(&missing).unwrap_err(),
            "access missing" => io::Error::from(
                rustix::fs::access(&missing, rustix::fs::Access::EXISTS).unwrap_err(),
            ),
            "realpath missing" => fs::canonicalize(&missing).unwrap_err(),
            "readlink file" => fs::read_link(&file).unwrap_err(),
            "rename missing" => fs::rename(&missing, root.path().join("other")).unwrap_err(),
            "unlink missing" => fs::remove_file(&missing).unwrap_err(),
            "unlink directory" => fs::remove_file(&dir).unwrap_err(),
            "rmdir not empty" => fs::remove_dir(root.path().join("full")).unwrap_err(),
            "rmdir missing" => fs::remove_dir(&missing).unwrap_err(),
            "copyFile missing" => fs::copy(&missing, root.path().join("copy")).unwrap_err(),
            "link missing" => fs::hard_link(&missing, root.path().join("hard")).unwrap_err(),
            "symlink onto file" => std::os::unix::fs::symlink("x", &file).unwrap_err(),
            "chmod missing" => {
                fs::set_permissions(&missing, fs::Permissions::from_mode(0o600)).unwrap_err()
            }
            // mkdtemp fails at mkdir when the parent is missing. tempfile wraps
            // that error and discards raw_os_error, so use the underlying call.
            "mkdtemp under missing" => fs::create_dir(missing.join("tmp-XXXXXX")).unwrap_err(),
            other => panic!("unhandled fixture case: {other}"),
        };
        assert_eq!(node_error_code(&error), row["code"].as_str(), "{name}");
        let actual = node_fs_error_message(&error, row["syscall"].as_str().unwrap(), path, dest);
        assert_eq!(actual.replace(root_text, "<root>"), expected, "{name}");
        checked += 1;
    }
    assert_eq!(checked, 30);
}

#[test]
fn real_spawn_failures_match_node() {
    let fixture = fixture();
    let root = tempfile::tempdir().unwrap();
    let root_text = root.path().to_str().unwrap();
    fs::write(root.path().join("file"), "not executable").unwrap();
    fs::set_permissions(root.path().join("file"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::create_dir(root.path().join("dir")).unwrap();
    for (name, suffix) in [
        ("missing", "missing"),
        ("not executable", "file"),
        ("directory", "dir"),
        ("under a file", "file/x"),
    ] {
        let command = root.path().join(suffix);
        let error = Command::new(&command).spawn().unwrap_err();
        let row = &fixture["spawnMessages"][name];
        assert_eq!(node_error_code(&error), row["code"].as_str(), "{name}");
        assert_eq!(
            node_spawn_error_message(&error, command.to_str().unwrap())
                .replace(root_text, "<root>"),
            row["message"].as_str().unwrap(),
            "{name}"
        );
    }
    let command = "hoh-no-such-command";
    let error = Command::new(command)
        .env("PATH", root.path())
        .spawn()
        .unwrap_err();
    assert_eq!(
        node_spawn_error_message(&error, command),
        fixture["spawnMessages"]["missing in PATH"]["message"]
    );
}

#[test]
fn unnamed_errors_use_their_own_display() {
    for error in [
        io::Error::other("custom failure"),
        io::Error::from_raw_os_error(123456),
        io::Error::from_raw_os_error(0),
        io::Error::from_raw_os_error(-1),
        io::Error::from_raw_os_error(4096),
        io::Error::from_raw_os_error(i32::MIN),
        io::Error::from_raw_os_error(i32::MAX),
    ] {
        assert_eq!(node_error_code(&error), None);
        assert_eq!(
            node_fs_error_message(&error, "open", Some("a'b"), Some("dest")),
            format!("{error}, open 'a'b' -> 'dest'")
        );
        assert_eq!(
            node_fs_error_message(&error, "read", None, None),
            format!("{error}, read")
        );
        assert_eq!(
            node_spawn_error_message(&error, "command"),
            format!("spawn {error}")
        );
    }
}
