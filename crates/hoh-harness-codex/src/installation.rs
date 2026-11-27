//! Find an installed Codex CLI together with its local Code Mode host.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use hoh_external_harness::{HarnessFailure, HarnessResult, code};

use crate::{CodexConfig, version};

const HOST: &str = "codex-code-mode-host";
const STANDALONE_BIN: &str = "packages/standalone/current/bin";

impl CodexConfig {
    pub(crate) async fn with_installation(mut self) -> HarnessResult<Self> {
        if self.executable != Path::new("codex") {
            // Keep an explicit choice, but diagnose incomplete native installations before launch.
            if self.executable.is_absolute() {
                self.executable = launch_path(&self, &self.executable)
                    .await
                    .ok_or_else(missing_host)?;
            }
            return Ok(self);
        }

        let path = self.env("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        let codex_home = self.env("CODEX_HOME").map(PathBuf::from).or_else(|| {
            self.env("HOME")
                .map(|home| PathBuf::from(home).join(".codex"))
        });
        let standalone = codex_home.map(|home| home.join(STANDALONE_BIN));
        let mut incomplete = false;
        for directory in std::env::split_paths(&path).chain(standalone) {
            // A workspace must not supply the CLI via a relative PATH entry.
            if !directory.is_absolute() {
                continue;
            }
            let launcher = directory.join("codex");
            if !is_executable(&launcher) {
                continue;
            }
            if let Some(program) = launch_path(&self, &launcher).await {
                self.executable = program;
                return Ok(self);
            }
            incomplete = true;
        }
        if incomplete {
            Err(missing_host())
        } else {
            Err(HarnessFailure::new(
                code::NOT_INSTALLED,
                version::not_installed_message(),
            ))
        }
    }

    fn env(&self, name: &str) -> Option<OsString> {
        self.extra_env
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| OsString::from(value))
            .or_else(|| std::env::var_os(name))
    }
}

fn missing_host() -> HarnessFailure {
    HarnessFailure::new(
        code::NOT_INSTALLED,
        version::missing_code_mode_host_message(),
    )
}

async fn launch_path(config: &CodexConfig, launcher: &Path) -> Option<PathBuf> {
    let Some(native) = native_codex(launcher) else {
        // npm and other wrappers locate and launch their own native package.
        return Some(launcher.to_owned());
    };
    if is_executable(&host_program(&native)) {
        return Some(native);
    }
    // macOS may report the path through which the CLI was launched.
    if cfg!(target_os = "macos") && is_executable(&launcher.with_file_name(HOST)) {
        return Some(launcher.to_owned());
    }
    // Older CLIs can speak app-server without an external Code Mode process.
    (!requires_code_mode_host(config, &native).await).then_some(native)
}

async fn requires_code_mode_host(config: &CodexConfig, program: &Path) -> bool {
    let mut command = tokio::process::Command::new(program);
    command
        .args(["features", "list"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .envs(
            config
                .extra_env
                .iter()
                .filter(|(name, _)| !hoh_external_harness::supervisor::is_telegram_env(name))
                .map(|(name, value)| (name, value)),
        );
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("TELEGRAM_") {
            command.env_remove(name);
        }
    }
    let Ok(Ok(output)) = tokio::time::timeout(config.version_timeout, command.output()).await
    else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            let mut columns = line.split_whitespace();
            columns.next() == Some("code_mode_host") && columns.last() == Some("true")
        })
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn native_codex(launcher: &Path) -> Option<PathBuf> {
    let program = launcher.canonicalize().ok()?;
    let name = program.file_name()?.to_str()?;
    if !matches!(
        name,
        "codex"
            | "codex-aarch64-apple-darwin"
            | "codex-x86_64-apple-darwin"
            | "codex-aarch64-unknown-linux-gnu"
            | "codex-x86_64-unknown-linux-gnu"
            | "codex-aarch64-unknown-linux-musl"
            | "codex-x86_64-unknown-linux-musl"
    ) || !is_executable(&program)
    {
        return None;
    }

    let mut magic = [0; 4];
    File::open(&program).ok()?.read_exact(&mut magic).ok()?;
    // ELF, Mach-O (either byte order and width), and universal macOS binaries.
    matches!(
        u32::from_be_bytes(magic),
        0x7f454c46
            | 0xfeedface
            | 0xcefaedfe
            | 0xfeedfacf
            | 0xcffaedfe
            | 0xcafebabe
            | 0xbebafeca
            | 0xcafebabf
            | 0xbfbafeca
    )
    .then_some(program)
}

fn host_program(program: &Path) -> PathBuf {
    let executable_dir = program.parent().expect("resolved absolute executable path");
    let package = match executable_dir.file_name().and_then(|name| name.to_str()) {
        Some("bin" | "codex-resources") => executable_dir.parent(),
        Some("MacOS") if executable_dir.ends_with("CodexCLI.app/Contents/MacOS") => {
            executable_dir.ancestors().nth(3)
        }
        _ => None,
    }
    .filter(|root| root.join("codex-package.json").is_file() && root.join("bin").is_dir());

    if let Some(root) = package {
        let resource = root.join("codex-resources").join(HOST);
        if resource.is_file() {
            return resource;
        }
        let sibling = root.join("bin").join(HOST);
        if sibling.is_file() {
            return sibling;
        }
    }
    executable_dir.join(HOST)
}
