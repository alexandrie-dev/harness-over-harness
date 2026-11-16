//! Shared user-data root resolution and native watch-path canonicalization.
use std::{
    collections::HashMap,
    io,
    path::{Component, Path, PathBuf},
};

pub const DSH_HOME_DIR_NAME: &str = ".dsh";
pub const DEFAULT_DSH_HOME_DISPLAY: &str = "~/.dsh";
pub const DSH_HOME_ENV: &str = "DSH_HOME";
pub const AGENTS_HOME_DIR_NAME: &str = ".agents";
pub const DEFAULT_AGENTS_HOME_DISPLAY: &str = "~/.agents";
pub const AGENTS_HOME_ENV: &str = "DSH_AGENTS_HOME";

fn home() -> io::Result<PathBuf> {
    std::env::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "OS home directory is unavailable"))
}
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}
fn resolve(path: &Path) -> io::Result<PathBuf> {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    Ok(normalize(&std::path::absolute(path)?))
}
// Node join concatenates segments before normalizing; a rooted child does not discard the base.
fn join(base: &Path, segments: &[&str]) -> PathBuf {
    let mut joined = base.as_os_str().to_os_string();
    for segment in segments.iter().filter(|s| !s.is_empty()) {
        joined.push(std::path::MAIN_SEPARATOR_STR);
        joined.push(segment);
    }
    let trailing = segments
        .last()
        .is_some_and(|s| s.ends_with(std::path::MAIN_SEPARATOR));
    let mut path = normalize(Path::new(&joined));
    if trailing {
        path.push("");
    }
    path
}
pub fn default_dsh_home() -> io::Result<PathBuf> {
    Ok(home()?.join(DSH_HOME_DIR_NAME))
}
pub fn default_agents_home() -> io::Result<PathBuf> {
    Ok(home()?.join(AGENTS_HOME_DIR_NAME))
}
pub fn expand_home_path(path: &str) -> io::Result<PathBuf> {
    if path == "~" {
        return home();
    }
    if path.starts_with("~/") || path.starts_with("~\\") {
        return Ok(join(&home()?, &[&path[2..]]));
    }
    Ok(path.into())
}
fn resolve_root(
    configured: Option<&str>,
    env: Option<&HashMap<String, String>>,
    key: &str,
    fallback: fn() -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    let from_env = match env {
        Some(env) => env.get(key).cloned(),
        None => std::env::var(key).ok(),
    };
    match configured.or(from_env
        .as_deref()
        .filter(|v| !v.trim_matches(js_whitespace).is_empty()))
    {
        Some(value) => resolve(&expand_home_path(value)?),
        None => resolve(&fallback()?),
    }
}
fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
pub fn resolve_dsh_home(
    configured: Option<&str>,
    env: Option<&HashMap<String, String>>,
) -> io::Result<PathBuf> {
    resolve_root(configured, env, DSH_HOME_ENV, default_dsh_home)
}
pub fn resolve_agents_home(
    configured: Option<&str>,
    env: Option<&HashMap<String, String>>,
) -> io::Result<PathBuf> {
    resolve_root(configured, env, AGENTS_HOME_ENV, default_agents_home)
}
pub fn dsh_home_display(resolved_home: &Path) -> io::Result<&'static str> {
    Ok(if resolved_home == resolve(&default_dsh_home()?)? {
        DEFAULT_DSH_HOME_DISPLAY
    } else {
        "$DSH_HOME"
    })
}
pub fn agents_home_display(resolved_home: &Path) -> io::Result<&'static str> {
    Ok(if resolved_home == resolve(&default_agents_home()?)? {
        DEFAULT_AGENTS_HOME_DISPLAY
    } else {
        "$DSH_AGENTS_HOME"
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DshHomeCategory {
    Config,
    Data,
    State,
    Cache,
    Backup,
}
impl DshHomeCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Data => "data",
            Self::State => "state",
            Self::Cache => "cache",
            Self::Backup => "backup",
        }
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPathLocation {
    pub category: DshHomeCategory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsh_home: Option<String>,
}
impl From<DshHomeCategory> for DshPathLocation {
    fn from(category: DshHomeCategory) -> Self {
        Self {
            category,
            dsh_home: None,
        }
    }
}
pub fn dsh_home_path(
    segments: &[&str],
    env: Option<&HashMap<String, String>>,
) -> io::Result<PathBuf> {
    Ok(join(&resolve_dsh_home(None, env)?, segments))
}
pub fn dsh_path(
    location: impl Into<DshPathLocation>,
    segments: &[&str],
    env: Option<&HashMap<String, String>>,
) -> io::Result<PathBuf> {
    let location = location.into();
    Ok(join(
        &resolve_dsh_home(location.dsh_home.as_deref(), env)?.join(location.category.as_str()),
        segments,
    ))
}
pub fn dsh_cache_path(
    dsh_home: Option<&str>,
    segments: &[&str],
    env: Option<&HashMap<String, String>>,
) -> io::Result<PathBuf> {
    dsh_path(
        DshPathLocation {
            category: DshHomeCategory::Cache,
            dsh_home: dsh_home.map(str::to_owned),
        },
        segments,
        env,
    )
}
pub async fn canonicalize_watch_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let mut current = resolve(path.as_ref())?;
    let mut missing = Vec::new();
    loop {
        match tokio::fs::canonicalize(&current).await {
            Ok(mut canonical) => {
                if !missing.is_empty() {
                    let _directory = tokio::fs::read_dir(&canonical).await?;
                }
                for part in missing.into_iter().rev() {
                    canonical.push(part);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = current.file_name().map(|n| n.to_os_string()) else {
                    return Err(error);
                };
                if !current.pop() {
                    return Err(error);
                }
                missing.push(name);
            }
            Err(error) => return Err(error),
        }
    }
}
