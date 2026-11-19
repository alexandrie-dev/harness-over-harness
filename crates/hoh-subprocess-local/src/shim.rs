// SHIM(hoh-subprocess): its classifier recognizes ForeignError but cannot carry the
// runner's name/syscall/path; replace when the shared foreign-error record retains them.
pub(crate) fn restore_spawn_error(
    record: crate::runner_protocol::SerializedRunnerError,
) -> hoh_error::BoxError {
    if let Some(code) = record.code.as_ref() {
        Box::new(hoh_subprocess::SubprocessForeignError::Coded {
            message: record.message,
            code: serde_json::Value::String(code.clone()),
        })
    } else {
        crate::runner_protocol::deserialize_runner_error(record)
    }
}

// SHIM(hoh-kernel): no synchronous host-exit callback exists; replace when the app
// exposes its finalization hook. The application can invoke the public provider method.
pub(crate) fn terminate_for_host_exit(callback: impl FnOnce()) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
}

// SHIM(hoh-util-values): Node inspect(string) baseline for NUL diagnostics;
// replace when the common helper supplies long-string/multiline inspect formatting.
pub(crate) fn inspect_string(value: &str) -> String {
    let quote = if !value.contains('\'') {
        '\''
    } else if !value.contains('"') {
        '"'
    } else if !value.contains('`') && !value.contains("${") {
        '`'
    } else {
        '\''
    };
    let mut result = String::from(quote);
    for ch in value.chars() {
        match ch {
            '\0' => result.push_str("\\x00"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\\' => result.push_str("\\\\"),
            c if c == quote => {
                result.push('\\');
                result.push(c);
            }
            c => result.push(c),
        }
    }
    result.push(quote);
    result
}

// SHIM(node:os): local passwd fallback only; replace when an approved safe NSS
// adapter supplies userInfo().shell and its native failures.
pub(crate) fn user_info_shell() -> Option<String> {
    let uid = rustix::process::getuid().as_raw().to_string();
    crate::read_utf8("/etc/passwd")
        .ok()?
        .lines()
        .find_map(|line| {
            let fields = line.split(':').collect::<Vec<_>>();
            (fields.get(2) == Some(&uid.as_str()))
                .then(|| {
                    fields
                        .last()
                        .filter(|shell| !shell.is_empty())
                        .map(|shell| shell.to_string())
                })
                .flatten()
        })
}

// SHIM(hoh-util-values): Node os.tmpdir's POSIX environment precedence; replace
// when the common Node host helpers expose this (std::env::temp_dir reads only TMPDIR).
pub(crate) fn temporary_directory() -> std::path::PathBuf {
    temporary_directory_from(["TMPDIR", "TMP", "TEMP"].map(std::env::var_os))
}
fn temporary_directory_from(values: [Option<std::ffi::OsString>; 3]) -> std::path::PathBuf {
    let directory = values
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    // Every caller uses Node join(tmpdir(), prefix), before mkdtemp touches the FS.
    normalize_temporary_path(&directory)
}
pub(crate) fn normalize_temporary_path(path: &std::path::Path) -> std::path::PathBuf {
    hoh_util_workspace_path::node_path::normalize(&path.to_string_lossy()).into()
}

// SHIM(hoh-util-values): randomBytes' native entropy/error adapter. Successful
// bytes come from getrandom; replace when the shared Node host adapter also
// provides Node/OpenSSL failure names and messages. Never invent a fallback ID.
pub(crate) fn random_bytes<const SIZE: usize>() -> Result<[u8; SIZE], hoh_error::BoxError> {
    let mut bytes = [0; SIZE];
    getrandom::fill(&mut bytes).map_err(|error| crate::error(error.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_node_join_path_text_for_temporary_directories_without_mutating_the_environment() {
        assert_eq!(
            temporary_directory_from([None, None, None]),
            std::path::Path::new("/tmp")
        );
        assert_eq!(
            temporary_directory_from([
                Some("".into()),
                Some("/tmp//a/../".into()),
                Some("/ignored".into())
            ])
            .to_string_lossy(),
            "/tmp/"
        );
        assert_eq!(
            normalize_temporary_path(std::path::Path::new("./dsh-random")).to_string_lossy(),
            "dsh-random"
        );
        let root = tempfile::tempdir().unwrap();
        let directory = temporary_directory_from([
            Some(root.path().join("missing/..").into_os_string()),
            None,
            None,
        ]);
        // Node's lexical join removes missing/.. before the mkdir syscall.
        let child = tempfile::tempdir_in(&directory).unwrap();
        assert_eq!(directory, root.path());
        assert!(child.path().is_dir());
    }
}
