//! Launch-time paths and checked .env layers; all ambient facts are explicit inputs.
use hoh_launch_environment::{
    LaunchEnvironmentLayerInput, LaunchEnvironmentSnapshot, LaunchEnvironmentSource,
    create_launch_environment_snapshot,
};
use hoh_util_values::js_trim;
#[cfg(not(windows))]
use hoh_util_workspace_path::node_path;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};

pub fn resolve_config_path(config_path: &str, snapshot_mode: Option<&str>, cwd: &str) -> PathBuf {
    let absolute = resolve_native(cwd, &[config_path]);
    if snapshot_mode != Some("replay") {
        return absolute.into();
    }
    #[cfg(not(windows))]
    let name = node_path::basename(&absolute);
    #[cfg(windows)]
    let name = Path::new(&absolute)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let changed = if let Some(prefix) = name
        .strip_suffix("cordis.yaml")
        .or_else(|| name.strip_suffix("cordis.yml"))
    {
        format!("{prefix}cordis.snapshot.yml")
    } else {
        name
    };
    #[cfg(not(windows))]
    let parent = node_path::dirname(&absolute);
    #[cfg(windows)]
    let parent = Path::new(&absolute)
        .parent()
        .unwrap_or(Path::new(&absolute))
        .to_string_lossy()
        .into_owned();
    resolve_native(cwd, &[&parent, &changed]).into()
}
fn resolve_native(cwd: &str, parts: &[&str]) -> String {
    #[cfg(not(windows))]
    {
        node_path::resolve(cwd, parts)
    }
    #[cfg(windows)]
    {
        let mut env = crate::EvalEnv {
            home: PathBuf::new(),
            cwd: cwd.into(),
            exe: PathBuf::new(),
            platform: "win32",
            var: std::sync::Arc::new(|_| None),
        };
        for part in parts {
            env.cwd = crate::expr::resolve_path(part, &env).into();
        }
        crate::expr::resolve_path("", &env)
    }
}
// Node's dotenv parser treats quotes as literal delimiters, permits multiline quoted values,
// and expands only backslash-n in double-quoted values. No shell expansion is performed.
fn parse_env(content: &str) -> IndexMap<String, String> {
    let text = content.replace("\r\n", "\n");
    let mut rest = text.trim_start_matches('\u{feff}');
    let mut values = IndexMap::new();
    while !rest.is_empty() {
        rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        if rest.starts_with('#') {
            rest = rest.split_once('\n').map_or("", |(_, r)| r);
            continue;
        }
        let line_end = rest.find('\n').unwrap_or(rest.len());
        let Some(equal) = rest[..line_end].find('=') else {
            rest = rest.get(line_end + 1..).unwrap_or("");
            continue;
        };
        let key = js_trim(&rest[..equal]);
        let key = key.strip_prefix("export ").map(js_trim).unwrap_or(key);
        if key.is_empty() {
            rest = rest.get(line_end + 1..).unwrap_or("");
            continue;
        }
        let after = rest[equal + 1..].trim_start_matches([' ', '\t']);
        let (value, remaining) = if let Some(quote) = after
            .chars()
            .next()
            .filter(|c| matches!(c, '\'' | '"' | '`'))
        {
            let body = &after[quote.len_utf8()..];
            if let Some(end) = body.find(quote) {
                let raw = &body[..end];
                let value = if quote == '"' {
                    raw.replace("\\n", "\n")
                } else {
                    raw.into()
                };
                let remaining = body[end + quote.len_utf8()..]
                    .split_once('\n')
                    .map_or("", |(_, r)| r);
                (value, remaining)
            } else {
                (js_trim(after.split('#').next().unwrap_or("")).into(), "")
            }
        } else {
            let end = after.find('\n').unwrap_or(after.len());
            let value = js_trim(after[..end].split('#').next().unwrap_or("")).into();
            (value, after.get(end + 1..).unwrap_or(""))
        };
        values.insert(key.into(), value);
        rest = remaining;
    }
    values
}
fn read_env(
    bin: &str,
    dir: &Path,
    warn: &mut dyn FnMut(String),
) -> Option<(String, IndexMap<String, String>)> {
    let path = dir.join(".env");
    match std::fs::read_to_string(&path) {
        Ok(text) => Some((path.to_string_lossy().into_owned(), parse_env(&text))),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                let (syscall, name) = if error.kind() == std::io::ErrorKind::IsADirectory {
                    ("read", None)
                } else {
                    ("open", path.to_str())
                };
                warn(format!(
                    "{bin}: failed to load .env: Error: {}\n",
                    hoh_error::node_fs_error_message(&error, syscall, name, None)
                ));
            }
            None
        }
    }
}
fn materialize(environment: &mut IndexMap<String, String>, values: &IndexMap<String, String>) {
    for (name, value) in values {
        #[cfg(windows)]
        let present = environment.keys().any(|key| key.eq_ignore_ascii_case(name));
        #[cfg(not(windows))]
        let present = environment.contains_key(name);
        if !present {
            environment.insert(name.clone(), value.clone());
        }
    }
}
/// Node loadEnvFile semantics over a caller-owned process map.
pub fn load_env(
    bin: &str,
    dir: &Path,
    environment: &mut IndexMap<String, String>,
    warn: &mut dyn FnMut(String),
) {
    if let Some((_, values)) = read_env(bin, dir, warn) {
        materialize(environment, &values);
    }
}
const BOOTSTRAP_NAMES: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "SHELL",
    "NODE_OPTIONS",
    "NODE_PATH",
    "NODE_EXTRA_CA_CERTS",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "PERL5OPT",
    "PERL5LIB",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "RUBYOPT",
    "RUBYLIB",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "PYTHONHOME",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "GIT_EDITOR",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_COUNT",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "BROWSER",
    "DEEPSEEK_BASE_URL",
    "DEEPSEEK_SEARCH_BASE_URL",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_TLS_REJECT_UNAUTHORIZED",
];
const BOOTSTRAP_PREFIXES: &[&str] = &["DSH_", "XDG_", "DYLD_", "BASH_FUNC_"];
const ENDPOINTS: &[&str] = &["DEEPSEEK_BASE_URL", "DEEPSEEK_SEARCH_BASE_URL"];
fn check_layer(
    bin: &str,
    path: &str,
    values: &IndexMap<String, String>,
    is_home: bool,
    home: &Path,
) -> Result<(), crate::LoadError> {
    for name in values.keys() {
        let upper = name.to_uppercase();
        if !BOOTSTRAP_NAMES.contains(&upper.as_str())
            && !BOOTSTRAP_PREFIXES.iter().any(|p| upper.starts_with(p))
        {
            continue;
        }
        let home_name = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
            .contains(&upper.as_str())
            || ENDPOINTS.contains(&name.as_str());
        if is_home && home_name {
            continue;
        }
        let home_env = home.join(".env");
        let remedy = if home_name {
            format!(
                "export {name}, or put it in {}, which does not travel with a repository",
                home_env.display()
            )
        } else if ENDPOINTS.contains(&upper.as_str()) {
            format!(
                "spell it {upper}, then export it or put it in {}, which does not travel with a repository",
                home_env.display()
            )
        } else {
            format!("export {name} instead of putting it in a .env file")
        };
        return Err(crate::LoadError::Runtime(format!(
            "{bin}: {path} sets \"{name}\", which only the launching environment may set (it decides how this process starts, where its code and instructions load from, or how it reaches the network); {remedy}"
        )));
    }
    Ok(())
}
/// Validate both files before materializing either; home is resolved from inherited facts by the host.
pub fn load_layered_env(
    bin: &str,
    cwd: &Path,
    home: &Path,
    environment: &mut IndexMap<String, String>,
    warn: &mut dyn FnMut(String),
) -> Result<LaunchEnvironmentSnapshot, crate::LoadError> {
    let home = PathBuf::from(resolve_native(
        &cwd.to_string_lossy(),
        &[&home.to_string_lossy()],
    ));
    let cwd = PathBuf::from(resolve_native(&cwd.to_string_lossy(), &[]));
    let inherited = LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::Process,
        path: None,
        values: environment.clone(),
    };
    let project = read_env(bin, &cwd, warn);
    if let Some((path, values)) = &project {
        check_layer(bin, path, values, cwd == home, &home)?;
    }
    let user = if home == cwd {
        None
    } else {
        read_env(bin, &home, warn)
    };
    if let Some((path, values)) = &user {
        check_layer(bin, path, values, true, &home)?;
    }
    let mut layers = vec![inherited];
    for (layer, source) in [
        (project, LaunchEnvironmentSource::ProjectEnv),
        (user, LaunchEnvironmentSource::UserEnv),
    ] {
        if let Some((path, values)) = layer {
            materialize(environment, &values);
            layers.push(LaunchEnvironmentLayerInput {
                source,
                path: Some(path),
                values,
            });
        }
    }
    Ok(create_launch_environment_snapshot(&layers))
}
