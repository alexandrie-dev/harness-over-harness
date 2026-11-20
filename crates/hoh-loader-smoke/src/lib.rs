//! Shared subprocess and direct-agent harness for keyless Loader smoke tests.

mod agent_turn;

pub use agent_turn::{
    FixtureEventObserver, FixtureTurnOptions, FixtureTurnResult, run_fixture_turn,
};

use hoh_error::BoxError;
use indexmap::IndexMap;
use std::{env, ffi::OsStr, path::PathBuf, pin::Pin, process::Stdio, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;

const DEFAULT_PROCESS_TIMEOUT_MS: u64 = 30_000;
const EXECA_DEFAULT_MAX_BUFFER: u64 = 100_000_000;

/// Vitest-equivalent deadline that leaves room around the process-owned deadline.
pub const LOADER_SMOKE_TEST_TIMEOUT_MS: u64 = DEFAULT_PROCESS_TIMEOUT_MS + 15_000;

/// Environment variable selecting the example artifact.
pub const EXAMPLE_MODE_ENV: &str = "DSH_EXAMPLE_MODE";

/// Which artifact an example executable is selected from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExampleMode {
    #[default]
    Src,
    Lib,
}

impl ExampleMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Src => "src",
            Self::Lib => "lib",
        }
    }
}

/// Parse an [`ExampleMode`], reading [`EXAMPLE_MODE_ENV`] when no value is supplied.
pub fn resolve_example_mode(raw: Option<&str>) -> Result<ExampleMode, BoxError> {
    let owned;
    let raw = match raw {
        Some(raw) => Some(raw),
        None => {
            owned = env::var(EXAMPLE_MODE_ENV).ok();
            owned.as_deref()
        }
    };
    match raw {
        None | Some("") | Some("src") => Ok(ExampleMode::Src),
        Some("lib") => Ok(ExampleMode::Lib),
        Some(raw) => Err(format!(
            "{EXAMPLE_MODE_ENV} must be 'src' or 'lib', got {}.",
            serde_json::to_string(raw).expect("a Rust string is JSON")
        )
        .into()),
    }
}

/// Source hook selection retained from the TypeScript launch contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceImport {
    TsxEsm,
}

/// Environment overlay. `None` removes a variable from the child.
pub type ChildEnvironment = IndexMap<String, Option<String>>;

/// Inputs to [`resolve_example_launch`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExampleLaunchOptions {
    pub src_bin: String,
    pub lib_bin: Option<String>,
    pub config_args: Vec<String>,
    pub mode: Option<ExampleMode>,
    pub tsconfig_path: Option<String>,
    pub source_import: Option<SourceImport>,
    pub env: ChildEnvironment,
}

/// Resolved native spawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExampleLaunch {
    pub command: String,
    pub args: Vec<String>,
    pub env: ChildEnvironment,
}

fn to_lib_bin(src_bin: &str) -> Result<String, BoxError> {
    let slash = src_bin.rfind("/src/");
    let backslash = src_bin.rfind("\\src\\");
    let cut = match (slash, backslash) {
        (Some(a), Some(b)) => a.max(b),
        (Some(cut), None) | (None, Some(cut)) => cut,
        (None, None) => {
            return Err(format!(
                "resolveExampleLaunch: expected a \"/src/\" segment or Windows equivalent in bin path {}.",
                serde_json::to_string(src_bin).expect("a Rust string is JSON")
            )
            .into());
        }
    };
    let separator = &src_bin[cut..cut + 1];
    let mut tail = src_bin[cut + "/src/".len()..].to_owned();
    if tail.ends_with(".ts") {
        tail.truncate(tail.len() - 3);
        tail.push_str(".js");
    }
    Ok(format!(
        "{}{separator}lib{separator}{tail}",
        &src_bin[..cut]
    ))
}

/// Resolve the selected artifact and child environment.
///
/// Rust launches the selected executable directly. `tsconfig_path` and `source_import` retain the
/// source contract for callers and validation; they do not install a JavaScript runtime hook.
pub fn resolve_example_launch(options: ExampleLaunchOptions) -> Result<ExampleLaunch, BoxError> {
    let mode = match options.mode {
        Some(mode) => mode,
        None => resolve_example_mode(None)?,
    };
    let mut child_env = hoh_http_proxy::cleared_proxy_env();
    child_env.extend(options.env);
    let command = match mode {
        ExampleMode::Src => {
            let tsconfig = options.tsconfig_path.ok_or_else(|| -> BoxError {
                "resolveExampleLaunch: 'src' mode needs tsconfigPath for the workspace paths map."
                    .into()
            })?;
            child_env.insert("TSX_TSCONFIG_PATH".into(), Some(tsconfig));
            options.src_bin
        }
        ExampleMode::Lib => match options.lib_bin {
            Some(lib_bin) => lib_bin,
            None => to_lib_bin(&options.src_bin)?,
        },
    };
    Ok(ExampleLaunch {
        command,
        args: options.config_args,
        env: child_env,
    })
}

/// Boxed asynchronous world-state hook used by [`LoaderSmokeOptions`].
pub type LoaderSmokeHook = Arc<
    dyn Fn(PathBuf) -> Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>> + Send + Sync,
>;

/// Inputs every native Loader smoke supplies.
#[derive(Clone)]
pub struct LoaderSmokeOptions {
    pub label: String,
    pub bin_script: String,
    pub lib_bin_script: Option<String>,
    pub config_path: String,
    pub bin_args: Option<Vec<String>>,
    pub tsconfig_path: String,
    pub mode: Option<ExampleMode>,
    pub source_import: Option<SourceImport>,
    pub env: ChildEnvironment,
    pub process_timeout_ms: Option<u64>,
    pub prepare: Option<LoaderSmokeHook>,
    pub inspect: Option<LoaderSmokeHook>,
    pub expected_exit_code: Option<i32>,
    pub temp_dir_prefix: Option<String>,
    pub temp_dir_parent: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
}

/// Captured output from a Loader smoke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoaderSmokeResult {
    pub stdout: String,
    pub stderr: String,
}

fn apply_environment(command: &mut tokio::process::Command, values: &ChildEnvironment) {
    for (name, value) in values {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
}

async fn read_pipe<R>(pipe: R) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    pipe.take(EXECA_DEFAULT_MAX_BUFFER + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() as u64 > EXECA_DEFAULT_MAX_BUFFER {
        return Err(std::io::Error::other(
            "loader smoke output exceeded execa's 100 MB maxBuffer",
        ));
    }
    Ok(bytes)
}

fn display_seconds(milliseconds: u64) -> String {
    if milliseconds.is_multiple_of(1_000) {
        (milliseconds / 1_000).to_string()
    } else {
        let mut value = format!("{:.3}", milliseconds as f64 / 1_000.0);
        while value.ends_with('0') {
            value.pop();
        }
        value
    }
}

fn io_error(stage: &str, target: &OsStr, error: std::io::Error) -> BoxError {
    format!("{stage} {}: {error}", target.to_string_lossy()).into()
}

/// Boot one Loader executable from an isolated cwd, close stdin, and await its declared exit.
pub async fn run_loader_smoke(options: LoaderSmokeOptions) -> Result<LoaderSmokeResult, BoxError> {
    let provided_cwd = options.cwd.is_some();
    if provided_cwd && options.temp_dir_prefix.is_some() {
        return Err("runLoaderSmoke: cwd and tempDirPrefix cannot be combined".into());
    }
    if !provided_cwd && options.temp_dir_prefix.is_none() {
        return Err("runLoaderSmoke: either cwd or tempDirPrefix is required".into());
    }

    let mut owned_temp = None;
    let cwd = if let Some(cwd) = &options.cwd {
        cwd.clone()
    } else {
        let prefix = options
            .temp_dir_prefix
            .as_deref()
            .unwrap_or_default()
            .to_owned();
        let parent = options.temp_dir_parent.clone();
        let temp = tokio::task::spawn_blocking(move || -> Result<_, BoxError> {
            let mut builder = tempfile::Builder::new();
            builder.prefix(&prefix);
            match parent.as_deref() {
                Some(parent) => builder
                    .tempdir_in(parent)
                    .map_err(|error| io_error("mkdtemp", parent.as_os_str(), error)),
                None => builder
                    .tempdir()
                    .map_err(|error| io_error("mkdtemp", OsStr::new(&prefix), error)),
            }
        })
        .await
        .map_err(|error| -> BoxError { format!("mkdtemp worker failed: {error}").into() })??;
        let path = temp.path().to_path_buf();
        owned_temp = Some(temp);
        path
    };

    let result = async {
        if let Some(prepare) = &options.prepare {
            prepare(cwd.clone()).await?;
        }
        let mut child_env = ChildEnvironment::new();
        child_env.insert(
            "DSH_HOME".into(),
            Some(cwd.join(".dsh").to_string_lossy().into_owned()),
        );
        child_env.insert(
            "DSH_AGENTS_HOME".into(),
            Some(cwd.join(".agents").to_string_lossy().into_owned()),
        );
        // The caller overlay follows the isolated defaults in the source object spread.
        child_env.extend(options.env.clone());
        let launch = resolve_example_launch(ExampleLaunchOptions {
            src_bin: options.bin_script.clone(),
            lib_bin: options.lib_bin_script.clone(),
            config_args: options
                .bin_args
                .clone()
                .unwrap_or_else(|| vec![options.config_path.clone()]),
            mode: options.mode,
            tsconfig_path: Some(options.tsconfig_path.clone()),
            source_import: options.source_import,
            env: child_env,
        })?;

        let mut command = tokio::process::Command::new(&launch.command);
        command
            .args(&launch.args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_environment(&mut command, &launch.env);
        let expected = options.expected_exit_code.unwrap_or(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                return Err(format!(
                    "{} exited undefined (expected {}). stdout:\n\nstderr:\n",
                    options.label, expected
                )
                .into());
            }
        };
        drop(child.stdin.take());
        let stdout_task = tokio::spawn(read_pipe(
            child.stdout.take().expect("piped stdout is available"),
        ));
        let stderr_task = tokio::spawn(read_pipe(
            child.stderr.take().expect("piped stderr is available"),
        ));
        let timeout_ms = options
            .process_timeout_ms
            .unwrap_or(DEFAULT_PROCESS_TIMEOUT_MS);
        let status = if timeout_ms == 0 {
            Some(
                child
                    .wait()
                    .await
                    .map_err(|error| io_error("wait", OsStr::new(&launch.command), error))?,
            )
        } else {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
                Ok(status) => Some(
                    status.map_err(|error| io_error("wait", OsStr::new(&launch.command), error))?,
                ),
                Err(_) => {
                    child
                        .kill()
                        .await
                        .map_err(|error| io_error("kill", OsStr::new(&launch.command), error))?;
                    None
                }
            }
        };
        let stdout = stdout_task.await??;
        let stderr = stderr_task.await??;
        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        if status.is_none() {
            return Err(format!(
                "{} did not exit within {}s. stdout:\n{}\nstderr:\n{}",
                options.label,
                display_seconds(timeout_ms),
                stdout,
                stderr
            )
            .into());
        }
        let exit_code = status.and_then(|status| status.code());
        if exit_code != Some(expected) {
            let actual = exit_code.map_or_else(|| "undefined".into(), |code| code.to_string());
            return Err(format!(
                "{} exited {} (expected {}). stdout:\n{}\nstderr:\n{}",
                options.label, actual, expected, stdout, stderr
            )
            .into());
        }
        if let Some(inspect) = &options.inspect {
            inspect(cwd.clone()).await?;
        }
        Ok(LoaderSmokeResult { stdout, stderr })
    }
    .await;

    if let Some(temp) = owned_temp {
        let path = temp.path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            temp.close()
                .map_err(|error| io_error("rm", path.as_os_str(), error))
        })
        .await
        .map_err(|error| -> BoxError { format!("rm worker failed: {error}").into() })??;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_only_the_last_source_segment() {
        assert_eq!(
            to_lib_bin("/repo/src/apps/cli/src/bin.ts").unwrap(),
            "/repo/src/apps/cli/lib/bin.js"
        );
    }
}
