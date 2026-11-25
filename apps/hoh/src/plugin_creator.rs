//! CLI adapter for the built-in Plugin Creator service.

use crate::bin::{CliHost, CliIo};
use hoh_commander::{CommanderError, Output, ParseError};
use hoh_error::BoxError;
use hoh_kernel::Kernel;
use hoh_plugin_creator::{Creator, LinkedManifest, PluginCreator, Template};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const HELP: &str = "\nPlugin Creator:\n  hoh plugin templates [--json]\n  hoh plugin guide <basic|tool|session-panel> [--json]\n  hoh plugin new <id> [--template <basic|tool>] [--json]\n  hoh plugin check <id> [--json]\n";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum AuthorCommand {
    Templates,
    Guide { topic: String },
    New { id: String, template: Template },
    Check { id: String },
}

/// Leave the legacy profile-scoped package forwarding grammar to the launcher.
pub fn parse(
    argv: &[String],
    output: &Output,
) -> Option<Result<crate::args::DshInvocation, ParseError>> {
    use crate::args::DshInvocation;

    let [plugin, verb, tail @ ..] = argv else {
        return None;
    };
    if plugin != "plugin" || !matches!(verb.as_str(), "templates" | "guide" | "new" | "check") {
        return None;
    }
    if matches!(tail, [flag] if flag == "--help" || flag == "-h") {
        (output.write_out)(HELP);
        return Some(Err(
            CommanderError::new(0, "commander.helpDisplayed", "").into()
        ));
    }
    if let ("new", [id]) = (verb.as_str(), tail)
        && !id.starts_with('-')
    {
        return Some(Ok(DshInvocation::PluginNew { id: id.clone() }));
    }
    let (args, json) = match tail.split_last() {
        Some((flag, args)) if flag == "--json" => (args, true),
        _ => (tail, false),
    };
    let command = match (verb.as_str(), args) {
        ("templates", []) => AuthorCommand::Templates,
        ("guide", [topic]) if !topic.starts_with('-') => AuthorCommand::Guide {
            topic: topic.clone(),
        },
        ("new", [id]) if !id.starts_with('-') => AuthorCommand::New {
            id: id.clone(),
            template: Template::default(),
        },
        ("new", [id, flag, template]) if !id.starts_with('-') && flag == "--template" => {
            match template.parse() {
                Ok(template) => AuthorCommand::New {
                    id: id.clone(),
                    template,
                },
                Err(error) => return Some(Err(ParseError::Other(error))),
            }
        }
        ("check", [id]) if !id.starts_with('-') => AuthorCommand::Check { id: id.clone() },
        _ => {
            return Some(Err(ParseError::Other(
                format!("Invalid plugin command. {HELP}").into(),
            )));
        }
    };
    Some(Ok(DshInvocation::PluginAuthor { command, json }))
}

pub async fn run(command: AuthorCommand, json: bool, host: &CliHost) -> Result<i32, BoxError> {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let result = async {
        ctx.plugin::<PluginCreator>(Value::Null)?.settled().await?;
        let creator = ctx.get::<Creator>().ok_or("Plugin Creator service is unavailable")?;
        match command {
            AuthorCommand::Templates => {
                let text = Template::ALL.map(|template| {
                    format!("{}  {}\n", template.name(), template.description())
                }).concat();
                let rows = Template::ALL.map(|template| {
                    json!({"id": template, "description": template.description()})
                });
                render(&host.io, json, &rows, &text)?;
                Ok(0)
            }
            AuthorCommand::Guide { topic } => {
                let text = if topic == "session-panel" {
                    "Start from plugins/turn-note. Subscribe to turn/end, append an ignorable plugin:<id>/<record> record after the listener returns, and use the same record in the manifest panel and tool. Run cargo test --offline -p hoh-plugin-turn-note. The sample shows event handling, record names and panel fields."
                } else {
                    topic.parse::<Template>()?.guide()
                };
                render(&host.io, json, &json!({"id": topic, "guide": text}), &format!("{text}\n"))?;
                Ok(0)
            }
            AuthorCommand::New { id, template } => {
                let scaffold = creator.create(&host.cwd, &id, template)?;
                let text = format!(
                    "Created {}\nTest: {}\nIndex after review: {}\n",
                    scaffold.path.display(), scaffold.test_command, scaffold.index_command
                );
                render(&host.io, json, &scaffold, &text)?;
                Ok(0)
            }
            AuthorCommand::Check { id } => {
                let inspection = creator.inspect(&host.cwd, &id, hoh_authored_plugins::PLUGINS)?;
                let mut text = String::new();
                for issue in &inspection.issues {
                    text.push_str(&format!("{}: {}\n", issue.file, issue.message));
                }
                let status = match inspection.linked_manifest {
                    LinkedManifest::Missing => "absent from this engine",
                    LinkedManifest::Matches => "matches the manifest embedded in this engine",
                    LinkedManifest::Differs => "differs from the manifest embedded in this engine",
                };
                text.push_str(&format!(
                    "Manifest {status}.\nTest: {}\nIndex after review: {}\n",
                    inspection.test_command, inspection.index_command
                ));
                render(&host.io, json, &inspection, &text)?;
                Ok(i32::from(!inspection.issues.is_empty()))
            }
        }
    }.await;
    kernel.shutdown().await;
    result
}

fn render(io: &CliIo, json: bool, value: &impl Serialize, text: &str) -> Result<(), BoxError> {
    if json {
        (io.stdout)(&format!("{}\n", serde_json::to_string(value)?))
    } else {
        (io.stdout)(text)
    }
}
