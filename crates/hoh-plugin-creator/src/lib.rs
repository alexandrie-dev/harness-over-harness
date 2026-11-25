//! Built-in authoring support for the in-tree Rust plugin workflow.

use hoh_kernel::{Cfg, Context, Error, NoConfig, Plugin, service};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const MANIFEST: &str = "hoh-plugin.json";
const FILES: [&str; 4] = ["Cargo.toml", "src/lib.rs", MANIFEST, "tests/smoke.rs"];
const INDEX_COMMAND: &str = "python3 scripts/generate-plugin-index.py";

pub struct PluginCreator;
service!(pub Creator: PluginCreator = "pluginCreator");

impl Plugin for PluginCreator {
    const NAME: &'static str = "hoh:core-plugin-creator";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        ctx.provide::<Creator>(Arc::new(Self)).await?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Template {
    #[default]
    Basic,
    Tool,
}

impl Template {
    pub const ALL: [Self; 2] = [Self::Basic, Self::Tool];

    pub fn name(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Tool => "tool",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Basic => "Minimal Rust plugin with a kernel mount test.",
            Self::Tool => "Pure Rust echo tool with a real tool call and unload test.",
        }
    }

    pub fn guide(self) -> &'static str {
        match self {
            Self::Basic => {
                "Edit src/lib.rs. Add typed Service<T> fields to an Inject struct when the plugin needs kernel services. Mount those services before AuthorPlugin in tests/smoke.rs."
            }
            Self::Tool => {
                "Edit the parameter schema, handler and result schema in src/lib.rs together. Keep the tool name in hoh-plugin.json in sync. Extend tests/smoke.rs with success and invalid-input cases. The supplied fixture uses the native HoH tool pipeline."
            }
        }
    }
}

impl std::str::FromStr for Template {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|template| template.name() == name)
            .ok_or_else(|| format!("unknown template {name:?}. Run hoh plugin templates").into())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scaffold {
    pub id: String,
    pub template: Template,
    pub path: PathBuf,
    pub files: [&'static str; 4],
    pub test_command: String,
    pub index_command: &'static str,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkedManifest {
    Missing,
    Matches,
    Differs,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub id: String,
    pub path: PathBuf,
    pub issues: Vec<Issue>,
    pub linked_manifest: LinkedManifest,
    pub test_command: String,
    pub index_command: &'static str,
}

#[derive(Serialize)]
pub struct Issue {
    pub file: &'static str,
    pub message: String,
}

impl PluginCreator {
    pub fn create(&self, cwd: &Path, id: &str, template: Template) -> Result<Scaffold, Error> {
        validate_id(id)?;
        let tool = template == Template::Tool;
        if tool && id.len() > 59 {
            return Err(
                "the tool template needs an id of at most 59 characters to fit the tool name"
                    .into(),
            );
        }
        let parent = repository(cwd)?.join("plugins");
        create_plugins_dir(&parent)?;
        let path = parent.join(id);
        fs::create_dir(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                format!("{} already exists. Choose a new plugin id", path.display())
            } else {
                format!("cannot create {}: {error}", path.display())
            }
        })?;

        let dependencies = if tool {
            include_str!("../templates/tool-dependencies.toml")
        } else {
            include_str!("../templates/basic-dependencies.toml")
        };
        let cargo = include_str!("../templates/Cargo.toml")
            .replace("@ID@", id)
            .replace("@DEPENDENCIES@", dependencies);
        let tool_name = format!("{}_echo", id.replace('-', "_"));
        let source = if tool {
            include_str!("../templates/tool.rs")
        } else {
            include_str!("../templates/basic.rs")
        };
        let smoke = if tool {
            include_str!("../templates/tool-smoke.rs")
        } else {
            include_str!("../templates/basic-smoke.rs")
        };
        let render = |text: &str| {
            text.replace("@ID@", id)
                .replace("@CRATE@", &format!("hoh_plugin_{}", id.replace('-', "_")))
                .replace("@TOOL@", &tool_name)
        };
        let manifest = serde_json::json!({
            "id": id,
            "name": id,
            "version": "0.1.0",
            "entry": "AuthorPlugin",
            "description": template.description(),
            "permissions": if tool { serde_json::json!({"tools": [tool_name]}) } else { serde_json::json!({}) }
        });

        let mut created_files = Vec::new();
        let mut created_dirs = Vec::new();
        let result = (|| -> Result<(), Error> {
            for directory in ["src", "tests"] {
                let directory = path.join(directory);
                fs::create_dir(&directory)?;
                created_dirs.push(directory);
            }
            let contents = [
                cargo,
                render(source),
                format!("{}\n", serde_json::to_string_pretty(&manifest)?),
                render(smoke),
            ];
            for (name, contents) in FILES.into_iter().zip(contents) {
                let file = path.join(name);
                let mut writer = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&file)?;
                created_files.push(file);
                writer.write_all(contents.as_bytes())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for file in created_files {
                let _ = fs::remove_file(file);
            }
            for directory in created_dirs {
                let _ = fs::remove_dir(directory);
            }
            let _ = fs::remove_dir(&path);
            return Err(format!("could not write {}: {error}", path.display()).into());
        }
        Ok(Scaffold {
            id: id.into(),
            template,
            path,
            files: FILES,
            test_command: test_command(id),
            index_command: INDEX_COMMAND,
        })
    }

    /// Check source metadata and compare the manifest to the one embedded in this engine.
    pub fn inspect(
        &self,
        cwd: &Path,
        id: &str,
        linked: &[(&str, &str)],
    ) -> Result<Inspection, Error> {
        validate_id(id)?;
        let parent = repository(cwd)?.join("plugins");
        require_directory(&parent)?;
        let path = parent.join(id);
        require_directory(&path)?;
        let mut issues = Vec::new();
        if !path.join("Cargo.toml").is_file() {
            issues.push(Issue {
                file: "Cargo.toml",
                message: format!("add the hoh-plugin-{id} Cargo package"),
            });
        }
        let manifest = match read_manifest(&path.join(MANIFEST)) {
            Ok(text) => {
                match serde_json::from_str::<Value>(&text) {
                    Ok(manifest) => check_manifest(&manifest, id, &mut issues),
                    Err(error) => issues.push(Issue {
                        file: MANIFEST,
                        message: format!("invalid JSON: {error}"),
                    }),
                }
                Some(text)
            }
            Err(error) => {
                issues.push(Issue {
                    file: MANIFEST,
                    message: error.to_string(),
                });
                None
            }
        };
        let linked_manifest = match linked.iter().find(|(name, _)| *name == id) {
            None => LinkedManifest::Missing,
            Some((_, text)) if manifest.as_deref() == Some(*text) => LinkedManifest::Matches,
            Some(_) => LinkedManifest::Differs,
        };
        Ok(Inspection {
            id: id.into(),
            path,
            issues,
            linked_manifest,
            test_command: test_command(id),
            index_command: INDEX_COMMAND,
        })
    }
}

fn repository(cwd: &Path) -> Result<&Path, Error> {
    cwd.ancestors()
        .find(|root| root.join("crates/hoh-kernel/Cargo.toml").is_file())
        .ok_or_else(|| "run this command inside a HoH source checkout".into())
}

fn validate_id(id: &str) -> Result<(), Error> {
    let bytes = id.as_bytes();
    if bytes.len() <= 64
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    {
        return Ok(());
    }
    Err("plugin id must start with a lowercase letter, end with a letter or digit, and contain only lowercase letters, digits and hyphens (maximum 64 characters)".into())
}

fn create_plugins_dir(path: &Path) -> Result<(), Error> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => require_directory(path),
        Err(error) => Err(format!("cannot create {}: {error}", path.display()).into()),
    }
}

fn require_directory(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "expected a directory without a symlink at {}",
            path.display()
        )
        .into()),
        Err(error) => Err(format!("cannot open {}: {error}", path.display()).into()),
    }
}

fn read_manifest(path: &Path) -> Result<String, Error> {
    const MAX_BYTES: u64 = 256 * 1024;
    if !fs::symlink_metadata(path)?.is_file() {
        return Err("expected a regular manifest file without a symlink".into());
    }
    let mut text = String::new();
    let size = fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)?;
    if size as u64 > MAX_BYTES {
        return Err("manifest exceeds 256 KiB".into());
    }
    Ok(text)
}

fn check_manifest(manifest: &Value, id: &str, issues: &mut Vec<Issue>) {
    let mut issue = |message: &str| {
        issues.push(Issue {
            file: MANIFEST,
            message: message.into(),
        });
    };
    if !manifest.is_object() {
        issue("manifest must be a JSON object");
        return;
    }
    if manifest["id"].as_str() != Some(id) {
        issue("id must match the plugin directory");
    }
    if !manifest["version"]
        .as_str()
        .is_some_and(|value| !value.trim().is_empty())
    {
        issue("version must be a nonempty string");
    }
    if !manifest["entry"].as_str().is_some_and(is_rust_identifier) {
        issue("entry must name a Rust type exported by the crate");
    }
    if manifest
        .get("permissions")
        .is_some_and(|value| !value.is_object())
    {
        issue("permissions must be an object");
    }
}

fn is_rust_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn test_command(id: &str) -> String {
    format!("cargo test --offline -p hoh-plugin-{id}")
}
