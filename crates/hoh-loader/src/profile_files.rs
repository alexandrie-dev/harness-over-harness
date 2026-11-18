//! Profile discovery and filesystem metadata; no package installation or module resolution.
use crate::{BundleSource, EvalEnv, LoadError, Profile};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

const PATCH: &str = "# Your patch layer for this dsh profile, applied after every bundle layer:\n# a top-level YAML array of loader patch entries (id-targeted config\n# overrides, disables, and insert lists; `!!js` expressions allowed).\n[]\n";
const WORKSPACE: &str = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n";
fn io(file: &Path, source: std::io::Error) -> LoadError {
    LoadError::Io {
        file: file.into(),
        source,
    }
}
pub fn resolve_profile_dir(name: &str, home: &Path) -> Result<PathBuf, LoadError> {
    if name.is_empty() || matches!(name, "." | ".." | "node_modules") || name.contains(['/', '\\'])
    {
        return Err(LoadError::Runtime(format!(
            "dsh: invalid profile name {}",
            hoh_util_values::js_json_stringify(&json!(name))
        )));
    }
    let path = home.join("profiles").join(name);
    Ok(PathBuf::from(if cfg!(windows) {
        crate::expr::windows_normalize(&path.to_string_lossy())
    } else {
        crate::expr::normalize(&path.to_string_lossy())
    }))
}
pub fn init_profile(dir: &Path, bundles: &[String]) -> Result<(), LoadError> {
    std::fs::create_dir_all(dir).map_err(|source| io(dir, source))?;
    if !dir.join("package.json").exists() {
        write_profile_manifest(
            dir,
            &json!({"name":format!("dsh-profile-{}",dir.file_name().unwrap_or_default().to_string_lossy()),"private":true,"dependencies":{},"dsh":{"profile":{"bundles":bundles}}}),
        )?;
    }
    for (name, text) in [
        ("cordis.patch.yml", PATCH),
        ("pnpm-workspace.yaml", WORKSPACE),
    ] {
        let path = dir.join(name);
        if !path.exists() {
            std::fs::write(&path, text).map_err(|source| io(&path, source))?;
        }
    }
    Ok(())
}
// These read-only filesystem errors are the Node strings exposed by readProfileManifest.
// hoh-error is outside this task's allowed direct dependencies; keep this adapter local until authorized.
fn read_error(error: &std::io::Error, file: &Path) -> String {
    use std::io::ErrorKind;
    let text = match error.kind() {
        ErrorKind::NotFound => "ENOENT: no such file or directory",
        ErrorKind::PermissionDenied => "EACCES: permission denied",
        ErrorKind::IsADirectory => {
            return "Error: EISDIR: illegal operation on a directory, read".into();
        }
        ErrorKind::NotADirectory => "ENOTDIR: not a directory",
        _ => return format!("Error: {error}"),
    };
    format!("Error: {text}, open '{}'", file.display())
}
pub fn read_profile_manifest(bin_name: &str, dir: &Path) -> Result<Value, LoadError> {
    let path = dir.join("package.json");
    let text = std::fs::read_to_string(&path).map_err(|error| {
        LoadError::Runtime(format!(
            "{bin_name}: failed to read profile manifest {}: {}",
            path.display(),
            read_error(&error, &path)
        ))
    })?;
    let value = hoh_util_values::js_json_parse(&text)
        .map_err(|error| LoadError::Runtime(error.to_string()))?;
    if !value.is_object() {
        return Err(LoadError::Runtime(format!(
            "{bin_name}: profile manifest {} must hold a JSON object",
            path.display()
        )));
    }
    Ok(value)
}
pub fn write_profile_manifest(dir: &Path, manifest: &Value) -> Result<(), LoadError> {
    let path = dir.join("package.json");
    std::fs::write(
        &path,
        format!(
            "{}\n",
            hoh_util_values::js_json_stringify_pretty(manifest, 2)
        ),
    )
    .map_err(|source| io(&path, source))
}
fn spread(value: &Value) -> Result<Map<String, Value>, LoadError> {
    Ok(match value {
        Value::Object(map) => map.clone(),
        Value::Array(values) => values
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, value)| (i.to_string(), value))
            .collect(),
        Value::String(text) => {
            let mut map = Map::new();
            for (index, unit) in text.encode_utf16().enumerate() {
                let character = char::from_u32(u32::from(unit)).ok_or_else(|| {
                    LoadError::Runtime(
                        "profile object spread contains an unrepresentable UTF-16 surrogate".into(),
                    )
                })?;
                map.insert(index.to_string(), character.to_string().into());
            }
            map
        }
        _ => Map::new(),
    })
}
pub(crate) fn with_bundles(manifest: &Value, bundles: &[Value]) -> Result<Value, LoadError> {
    let mut result = spread(manifest)?;
    let dsh = manifest.get("dsh").unwrap_or(&Value::Null);
    let mut dsh_result = spread(dsh)?;
    let mut profile = spread(dsh.get("profile").unwrap_or(&Value::Null))?;
    profile.insert("bundles".into(), Value::Array(bundles.to_vec()));
    dsh_result.insert("profile".into(), profile.into());
    result.insert("dsh".into(), dsh_result.into());
    Ok(result.into())
}
pub(crate) fn drop_retired(dir: &Path, manifest: &mut Value) -> Result<(), LoadError> {
    let Some(bundles) = manifest
        .pointer("/dsh/profile/bundles")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    let kept: Vec<_> = bundles
        .iter()
        .filter(|value| {
            !matches!(
                value.as_str(),
                Some(
                    "@deepseek-ai/dsh-experimental-schedule-bundle"
                        | "@deepseek-ai/dsh-experimental-agent-team-profile"
                )
            )
        })
        .cloned()
        .collect();
    if kept.len() != bundles.len() {
        *manifest = with_bundles(manifest, &kept)?;
        write_profile_manifest(dir, manifest)?;
    }
    Ok(())
}
pub fn load_profile(
    bin_name: &str,
    name: &str,
    bundles: &dyn BundleSource,
    home: &Path,
    overlays: &[PathBuf],
    env: &EvalEnv,
) -> Result<Profile, LoadError> {
    let home = if home.is_absolute() {
        home.to_path_buf()
    } else {
        env.cwd.join(home)
    };
    let dir = resolve_profile_dir(name, &home)?;
    let template: Option<&[&str]> = match name {
        "acp" => Some(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-acp-app"]),
        "web" => Some(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"]),
        "headless" => Some(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-headless"]),
        "sdk" => Some(&["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"]),
        "sdk-minimal" => Some(&["@deepseek-ai/dsh-sdk-minimal"]),
        _ => None,
    };
    if !dir.join("package.json").exists() {
        let template=template.ok_or_else(||LoadError::Runtime(format!("{bin_name}: profile {} does not exist; create it with 'dsh plugin --profile {name} add <package>'",hoh_util_values::js_json_stringify(&json!(name)))))?;
        init_profile(
            &dir,
            &template
                .iter()
                .map(|name| (*name).into())
                .collect::<Vec<_>>(),
        )?;
    }
    let manifest = read_profile_manifest(bin_name, &dir)?;
    if name == "headless"
        && manifest.pointer("/dsh/profile/bundles")
            == Some(&json!([
                "@deepseek-ai/dsh-base",
                "@deepseek-ai/dsh-web-app",
                "@deepseek-ai/dsh-headless"
            ]))
    {
        let updated = with_bundles(
            &manifest,
            &[
                json!("@deepseek-ai/dsh-base"),
                json!("@deepseek-ai/dsh-headless"),
            ],
        )?;
        write_profile_manifest(&dir, &updated)?;
    }
    Profile::load(&dir, bundles, &home, overlays, env)
}
