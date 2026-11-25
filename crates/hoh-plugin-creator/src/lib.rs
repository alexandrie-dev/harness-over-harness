//! Built-in authoring support for the in-tree Rust plugin workflow.

mod test;
mod validation;

pub use test::TestReport;
pub use validation::{FORMAT_VERSION, Issue};

use hoh_kernel::{Cfg, Context, Error, NoConfig, Plugin, service};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

const MANIFEST: &str = "hoh-plugin.json";
const FILES: [&str; 4] = ["Cargo.toml", "src/lib.rs", MANIFEST, "tests/smoke.rs"];

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
    pub format_version: u32,
    pub ok: bool,
    pub id: String,
    pub template: Template,
    pub path: PathBuf,
    pub files: [&'static str; 4],
    pub edit_files: [&'static str; 2],
    pub test_command: String,
    pub index_command: String,
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
    pub format_version: u32,
    pub ok: bool,
    pub id: String,
    pub path: PathBuf,
    pub issues: Vec<Issue>,
    pub linked_manifest: LinkedManifest,
    pub test_command: String,
    pub index_command: String,
}

impl PluginCreator {
    pub fn create(&self, cwd: &Path, id: &str, template: Template) -> Result<Scaffold, Issue> {
        let root = repository(cwd)?;
        if let Some(issue) = validation::preflight(root, "--check-id", id)?.issues.pop() {
            return Err(issue);
        }
        let tool = template == Template::Tool;
        if tool && id.len() > 59 {
            return Err(Issue::new(
                "tool_id_length",
                "The tool template needs an ID of at most 59 characters.",
            ));
        }
        let parent = root.join("plugins");
        create_plugins_dir(&parent)?;
        let path = parent.join(id);
        if fs::symlink_metadata(&path).is_ok() {
            return Err(already_exists(&path, id));
        }

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

        let contents = [
            cargo,
            render(source),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&manifest).expect("scaffold manifest is JSON")
            ),
            render(smoke),
        ];
        stage_scaffold(root, &path, contents).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists || fs::symlink_metadata(&path).is_ok() {
                already_exists(&path, id)
            } else {
                Issue::new("create_failed", error.to_string()).at(&path)
            }
        })?;
        Ok(Scaffold {
            format_version: FORMAT_VERSION,
            ok: true,
            id: id.into(),
            template,
            path,
            files: FILES,
            edit_files: ["src/lib.rs", MANIFEST],
            test_command: test_command(id),
            index_command: preview_command(id),
        })
    }

    /// Check source metadata and compare the manifest to the one embedded in this engine.
    pub fn inspect(
        &self,
        cwd: &Path,
        id: &str,
        linked: &[(&str, &str)],
    ) -> Result<Inspection, Issue> {
        let root = repository(cwd)?;
        let report = validation::preflight(root, "--check", id)?;
        if report
            .issues
            .iter()
            .any(|issue| matches!(issue.code.as_str(), "invalid_id" | "reserved_id"))
        {
            return Err(report.issues.into_iter().next().expect("invalid ID issue"));
        }
        let linked_manifest = match linked.iter().find(|(name, _)| *name == id) {
            None => LinkedManifest::Missing,
            Some((_, text)) if report.manifest.as_deref() == Some(*text) => LinkedManifest::Matches,
            Some(_) => LinkedManifest::Differs,
        };
        let path = root.join("plugins").join(id);
        Ok(Inspection {
            format_version: FORMAT_VERSION,
            ok: report.issues.is_empty(),
            id: id.into(),
            path,
            issues: report.issues,
            linked_manifest,
            test_command: test_command(id),
            index_command: preview_command(id),
        })
    }
}

pub(crate) fn repository(cwd: &Path) -> Result<&Path, Issue> {
    cwd.ancestors()
        .find(|root| root.join("crates/hoh-kernel/Cargo.toml").is_file())
        .ok_or_else(|| {
            Issue::new(
                "checkout_missing",
                "Run this command inside a HoH source checkout.",
            )
        })
}

fn create_plugins_dir(path: &Path) -> Result<(), Issue> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
                Ok(())
            } else {
                Err(Issue::new(
                    "plugins_directory",
                    "Expected a plugins directory without symlinks.",
                )
                .at(path))
            }
        }
        Err(error) => Err(Issue::new("create_failed", error.to_string()).at(path)),
    }
}

fn already_exists(path: &Path, id: &str) -> Issue {
    Issue::new("already_exists", "A plugin path already exists.")
        .at(path)
        .with_hint(format!(
            "Use hoh plugin check {id} to resume work, or choose a different ID."
        ))
}

fn stage_scaffold(root: &Path, destination: &Path, contents: [String; 4]) -> io::Result<()> {
    // Stage next to the workspace, outside Cargo's plugins/* glob.
    let staged = tempfile::Builder::new()
        .prefix(".hoh-scaffold-")
        .tempdir_in(root)?;
    let path = staged.path().join("plugin");
    fs::create_dir(&path)?;
    fs::create_dir(path.join("src"))?;
    fs::create_dir(path.join("tests"))?;
    for (file, contents) in FILES.into_iter().zip(contents) {
        fs::write(path.join(file), contents)?;
    }
    publish_scaffold(&path, destination)
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn publish_scaffold(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    Ok(renameat_with(
        CWD,
        source,
        CWD,
        destination,
        RenameFlags::NOREPLACE,
    )?)
}

#[cfg(windows)]
fn publish_scaffold(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
fn publish_scaffold(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "This platform does not support exclusive directory publication.",
    ))
}

fn test_command(id: &str) -> String {
    format!("hoh plugin test {id} --json")
}

fn preview_command(id: &str) -> String {
    format!("python3 scripts/generate-plugin-index.py --candidate {id} --dry-run --json")
}
