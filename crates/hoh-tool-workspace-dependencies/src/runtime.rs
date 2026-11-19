use crate::error::{WorkspaceDependencyError, WorkspaceDependencyErrorCode};
use crate::shim::host_path as node_path;
use hoh_error::{BoxError, node_fs_error_message};
use hoh_util_values::{js_json_parse, js_json_stringify};
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub type Result<T> = std::result::Result<T, BoxError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryRuntimeManifest {
    pub desktop_version: String,
    pub platform: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_digest: Option<String>,
    pub python: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pnpm: Option<String>,
    pub python_packages: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDependencies {
    pub python: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_packages: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pnpm: Option<String>,
    pub python_packages: String,
    pub python_distributions: IndexMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct NodeFsError {
    message: String,
    #[source]
    source: io::Error,
}

#[derive(Debug, thiserror::Error)]
enum RuntimeError {
    #[error("primary runtime: installation path is a filesystem link: {0}")]
    InstallationLink(String),
}

fn fs_error(error: io::Error, syscall: &str, path: &str, dest: Option<&str>) -> BoxError {
    Box::new(NodeFsError {
        message: node_fs_error_message(&error, syscall, Some(path), dest),
        source: error,
    })
}

fn invalid_payload(message: impl Into<String>, cause: Option<BoxError>) -> BoxError {
    let message = message.into();
    match cause {
        Some(cause) => Box::new(WorkspaceDependencyError::with_cause(
            message,
            WorkspaceDependencyErrorCode::InvalidPayload,
            cause,
        )),
        None => Box::new(WorkspaceDependencyError::new(
            message,
            WorkspaceDependencyErrorCode::InvalidPayload,
        )),
    }
}

fn version_regex() -> &'static Regex {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    // The expression is a source constant and is covered by manifest tests.
    VERSION.get_or_init(|| {
        Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9_.-]+)?$")
            .expect("static version regex")
    })
}

fn distribution_name_regex() -> &'static Regex {
    static NAME: OnceLock<Regex> = OnceLock::new();
    // The expression is a source constant and is covered by manifest tests.
    NAME.get_or_init(|| {
        Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").expect("static distribution-name regex")
    })
}

fn distribution_version_regex() -> &'static Regex {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    // The expression is a source constant and is covered by manifest tests.
    VERSION.get_or_init(|| {
        Regex::new(r"^[0-9][A-Za-z0-9_.!+-]*$").expect("static distribution-version regex")
    })
}

fn string<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

fn valid_version(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| version_regex().is_match(value))
        .map(str::to_owned)
}

fn normalized_distribution_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut separator = false;
    for byte in name.bytes() {
        if matches!(byte, b'-' | b'_' | b'.') {
            if !separator {
                normalized.push('-');
                separator = true;
            }
        } else {
            normalized.push(char::from(byte.to_ascii_lowercase()));
            separator = false;
        }
    }
    normalized
}

pub fn parse_primary_runtime(value: &Value) -> Result<PrimaryRuntimeManifest> {
    let Some(metadata) = value.as_object() else {
        return Err(invalid_payload("primary runtime: invalid metadata", None));
    };
    let legacy = metadata.contains_key("components");
    let versions = if legacy {
        let Some(components) = metadata.get("components").and_then(Value::as_object) else {
            return Err(invalid_payload("primary runtime: invalid metadata", None));
        };
        if ["python", "node", "pnpm"]
            .iter()
            .any(|key| metadata.contains_key(*key))
        {
            return Err(invalid_payload("primary runtime: invalid metadata", None));
        }
        components
    } else {
        metadata
    };

    let desktop_version = string(metadata, "desktopVersion")
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let platform = string(metadata, "platform")
        .filter(|value| matches!(*value, "win32" | "darwin" | "linux"))
        .map(str::to_owned);
    let arch = string(metadata, "arch")
        .filter(|value| matches!(*value, "x64" | "arm64"))
        .map(str::to_owned);
    let python = valid_version(string(versions, "python"));
    let node = match versions.get("node") {
        None => Some(None),
        Some(value) => valid_version(value.as_str()).map(Some),
    };
    let pnpm = match versions.get("pnpm") {
        None => Some(None),
        Some(value) => valid_version(value.as_str()).map(Some),
    };
    let payload_digest = match metadata.get("payloadDigest") {
        None => Some(None),
        Some(Value::String(value))
            if value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            Some(Some(value.clone()))
        }
        _ => None,
    };
    let packages = match metadata.get("pythonPackages") {
        None if legacy => Some(IndexMap::new()),
        Some(Value::Object(packages)) => {
            let mut result = IndexMap::with_capacity(packages.len());
            let mut normalized = HashSet::with_capacity(packages.len());
            let mut valid = true;
            for (name, value) in packages {
                let Some(version) = value.as_str() else {
                    valid = false;
                    break;
                };
                if !distribution_name_regex().is_match(name)
                    || !distribution_version_regex().is_match(version)
                    || !normalized.insert(normalized_distribution_name(name))
                {
                    valid = false;
                    break;
                }
                result.insert(name.clone(), version.to_owned());
            }
            valid.then_some(result)
        }
        _ => None,
    };

    let (
        Some(desktop_version),
        Some(platform),
        Some(arch),
        Some(python),
        Some(node),
        Some(pnpm),
        Some(payload_digest),
        Some(python_packages),
    ) = (
        desktop_version,
        platform,
        arch,
        python,
        node,
        pnpm,
        payload_digest,
        packages,
    )
    else {
        return Err(invalid_payload("primary runtime: invalid metadata", None));
    };
    if pnpm.is_some() && node.is_none() {
        return Err(invalid_payload("primary runtime: invalid metadata", None));
    }
    if legacy {
        for name in ["numpy", "pandas"] {
            let Some(component_version) = valid_version(string(versions, name)) else {
                return Err(invalid_payload("primary runtime: invalid metadata", None));
            };
            if let Some(distribution_version) = python_packages.iter().find_map(|(key, value)| {
                (normalized_distribution_name(key) == name).then_some(value)
            }) && distribution_version != &component_version
            {
                return Err(invalid_payload(
                    format!("primary runtime: conflicting {name} distribution version"),
                    None,
                ));
            }
        }
    }
    Ok(PrimaryRuntimeManifest {
        desktop_version,
        platform,
        arch,
        payload_digest,
        python,
        node,
        pnpm,
        python_packages,
    })
}

pub async fn read_primary_runtime(root: &str) -> Result<PrimaryRuntimeManifest> {
    let path = node_path::join(&[root, "runtime.json"]);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(invalid_payload(
                "primary runtime: runtime.json is missing",
                Some(fs_error(error, "open", &path, None)),
            ));
        }
        Err(error) => return Err(fs_error(error, "open", &path, None)),
    };
    let text = String::from_utf8_lossy(&bytes);
    let value = js_json_parse(&text).map_err(|cause| {
        invalid_payload(
            format!("primary runtime: runtime.json is not valid JSON: {cause}"),
            Some(Box::new(cause)),
        )
    })?;
    parse_primary_runtime(&value)
}

pub fn workspace_dependency_paths(
    root: &str,
    manifest: &PrimaryRuntimeManifest,
) -> WorkspaceDependencies {
    let dependencies = node_path::join(&[root, "dependencies"]);
    let windows = manifest.platform == "win32";
    let node = manifest.node.as_ref().map(|_| {
        node_path::join(&[
            &dependencies,
            "node",
            "bin",
            if windows { "node.exe" } else { "node" },
        ])
    });
    let node_packages = manifest
        .node
        .as_ref()
        .map(|_| node_path::join(&[&dependencies, "node", "node_modules"]));
    let pnpm = manifest
        .pnpm
        .as_ref()
        .map(|_| node_path::join(&[&dependencies, "pnpm", "bin", "pnpm.mjs"]));
    let python = if windows {
        node_path::join(&[&dependencies, "python", "python.exe"])
    } else {
        node_path::join(&[&dependencies, "python", "bin", "python3"])
    };
    let python_packages = if windows {
        node_path::join(&[&dependencies, "python", "Lib", "site-packages"])
    } else {
        let parts = manifest.python.split('.').take(2).collect::<Vec<_>>();
        let version = format!("python{}", parts.join("."));
        node_path::join(&[&dependencies, "python", "lib", &version, "site-packages"])
    };
    WorkspaceDependencies {
        python,
        node,
        node_packages,
        pnpm,
        python_packages,
        python_distributions: manifest.python_packages.clone(),
    }
}

pub async fn primary_runtime_python_aliases(
    root: &str,
    manifest: &PrimaryRuntimeManifest,
) -> Result<IndexMap<String, String>> {
    let mut aliases = IndexMap::new();
    if manifest.platform == "win32" {
        return Ok(aliases);
    }
    let interpreter = format!(
        "python{}",
        manifest
            .python
            .split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".")
    );
    for name in ["python", "python3"] {
        let relative = format!("dependencies/python/bin/{name}");
        let path = node_path::join(&[root, &relative]);
        let entry = match tokio::fs::symlink_metadata(&path).await {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(fs_error(error, "lstat", &path, None)),
        };
        if !entry.file_type().is_symlink() {
            continue;
        }
        let target = tokio::fs::read_link(&path)
            .await
            .map_err(|error| fs_error(error, "readlink", &path, None))?;
        if target.to_string_lossy() != interpreter {
            return Err(invalid_payload(
                format!("primary runtime: invalid Python alias {relative}"),
                None,
            ));
        }
        let interpreter_path = node_path::join(&[root, "dependencies/python/bin", &interpreter]);
        let target_entry = tokio::fs::symlink_metadata(&interpreter_path)
            .await
            .map_err(|error| fs_error(error, "lstat", &interpreter_path, None))?;
        if !target_entry.is_file() {
            return Err(invalid_payload(
                format!("primary runtime: invalid Python alias {relative}"),
                None,
            ));
        }
        aliases.insert(relative, interpreter.clone());
    }
    Ok(aliases)
}

async fn copy_tree_dereferenced(
    source: &str,
    destination: &str,
    excluded: &HashSet<String>,
) -> Result<()> {
    let mut pending = vec![(
        source.to_owned(),
        destination.to_owned(),
        Vec::<PathBuf>::new(),
    )];
    while let Some((from, to, ancestors)) = pending.pop() {
        if excluded.contains(&from) {
            continue;
        }
        let metadata = tokio::fs::metadata(&from)
            .await
            .map_err(|error| fs_error(error, "stat", &from, None))?;
        if metadata.is_dir() {
            let canonical = tokio::fs::canonicalize(&from)
                .await
                .map_err(|error| fs_error(error, "realpath", &from, None))?;
            if ancestors.contains(&canonical) {
                let error = io::Error::other("too many levels of symbolic links");
                return Err(fs_error(error, "stat", &from, None));
            }
            let mut descendants = ancestors;
            descendants.push(canonical);
            tokio::fs::create_dir_all(&to)
                .await
                .map_err(|error| fs_error(error, "mkdir", &to, None))?;
            let mut entries = tokio::fs::read_dir(&from)
                .await
                .map_err(|error| fs_error(error, "scandir", &from, None))?;
            let mut names = Vec::new();
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| fs_error(error, "scandir", &from, None))?
            {
                names.push(entry.file_name());
            }
            names.sort_by(|left, right| left.as_encoded_bytes().cmp(right.as_encoded_bytes()));
            for name in names.into_iter().rev() {
                let name = name.to_string_lossy();
                pending.push((
                    node_path::join(&[&from, &name]),
                    node_path::join(&[&to, &name]),
                    descendants.clone(),
                ));
            }
        } else {
            if let Some(parent) = Path::new(&to).parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|error| fs_error(error, "mkdir", &parent.to_string_lossy(), None))?;
            }
            tokio::fs::copy(&from, &to)
                .await
                .map_err(|error| fs_error(error, "copyfile", &from, Some(&to)))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn create_file_symlink(target: &str, path: &str) -> Result<()> {
    tokio::fs::symlink(target, path)
        .await
        .map_err(|error| fs_error(error, "symlink", target, Some(path)))
}

#[cfg(windows)]
async fn create_file_symlink(target: &str, path: &str) -> Result<()> {
    tokio::fs::symlink_file(target, path)
        .await
        .map_err(|error| fs_error(error, "symlink", target, Some(path)))
}

pub async fn copy_primary_runtime(
    source: &str,
    destination: &str,
    manifest: &PrimaryRuntimeManifest,
) -> Result<()> {
    let aliases = primary_runtime_python_aliases(source, manifest).await?;
    let excluded = aliases
        .keys()
        .map(|path| node_path::join(&[source, path]))
        .collect::<HashSet<_>>();
    copy_tree_dereferenced(source, destination, &excluded).await?;
    for (path, target) in aliases {
        create_file_symlink(&target, &node_path::join(&[destination, &path])).await?;
    }
    Ok(())
}

async fn validate_payload_entries(paths: &WorkspaceDependencies) -> Result<()> {
    let entries = [
        (Some(&paths.python), "file"),
        (paths.node.as_ref(), "file"),
        (paths.pnpm.as_ref(), "file"),
        (Some(&paths.python_packages), "directory"),
        (paths.node_packages.as_ref(), "directory"),
    ];
    for (path, kind) in entries {
        let Some(path) = path else { continue };
        let entry = match tokio::fs::metadata(path).await {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(invalid_payload(
                    format!("primary runtime: expected {kind} at {path}"),
                    Some(fs_error(error, "stat", path, None)),
                ));
            }
            Err(error) => return Err(fs_error(error, "stat", path, None)),
        };
        let valid = if kind == "file" {
            entry.is_file()
        } else {
            entry.is_dir()
        };
        if !valid {
            return Err(invalid_payload(
                format!("primary runtime: expected {kind} at {path}"),
                None,
            ));
        }
    }
    Ok(())
}

async fn exists(path: &str) -> Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(Box::new(RuntimeError::InstallationLink(path.to_owned())))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fs_error(error, "lstat", path, None)),
    }
}

fn node_platform(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        "windows" => "win32",
        value => value,
    }
}

fn current_platform() -> &'static str {
    node_platform(std::env::consts::OS)
}

fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        value => value,
    }
}

async fn compatible_manifest(source: &str) -> Result<PrimaryRuntimeManifest> {
    let manifest = read_primary_runtime(source).await?;
    if manifest.platform != current_platform() || manifest.arch != current_arch() {
        return Err(Box::new(WorkspaceDependencyError::new(
            "primary runtime: incompatible platform or architecture",
            WorkspaceDependencyErrorCode::UnsupportedPlatform,
        )));
    }
    Ok(manifest)
}

pub async fn resolve_primary_runtime(source: &str) -> Result<WorkspaceDependencies> {
    let manifest = compatible_manifest(source).await?;
    primary_runtime_python_aliases(source, &manifest).await?;
    let paths = workspace_dependency_paths(source, &manifest);
    validate_payload_entries(&paths).await?;
    Ok(paths)
}

async fn remove_tree(path: &str) -> Result<()> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(fs_error(error, "lstat", path, None)),
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        tokio::fs::remove_dir_all(path).await
    } else {
        tokio::fs::remove_file(path).await
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(fs_error(error, "rm", path, None)),
    }
}

async fn rename(from: &str, to: &str) -> Result<()> {
    tokio::fs::rename(from, to)
        .await
        .map_err(|error| fs_error(error, "rename", from, Some(to)))
}

async fn staging_directory(parent: &str) -> Result<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    loop {
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = node_path::join(&[
            parent,
            &format!(".primary-runtime-{}-{suffix:x}", std::process::id()),
        ]);
        match tokio::fs::create_dir(&path).await {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(fs_error(error, "mkdir", &path, None)),
        }
    }
}

pub async fn install_primary_runtime(source: &str, root: &str) -> Result<WorkspaceDependencies> {
    let manifest = compatible_manifest(source).await?;
    let parent = node_path::dirname(root);
    tokio::fs::create_dir_all(&parent)
        .await
        .map_err(|error| fs_error(error, "mkdir", &parent, None))?;
    let previous = format!("{root}.previous");
    let previous_exists = exists(&previous).await?;
    let root_exists = exists(root).await?;
    if !root_exists && previous_exists {
        rename(&previous, root).await?;
    }
    let runtime_json = node_path::join(&[root, "runtime.json"]);
    if exists(&runtime_json).await? {
        let installed = read_primary_runtime(root).await?;
        let installed = js_json_stringify(&serde_json::to_value(installed)?);
        let source = js_json_stringify(&serde_json::to_value(&manifest)?);
        if installed == source {
            primary_runtime_python_aliases(root, &manifest).await?;
            let paths = workspace_dependency_paths(root, &manifest);
            validate_payload_entries(&paths).await?;
            return Ok(paths);
        }
    }

    let staging = staging_directory(&parent).await?;
    let result = async {
        copy_primary_runtime(source, &staging, &manifest).await?;
        let paths = workspace_dependency_paths(&staging, &manifest);
        validate_payload_entries(&paths).await?;
        remove_tree(&previous).await?;
        let replacing = exists(root).await?;
        if replacing {
            rename(root, &previous).await?;
        }
        if let Err(error) = rename(&staging, root).await {
            if replacing {
                rename(&previous, root).await?;
            }
            return Err(error);
        }
        remove_tree(&previous).await?;
        Ok(())
    }
    .await;
    let cleanup = remove_tree(&staging).await;
    cleanup?;
    result?;
    Ok(workspace_dependency_paths(root, &manifest))
}

pub(crate) fn is_absolute(path: &str) -> bool {
    node_path::is_absolute(path)
}

#[cfg(test)]
mod tests {
    use super::node_platform;

    #[test]
    fn maps_rust_operating_system_names_to_node_platform_names() {
        assert_eq!(node_platform("windows"), "win32");
        assert_eq!(node_platform("macos"), "darwin");
        assert_eq!(node_platform("linux"), "linux");
    }
}
