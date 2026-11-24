//! A per-session Claude settings overlay that keeps channel plugins out of the HoH window.
//!
//! Claude Code records whether a plugin is enabled in user/project/local `settings*.json` and
//! records the installed copy in `plugins/installed_plugins.json`.  A channel plugin's MCP server
//! advertises the experimental `claude/channel` capability in its source or built JavaScript.
//! HoH only reads those files: it never changes Claude's configuration or plugin installation.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

const CHANNEL_CAPABILITY: &[u8] = b"claude/channel";
const KNOWN_CHANNEL_PLUGINS: &[&str] = &["discord", "fakechat", "imessage", "telegram"];
const MAX_SCAN_DEPTH: usize = 16;
const MAX_SCAN_ENTRIES: usize = 20_000;
const MAX_SCAN_FILES: usize = 10_000;
const MAX_SCAN_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

/// Add the narrow command-line settings overlay for a personal-settings session, if any enabled
/// installed plugin is a channel. Missing and malformed metadata is deliberately non-fatal.
pub(crate) fn append_disabled_channels(
    args: &mut Vec<String>,
    extra_env: &[(String, String)],
    cwd: &Path,
) {
    let Some(settings) = disabled_channel_settings(extra_env, cwd) else {
        return;
    };
    args.extend(["--settings".to_owned(), settings]);
}

fn disabled_channel_settings(extra_env: &[(String, String)], cwd: &Path) -> Option<String> {
    let home = env_path(extra_env, "HOME");
    let config_dir = env_path(extra_env, "CLAUDE_CONFIG_DIR")
        .or_else(|| home.as_ref().map(|home| home.join(".claude")))?;
    let effective = enabled_plugins(&config_dir, cwd);
    let registry = read_json(&config_dir.join("plugins/installed_plugins.json"));
    let mut candidates = effective.ids;
    if effective.uncertain {
        candidates.extend(installed_plugin_ids(registry.as_ref()));
        candidates.extend(cached_plugin_ids(&config_dir));
    }
    if candidates.is_empty() {
        return None;
    }

    let disabled: Map<String, Value> = candidates
        .into_iter()
        .filter(|id| {
            let roots = plugin_roots(registry.as_ref(), &config_dir, home.as_deref(), id);
            is_known_channel(id) || roots.iter().any(|root| is_channel_root(root))
        })
        .map(|id| (id, Value::Bool(false)))
        .collect();
    if disabled.is_empty() {
        return None;
    }
    serde_json::to_string(&serde_json::json!({ "enabledPlugins": disabled })).ok()
}

fn env_path(extra_env: &[(String, String)], name: &str) -> Option<PathBuf> {
    extra_env
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| std::env::var_os(name).map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty())
}

/// Resolve the same precedence relevant to plugin enablement: user, then project, then local.
/// The window starts Claude in a project root; walking upwards also covers callers that pass a
/// subdirectory without requiring Git or invoking another process.
struct EffectivePlugins {
    ids: BTreeSet<String>,
    uncertain: bool,
}

fn enabled_plugins(config_dir: &Path, cwd: &Path) -> EffectivePlugins {
    let mut effective = BTreeMap::<String, bool>::new();
    let mut uncertain = merge_enabled(&config_dir.join("settings.json"), &mut effective);
    if let Some(project) = project_settings_dir(cwd) {
        uncertain |= merge_enabled(&project.join(".claude/settings.json"), &mut effective);
        uncertain |= merge_enabled(&project.join(".claude/settings.local.json"), &mut effective);
    }
    EffectivePlugins {
        ids: effective
            .into_iter()
            .filter_map(|(id, enabled)| enabled.then_some(id))
            .collect(),
        uncertain,
    }
}

fn project_settings_dir(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| {
            let settings = dir.join(".claude/settings.json");
            let local = dir.join(".claude/settings.local.json");
            settings.is_file() || local.is_file()
        })
        .map(Path::to_path_buf)
}

/// Returns true when a present settings file could not be interpreted. In that case discovery
/// widens to installed plugin IDs and still disables anything statically recognizable as a
/// channel; a parser mismatch must not be the reason a bot-stealing server starts.
fn merge_enabled(path: &Path, effective: &mut BTreeMap<String, bool>) -> bool {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let Ok(settings) = serde_json::from_slice::<Value>(&bytes) else {
        return true;
    };
    let Some(plugins) = settings.get("enabledPlugins").and_then(Value::as_object) else {
        return settings.get("enabledPlugins").is_some();
    };
    let mut uncertain = false;
    for (id, enabled) in plugins {
        if let Some(enabled) = enabled.as_bool() {
            effective.insert(id.clone(), enabled);
        } else {
            // Preserve the ID as a conservative candidate while recording the schema mismatch.
            effective.insert(id.clone(), true);
            uncertain = true;
        }
    }
    uncertain
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn plugin_roots(
    registry: Option<&Value>,
    config_dir: &Path,
    home: Option<&Path>,
    id: &str,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let entries = registry
        .and_then(|value| value.get("plugins").or(Some(value)))
        .and_then(Value::as_object)
        .and_then(|plugins| plugins.get(id));
    collect_install_paths(entries, config_dir, home, &mut roots);

    // A damaged registry must not re-enable a conventional cached channel plugin. This path is
    // Claude Code's cache layout; it is only a fallback and does not replace registry entries.
    if let Some((plugin, marketplace)) = cache_coordinates(id) {
        let cache = config_dir
            .join("plugins/cache")
            .join(marketplace)
            .join(plugin);
        if cache.is_dir() {
            roots.push(cache);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn cache_coordinates(id: &str) -> Option<(&str, &str)> {
    let (plugin, marketplace) = id.split_once('@')?;
    if id[plugin.len() + 1..].contains('@')
        || !safe_cache_component(plugin)
        || !safe_cache_component(marketplace)
    {
        return None;
    }
    Some((plugin, marketplace))
}

fn safe_cache_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

fn installed_plugin_ids(registry: Option<&Value>) -> BTreeSet<String> {
    registry
        .and_then(|value| value.get("plugins").or(Some(value)))
        .and_then(Value::as_object)
        .map(|plugins| plugins.keys().cloned().collect())
        .unwrap_or_default()
}

fn cached_plugin_ids(config_dir: &Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut entries_seen = 0_usize;
    let Ok(markets) = fs::read_dir(config_dir.join("plugins/cache")) else {
        return ids;
    };
    'markets: for market in markets.flatten() {
        entries_seen += 1;
        if entries_seen > MAX_SCAN_ENTRIES {
            break;
        }
        if !market.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let marketplace = market.file_name().to_string_lossy().into_owned();
        let Ok(plugins) = fs::read_dir(market.path()) else {
            continue;
        };
        for plugin in plugins.flatten() {
            entries_seen += 1;
            if entries_seen > MAX_SCAN_ENTRIES {
                break 'markets;
            }
            if plugin.file_type().is_ok_and(|kind| kind.is_dir()) {
                ids.insert(format!(
                    "{}@{marketplace}",
                    plugin.file_name().to_string_lossy()
                ));
            }
        }
    }
    ids
}

fn collect_install_paths(
    value: Option<&Value>,
    config_dir: &Path,
    home: Option<&Path>,
    roots: &mut Vec<PathBuf>,
) {
    match value {
        Some(Value::Array(entries)) => {
            for entry in entries {
                collect_install_paths(Some(entry), config_dir, home, roots);
            }
        }
        Some(Value::Object(entry)) => {
            if let Some(path) = entry.get("installPath").and_then(Value::as_str) {
                roots.push(resolve_install_path(path, config_dir, home));
            }
        }
        Some(Value::String(path)) => roots.push(resolve_install_path(path, config_dir, home)),
        _ => {}
    }
}

fn resolve_install_path(path: &str, config_dir: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        config_dir.join(path)
    }
}

fn is_known_channel(id: &str) -> bool {
    let name = id.split_once('@').map_or(id, |(name, _)| name);
    KNOWN_CHANNEL_PLUGINS
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known))
}

fn is_channel_root(root: &Path) -> bool {
    manifest_marks_channel(root) || (declares_mcp_server(root) && source_tree_has_capability(root))
}

/// The capability belongs to an MCP server, not merely to an arbitrary example or test file in
/// a plugin. Require a non-empty server declaration before accepting a source-tree marker.
fn declares_mcp_server(root: &Path) -> bool {
    mcp_file_declares_server(&root.join(".mcp.json"))
        || manifest_declares_server(&root.join(".claude-plugin/plugin.json"))
        || fs::read_dir(root).ok().is_some_and(|entries| {
            entries.flatten().any(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && (mcp_file_declares_server(&entry.path().join(".mcp.json"))
                        || manifest_declares_server(
                            &entry.path().join(".claude-plugin/plugin.json"),
                        ))
            })
        })
}

fn mcp_file_declares_server(path: &Path) -> bool {
    declaration_file(path, |config| {
        config
            .get("mcpServers")
            .map_or_else(|| non_empty_declaration(config), non_empty_declaration)
    })
}

fn manifest_declares_server(path: &Path) -> bool {
    declaration_file(path, |manifest| {
        manifest
            .get("mcpServers")
            .is_some_and(non_empty_declaration)
    })
}

fn declaration_file(path: &Path, inspect: impl FnOnce(&Value) -> bool) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => inspect(&value),
        // A present but unparsable declaration file plus the capability marker is uncertain in
        // exactly the dangerous direction, so treat it as a declaration without aborting launch.
        Err(_) => !bytes.is_empty(),
    }
}

fn non_empty_declaration(value: &Value) -> bool {
    match value {
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::String(value) => !value.trim().is_empty(),
        _ => false,
    }
}

fn manifest_marks_channel(root: &Path) -> bool {
    manifest_has_channel_keyword(&root.join(".claude-plugin/plugin.json"))
        || fs::read_dir(root).ok().is_some_and(|entries| {
            entries.flatten().any(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && manifest_has_channel_keyword(
                        &entry.path().join(".claude-plugin/plugin.json"),
                    )
            })
        })
}

fn manifest_has_channel_keyword(path: &Path) -> bool {
    read_json(path)
        .and_then(|manifest| manifest.get("keywords").cloned())
        .and_then(|keywords| keywords.as_array().cloned())
        .is_some_and(|keywords| {
            keywords
                .iter()
                .filter_map(Value::as_str)
                .any(|keyword| keyword.eq_ignore_ascii_case("channel"))
        })
}

fn source_tree_has_capability(root: &Path) -> bool {
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut entries_seen = 0_usize;
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some((dir, depth)) = pending.pop() {
        if depth > MAX_SCAN_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            entries_seen += 1;
            if entries_seen > MAX_SCAN_ENTRIES {
                // This function is only called after an MCP declaration was found. If its tree
                // is too wide to classify safely, disable it rather than starting an uncertain
                // channel server or blocking the window with an unbounded scan.
                return true;
            }
            let name = entry.file_name();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if name != OsStr::new(".git") && name != OsStr::new("node_modules") {
                    pending.push((entry.path(), depth + 1));
                }
                continue;
            }
            if !kind.is_file() || !plausible_source(&entry.path()) || files >= MAX_SCAN_FILES {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let len = metadata.len();
            if len > MAX_SOURCE_BYTES || bytes.saturating_add(len) > MAX_SCAN_BYTES {
                continue;
            }
            files += 1;
            bytes += len;
            if file_contains(&entry.path(), CHANNEL_CAPABILITY) {
                return true;
            }
        }
    }
    false
}

fn plausible_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("cjs" | "cts" | "js" | "json" | "mjs" | "mts" | "ts" | "tsx")
    )
}

fn file_contains(path: &Path, needle: &[u8]) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut buffer = [0_u8; 64 * 1024];
    let mut carry = Vec::new();
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            return false;
        }
        carry.extend_from_slice(&buffer[..read]);
        if carry.windows(needle.len()).any(|window| window == needle) {
            return true;
        }
        if carry.len() >= needle.len() {
            carry.drain(..carry.len() - (needle.len() - 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        (dir, config, project)
    }

    #[test]
    fn capability_marker_disables_only_the_enabled_channel_plugin() {
        let (_dir, config, project) = fixture();
        let channel = config.join("plugins/cache/test/channel/1");
        let ordinary = config.join("plugins/cache/test/ordinary/1");
        write(
            &config.join("settings.json"),
            r#"{"enabledPlugins":{"channel@test":true,"ordinary@test":true,"off@test":false}}"#,
        );
        write(
            &config.join("plugins/installed_plugins.json"),
            &serde_json::json!({
                "version": 2,
                "plugins": {
                    "channel@test": [{"installPath": channel}],
                    "ordinary@test": [{"installPath": ordinary}]
                }
            })
            .to_string(),
        );
        write(
            &channel.join("src/server.ts"),
            "experimental: { 'claude/channel': {} }",
        );
        write(
            &channel.join(".mcp.json"),
            r#"{"mcpServers":{"channel":{"command":"node","args":["src/server.ts"]}}}"#,
        );
        write(&ordinary.join("dist/server.js"), "experimental: {};");

        let settings = disabled_channel_settings(
            &[(
                "CLAUDE_CONFIG_DIR".into(),
                config.to_string_lossy().into_owned(),
            )],
            &project,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&settings).unwrap(),
            serde_json::json!({"enabledPlugins":{"channel@test":false}})
        );
    }

    #[test]
    fn unrelated_capability_example_without_mcp_declaration_is_not_a_channel() {
        let (_dir, config, project) = fixture();
        let ordinary = config.join("plugins/cache/test/ordinary/1");
        write(
            &config.join("settings.json"),
            r#"{"enabledPlugins":{"ordinary@test":true}}"#,
        );
        write(
            &config.join("plugins/installed_plugins.json"),
            &serde_json::json!({
                "plugins": {"ordinary@test": [{"installPath": ordinary}]}
            })
            .to_string(),
        );
        write(
            &ordinary.join("examples/channel-response.json"),
            r#"{"experimental":{"claude/channel":{}}}"#,
        );

        assert_eq!(
            disabled_channel_settings(
                &[(
                    "CLAUDE_CONFIG_DIR".into(),
                    config.to_string_lossy().into_owned(),
                )],
                &project,
            ),
            None
        );
    }

    #[test]
    fn cache_fallback_rejects_traversal_shaped_plugin_ids() {
        assert_eq!(cache_coordinates("home@../../../../.."), None);
        assert_eq!(cache_coordinates("../home@test"), None);
        assert_eq!(cache_coordinates("home@test/../../"), None);
        assert_eq!(cache_coordinates("home@test@other"), None);
        assert_eq!(cache_coordinates("home@test"), Some(("home", "test")));
    }

    #[test]
    fn local_precedence_keyword_and_known_names_fail_closed() {
        let (_dir, config, project) = fixture();
        let keyword = config.join("plugins/cache/test/custom/1");
        write(
            &config.join("settings.json"),
            r#"{"enabledPlugins":{"custom@test":true,"telegram@test":true,"ordinary@test":true}}"#,
        );
        write(
            &project.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"ordinary@test":false}}"#,
        );
        write(
            &project.join(".claude/settings.local.json"),
            r#"{"enabledPlugins":{"ordinary@test":true}}"#,
        );
        write(
            &config.join("plugins/installed_plugins.json"),
            "{ damaged registry",
        );
        write(
            &keyword.join(".claude-plugin/plugin.json"),
            r#"{"name":"custom","keywords":["Channel"]}"#,
        );
        write(
            &config.join("plugins/cache/test/ordinary/1/server.ts"),
            "ok",
        );

        let settings = disabled_channel_settings(
            &[(
                "CLAUDE_CONFIG_DIR".into(),
                config.to_string_lossy().into_owned(),
            )],
            &project,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&settings).unwrap(),
            serde_json::json!({
                "enabledPlugins": {"custom@test": false, "telegram@test": false}
            })
        );
    }

    #[test]
    fn malformed_or_missing_files_never_prevent_launch() {
        let (_dir, config, project) = fixture();
        let channel = config.join("plugins/cache/test/custom/1");
        write(&config.join("settings.json"), "not json");
        write(
            &config.join("plugins/installed_plugins.json"),
            "damaged registry",
        );
        write(
            &channel.join(".mcp.json"),
            r#"{"mcpServers":{"custom":{"command":"node","args":["server.ts"]}}}"#,
        );
        write(
            &channel.join("server.ts"),
            "experimental: { 'claude/channel': {} }",
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                &disabled_channel_settings(
                    &[(
                        "CLAUDE_CONFIG_DIR".into(),
                        config.to_string_lossy().into_owned(),
                    )],
                    &project,
                )
                .unwrap(),
            )
            .unwrap(),
            serde_json::json!({"enabledPlugins":{"custom@test":false}})
        );
        assert_eq!(
            disabled_channel_settings(
                &[(
                    "CLAUDE_CONFIG_DIR".into(),
                    config.join("missing").to_string_lossy().into_owned(),
                )],
                &project,
            ),
            None
        );
    }
}
