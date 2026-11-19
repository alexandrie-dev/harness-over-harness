use crate::{LocalError, control_spawn::control_environment, error, shim::inspect_string};
use hoh_error::BoxError;
use hoh_subprocess::{SubprocessEnvironment, SubprocessSpawnSpec};
use indexmap::IndexMap;
use std::path::PathBuf;

pub const SUBPROCESS_RUNNER_ENV: &str = "DSH_SUBPROCESS_RUNNER";
pub(crate) const BOOTSTRAP_SESSION_ENV: &str = "__HOH_SUBPROCESS_BOOTSTRAP_SESSION";
pub(crate) const DETACHED_EXEC_MODE: &str = "--hoh-detached-exec";
pub type RunnerInvocation = Vec<String>;
pub fn spawn_runner_invocation() -> RunnerInvocation {
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("hoh"));
    let dir = executable
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let dir = if dir.file_name().is_some_and(|name| name == "deps") {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    };
    vec![
        dir.join("hoh-subprocess-runner")
            .to_string_lossy()
            .into_owned(),
    ]
}
pub fn runner_invocation_available(invocation: &[String]) -> bool {
    let Some(file) = invocation.first() else {
        return false;
    };
    let accessible =
        |path: &str, mode| !path.starts_with('/') || rustix::fs::access(path, mode).is_ok();
    accessible(file, rustix::fs::Access::EXEC_OK)
        && invocation
            .last()
            .is_none_or(|entry| entry == file || accessible(entry, rustix::fs::Access::READ_OK))
}
pub fn runner_environment(
    selection: &str,
    _invocation: Option<&[String]>,
) -> IndexMap<String, String> {
    runner_environment_from(selection, hoh_subprocess::scrubbed_parent_env(None))
}
fn runner_environment_from(
    selection: &str,
    mut env: IndexMap<String, String>,
) -> IndexMap<String, String> {
    env.retain(|name, _| {
        let name = name.to_uppercase();
        !name.starts_with("NODE_") && !name.starts_with("TSX_")
    });
    env.insert(SUBPROCESS_RUNNER_ENV.into(), selection.into());
    env.insert("SYSTEMD_LOG_TARGET".into(), "null".into());
    env.insert(BOOTSTRAP_SESSION_ENV.into(), "new".into());
    env
}
pub fn consume_runner_selection(env: &mut IndexMap<String, String>) -> Option<String> {
    env.shift_remove(SUBPROCESS_RUNNER_ENV)
}
pub fn parse_runner_target_argv(argv: &[String]) -> Result<Vec<String>, BoxError> {
    if argv.first().is_none_or(|s| s != "--") || argv.len() < 2 {
        Err(error(
            "subprocess runner requires target argv after a private -- delimiter",
        ))
    } else {
        Ok(argv[1..].to_vec())
    }
}
pub fn child_env(extra: Option<&SubprocessEnvironment>) -> IndexMap<String, String> {
    child_env_from(hoh_subprocess::scrubbed_parent_env(None), extra)
}
fn child_env_from(
    mut env: IndexMap<String, String>,
    extra: Option<&SubprocessEnvironment>,
) -> IndexMap<String, String> {
    for (key, value) in extra.into_iter().flatten() {
        if let Some(value) = value {
            env.insert(key.clone(), value.clone());
        } else {
            env.shift_remove(key);
        }
    }
    env
}

pub fn target_environment(
    spec: &SubprocessSpawnSpec,
) -> Result<IndexMap<String, String>, BoxError> {
    for (index, value) in spec.argv.iter().enumerate() {
        validate_no_null_byte(
            &if index == 0 {
                "file".into()
            } else {
                format!("args[{}]", index - 1)
            },
            value,
            true,
        )?;
    }
    validate_no_null_byte("options.cwd", &spec.cwd, false)?;
    let env = child_env(spec.env.as_ref());
    for (key, value) in &env {
        let property = format!("options.env['{key}']");
        validate_no_null_byte(&property, key, false)?;
        validate_no_null_byte(&property, value, false)?;
    }
    let env = control_environment(
        env.into_iter()
            .map(|(key, value)| (key, Some(value)))
            .collect(),
        spec.stdio.control.is_some(),
    )?;
    Ok(env
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect())
}
pub fn validate_no_null_byte(property: &str, value: &str, argument: bool) -> Result<(), BoxError> {
    if value.contains('\0') {
        Err(Box::new(LocalError::TypeError(format!(
            "The {} '{property}' must be a string without null bytes. Received {}",
            if argument { "argument" } else { "property" },
            inspect_string(value)
        ))))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_ambient_node_and_tsx_controls_from_the_bootstrap_environment_only() {
        let environment: IndexMap<String, String> = [
            ("NODE_OPTIONS", "--require /tmp/bootstrap.cjs"),
            ("NODE_DEBUG", "esm"),
            ("TSX_DISABLE_CACHE", "1"),
            ("TSX_TSCONFIG_PATH", "/ambient/tsconfig.json"),
            ("SAFE", "kept"),
        ]
        .map(|(k, v)| (k.into(), v.into()))
        .into();
        let target = child_env_from(environment.clone(), None);
        let bootstrap = runner_environment_from("/tmp/request", environment);
        for key in [
            "NODE_OPTIONS",
            "NODE_DEBUG",
            "TSX_DISABLE_CACHE",
            "TSX_TSCONFIG_PATH",
        ] {
            assert!(!bootstrap.contains_key(key));
            assert!(target.contains_key(key));
        }
        assert_eq!(bootstrap["SAFE"], "kept");
        assert_eq!(bootstrap["SYSTEMD_LOG_TARGET"], "null");
    }
    #[test]
    fn scrubs_credential_shaped_and_ambient_dsh_env_vars_and_forwards_explicit_overrides() {
        let parent = [
            ("DSH_TEST_API_KEY", "secret"),
            ("DSH_TEST_TOKEN", "secret"),
            ("SUBPROCESS_TEST_PASSWORD", "secret"),
            ("DSH_TEST_PLAIN", "plain"),
            ("KEEP", "kept"),
            ("TOMBSTONE", "old"),
            ("Mixed", "one"),
            ("MIXED", "two"),
        ];
        let scrubbed = hoh_subprocess::scrubbed_parent_env_from(
            parent.map(|(k, v)| (k.into(), Some(v.into()))),
            None,
        );
        let env = child_env_from(
            scrubbed,
            Some(
                &[
                    ("DSH_SHELL".into(), Some("bash".into())),
                    ("DSH_SESSION_ID".into(), Some("session".into())),
                    ("SUBPROCESS_TEST_PASSWORD".into(), Some("explicit".into())),
                    ("TOMBSTONE".into(), None),
                ]
                .into(),
            ),
        );
        assert!(!env.contains_key("DSH_TEST_API_KEY"));
        assert!(!env.contains_key("DSH_TEST_TOKEN"));
        assert!(!env.contains_key("DSH_TEST_PLAIN"));
        assert!(!env.contains_key("TOMBSTONE"));
        assert_eq!(env["KEEP"], "kept");
        assert_eq!(env["DSH_SHELL"], "bash");
        assert_eq!(env["DSH_SESSION_ID"], "session");
        assert_eq!(env["SUBPROCESS_TEST_PASSWORD"], "explicit");
        assert_eq!(env["Mixed"], "one");
        assert_eq!(env["MIXED"], "two");
    }
}
