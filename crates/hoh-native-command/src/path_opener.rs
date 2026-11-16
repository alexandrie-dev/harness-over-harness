use crate::{
    host_environment::{self, Environment, HostEnvironmentFacts, is_wsl},
    runner::*,
};
use hoh_error::SharedError;
use hoh_timeout::AbortSignal;
use hoh_util_workspace_path::node_path;
use regex::Regex;

pub use crate::runner::NativeCommandRunner as PathOpenerRunner;
#[derive(Default)]
pub struct PathOpenerInternals<'a> {
    pub platform: Option<String>,
    pub os_release: Option<String>,
    pub env: Option<Environment>,
    pub run: Option<&'a dyn NativeCommandRunner>,
}
impl PathOpenerInternals<'_> {
    pub(crate) fn platform(&self) -> &str {
        self.platform
            .as_deref()
            .unwrap_or_else(|| host_environment::platform())
    }
    pub(crate) fn environment(&self) -> Environment {
        self.env
            .clone()
            .unwrap_or_else(host_environment::ambient_environment)
    }
    fn wsl(&self) -> bool {
        is_wsl(&HostEnvironmentFacts {
            platform: self.platform.clone(),
            env: self.env.clone(),
            os_release: self.os_release.clone(),
        })
    }
    pub(crate) fn runner(&self) -> &dyn NativeCommandRunner {
        self.run.unwrap_or(&SystemCommandRunner)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFileManager {
    Finder,
    Explorer,
    Directory,
}

pub fn native_file_manager(internals: &PathOpenerInternals<'_>) -> Option<NativeFileManager> {
    match internals.platform() {
        "darwin" => Some(NativeFileManager::Finder),
        "win32" => Some(NativeFileManager::Explorer),
        "linux" if internals.wsl() => Some(NativeFileManager::Explorer),
        "linux" => Some(NativeFileManager::Directory),
        _ => None,
    }
}
pub fn can_open_native_path(internals: &PathOpenerInternals<'_>) -> bool {
    match internals.platform() {
        "darwin" | "win32" => true,
        "linux" => {
            internals.wsl()
                || ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|key| {
                    internals
                        .environment()
                        .get(*key)
                        .is_some_and(|s| !s.is_empty())
                })
        }
        _ => false,
    }
}

fn mac_bundle_for_https(plist: &str) -> Option<String> {
    let whitespace = |pattern: &str| {
        pattern.replace(r"\s", &format!("[{}]", hoh_util_values::JS_WHITE_SPACE_SET))
    };
    // These fixed expressions are valid; no user expression is compiled.
    let nested = Regex::new(&whitespace(r"LSHandlerPreferredVersions\s*=\s*\{[^}]*\};"))
        .expect("fixed regex");
    let block = Regex::new(&whitespace(
        r#"\{[^{}]*LSHandlerURLScheme\s*=\s*"?https"?;[^{}]*\}"#,
    ))
    .expect("fixed regex");
    let role = Regex::new(&whitespace(
        r#"LSHandlerRoleAll\s*=\s*"?([a-zA-Z0-9_.-]+)"?;"#,
    ))
    .expect("fixed regex");
    let stripped = nested.replace_all(plist, "");
    let found = block.find(&stripped)?;
    Some(role.captures(found.as_str())?.get(1)?.as_str().to_owned())
}

fn explorer_target(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = if let Some(rest) = path.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else {
        path.strip_prefix("//?/").unwrap_or(&path).to_owned()
    };
    let trailing = path.ends_with('/');
    let (prefix, rest) = if let Some(unc) = path.strip_prefix("//") {
        let (server, rest) = unc.split_once('/').unwrap_or((unc, ""));
        (
            format!("file://{}/", server.to_ascii_lowercase()),
            rest.to_owned(),
        )
    } else if path.as_bytes().get(1) == Some(&b':') {
        ("file:///".into(), path.clone())
    } else {
        let cwd = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .replace('\\', "/");
        (
            "file:///".into(),
            format!("{cwd}/{path}").trim_start_matches('/').to_owned(),
        )
    };
    let mut parts = Vec::new();
    let drive = rest.as_bytes().get(1) == Some(&b':');
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() > usize::from(drive) {
                    parts.pop();
                }
            }
            _ => parts.push(part),
        }
    }
    let mut resolved = parts.join("/");
    if trailing {
        resolved.push('/');
        // Node restores the final separator using the host separator, including drive roots.
        #[cfg(not(windows))]
        if resolved.len() == 3 && resolved.as_bytes().get(1) == Some(&b':') {
            resolved.push('/');
        }
    }
    let mut encoded = prefix;
    for c in resolved.chars() {
        match c {
            ' ' => encoded.push_str("%20"),
            '#' => encoded.push_str("%23"),
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3F"),
            ',' => encoded.push_str("%2C"),
            '=' => encoded.push_str("%3D"),
            c if c.is_ascii_control()
                || matches!(
                    c,
                    '"' | '<' | '>' | '[' | ']' | '^' | '`' | '{' | '|' | '}' | '~'
                ) =>
            {
                use std::fmt::Write;
                let _ = write!(encoded, "%{:02X}", c as u32);
            }
            _ => encoded.push(c),
        }
    }
    encoded
}

async fn run_explorer(
    args: &[String],
    signal: &AbortSignal,
    run: &dyn NativeCommandRunner,
) -> Result<(), SharedError> {
    if let Err(error) = run.run("explorer.exe", args, signal, Window::Visible).await {
        signal.throw_if_aborted()?;
        if error
            .downcast_ref::<NativeCommandError>()
            .and_then(|e| e.code.as_ref())
            != Some(&CommandCode::Exit(1))
        {
            return Err(error);
        }
    }
    Ok(())
}
pub(crate) async fn translated_path(
    path: &str,
    signal: &AbortSignal,
    run: &dyn NativeCommandRunner,
) -> Result<String, SharedError> {
    let result = run
        .run(
            "wslpath",
            &["-w".into(), path.into()],
            signal,
            Window::Hidden,
        )
        .await?;
    signal.throw_if_aborted()?;
    let path = result.stdout.trim_end_matches(['\r', '\n']);
    if path.is_empty() {
        return Err(NativeCommandError::message("wslpath returned no Windows path").into());
    }
    Ok(path.into())
}

#[derive(PartialEq, Eq)]
enum PathOpenIntent {
    Default,
    Association,
    TextEditor,
}
async fn open_native_path_with_intent(
    path: &str,
    signal: &AbortSignal,
    intent: PathOpenIntent,
    internals: &PathOpenerInternals<'_>,
) -> Result<(), SharedError> {
    let platform = internals.platform();
    let run = internals.runner();
    let wsl = platform == "linux" && internals.wsl();
    let extension = node_path::extname(path).to_lowercase();
    if !wsl
        && intent == PathOpenIntent::Default
        && [".html", ".htm", ".xhtml", ".svg"].contains(&extension.as_str())
    {
        if platform == "darwin" {
            if let Ok(output) = run
                .run(
                    "defaults",
                    &[
                        "read".into(),
                        "com.apple.LaunchServices/com.apple.launchservices.secure".into(),
                    ],
                    signal,
                    Window::Hidden,
                )
                .await
                && let Some(bundle) = mac_bundle_for_https(&output.stdout)
            {
                run.run(
                    "open",
                    &["-b".into(), bundle, path.into()],
                    signal,
                    Window::Hidden,
                )
                .await?;
                return Ok(());
            }
        } else if platform == "linux"
            && let Some(browser) = internals
                .environment()
                .get("BROWSER")
                .filter(|s| !s.is_empty())
        {
            run.run(browser, &[path.into()], signal, Window::Hidden)
                .await?;
            return Ok(());
        }
    }
    match platform {
        "darwin" => {
            let args = if intent == PathOpenIntent::TextEditor {
                vec!["-t".into(), path.into()]
            } else {
                vec![path.into()]
            };
            run.run("open", &args, signal, Window::Hidden).await?;
        }
        "win32" => run_explorer(&[explorer_target(path)], signal, run).await?,
        "linux" if wsl => {
            run_explorer(
                &[explorer_target(&translated_path(path, signal, run).await?)],
                signal,
                run,
            )
            .await?
        }
        "linux" => {
            run.run("xdg-open", &[path.into()], signal, Window::Hidden)
                .await?;
        }
        _ => {
            return Err(NativeCommandError::message(format!(
                "native path opener is unsupported on {platform}"
            ))
            .into());
        }
    }
    Ok(())
}
pub async fn open_native_path(
    path: &str,
    signal: &AbortSignal,
    internals: &PathOpenerInternals<'_>,
) -> Result<(), SharedError> {
    open_native_path_with_intent(path, signal, PathOpenIntent::Default, internals).await
}
pub async fn open_native_associated_path(
    path: &str,
    signal: &AbortSignal,
    internals: &PathOpenerInternals<'_>,
) -> Result<(), SharedError> {
    open_native_path_with_intent(path, signal, PathOpenIntent::Association, internals).await
}
pub async fn open_native_text_file(
    path: &str,
    signal: &AbortSignal,
    internals: &PathOpenerInternals<'_>,
) -> Result<(), SharedError> {
    open_native_path_with_intent(path, signal, PathOpenIntent::TextEditor, internals).await
}
pub async fn reveal_native_path(
    path: &str,
    signal: &AbortSignal,
    internals: &PathOpenerInternals<'_>,
) -> Result<(), SharedError> {
    signal.throw_if_aborted()?;
    let run = internals.runner();
    match native_file_manager(internals) {
        Some(NativeFileManager::Finder) => {
            run.run("open", &["-R".into(), path.into()], signal, Window::Hidden)
                .await?;
        }
        Some(NativeFileManager::Explorer) => {
            let path = if internals.platform() == "linux" {
                translated_path(path, signal, run).await?
            } else {
                path.into()
            };
            run_explorer(&["/select,".into(), explorer_target(&path)], signal, run).await?;
        }
        Some(NativeFileManager::Directory) => {
            let parent = node_path::dirname(path);
            run.run("xdg-open", &[parent], signal, Window::Hidden)
                .await?;
        }
        None => {
            return Err(NativeCommandError::message(format!(
                "native file manager is unsupported on {}",
                internals.platform()
            ))
            .into());
        }
    }
    Ok(())
}
