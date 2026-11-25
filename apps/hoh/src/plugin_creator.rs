//! CLI adapter for the built-in Plugin Creator service.

use crate::bin::{CliHost, CliIo};
use hoh_commander::{CommanderError, Output, ParseError};
use hoh_error::BoxError;
use hoh_kernel::Kernel;
use hoh_plugin_creator::{Creator, FORMAT_VERSION, Issue, LinkedManifest, PluginCreator, Template};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const HELP: &str = "\nPlugin Creator:\n  hoh plugin templates [--json]\n  hoh plugin guide <basic|tool|session-panel> [--json]\n  hoh plugin new <id> [--template <basic|tool>] [--json]\n  hoh plugin check <id> [--json]\n  hoh plugin test <id> [--json]\n";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum AuthorCommand {
    Templates,
    Guide { topic: String },
    New { id: String, template: Template },
    Check { id: String },
    Test { id: String },
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
    if plugin != "plugin"
        || !matches!(
            verb.as_str(),
            "templates" | "guide" | "new" | "check" | "test"
        )
    {
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
    let args: Vec<&str> = tail
        .iter()
        .map(String::as_str)
        .filter(|flag| *flag != "--json")
        .collect();
    let json = args.len() != tail.len();
    if tail.len() > args.len() + 1 {
        return Some(Err(argument_error(
            output,
            json,
            "invalid_arguments",
            "Pass --json only once.",
        )));
    }
    let command = match (verb.as_str(), args.as_slice()) {
        ("templates", []) => AuthorCommand::Templates,
        ("guide", [topic]) if !topic.starts_with('-') => AuthorCommand::Guide {
            topic: (*topic).into(),
        },
        ("new", [id]) => AuthorCommand::New {
            id: (*id).into(),
            template: Template::default(),
        },
        ("new", [id, flag, template]) if *flag == "--template" => match template.parse() {
            Ok(template) => AuthorCommand::New {
                id: (*id).into(),
                template,
            },
            Err(error) => {
                return Some(Err(argument_error(
                    output,
                    json,
                    "unknown_template",
                    error.to_string(),
                )));
            }
        },
        ("check", [id]) => AuthorCommand::Check { id: (*id).into() },
        ("test", [id]) => AuthorCommand::Test { id: (*id).into() },
        _ => {
            return Some(Err(argument_error(
                output,
                json,
                "invalid_arguments",
                "Invalid plugin command arguments.",
            )));
        }
    };
    Some(Ok(DshInvocation::PluginAuthor { command, json }))
}

fn argument_error(
    output: &Output,
    json: bool,
    code: &str,
    message: impl Into<String>,
) -> ParseError {
    let issue = Issue::new(code, message).with_hint("Run hoh plugin --help for syntax.");
    if json {
        (output.write_out)(&format!("{}\n", error_report(&issue)));
        CommanderError::new(2, "commander.invalidArgument", "").into()
    } else {
        ParseError::Other(issue.into())
    }
}

fn error_report(issue: &Issue) -> Value {
    json!({"formatVersion": FORMAT_VERSION, "ok": false, "issues": [issue]})
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
                    match topic.parse::<Template>() {
                        Ok(template) => template.guide(),
                        Err(error) => return render_error(&host.io, json,
                            Issue::new("unknown_guide", error.to_string())),
                    }
                };
                render(&host.io, json, &json!({"id": topic, "guide": text}), &format!("{text}\n"))?;
                Ok(0)
            }
            AuthorCommand::New { id, template } => {
                let scaffold = match creator.create(&host.cwd, &id, template) {
                    Ok(scaffold) => scaffold,
                    Err(issue) => return render_error(&host.io, json, issue),
                };
                let text = format!(
                    "Created {}\nTest: {}\nIndex preview: {}\n",
                    scaffold.path.display(), scaffold.test_command, scaffold.index_command
                );
                render(&host.io, json, &scaffold, &text)?;
                Ok(0)
            }
            AuthorCommand::Check { id } => {
                let inspection = match creator.inspect(&host.cwd, &id, hoh_authored_plugins::PLUGINS) {
                    Ok(inspection) => inspection,
                    Err(issue) => return render_error(&host.io, json, issue),
                };
                let mut text = String::new();
                for issue in &inspection.issues {
                    text.push_str(&format!("{issue}\n"));
                }
                let status = match inspection.linked_manifest {
                    LinkedManifest::Missing => "absent from this engine",
                    LinkedManifest::Matches => "matches the manifest embedded in this engine",
                    LinkedManifest::Differs => "differs from the manifest embedded in this engine",
                };
                text.push_str(&format!(
                    "Manifest {status}.\nTest: {}\nIndex preview: {}\n",
                    inspection.test_command, inspection.index_command
                ));
                render(&host.io, json, &inspection, &text)?;
                Ok(i32::from(!inspection.ok))
            }
            AuthorCommand::Test { id } => {
                let report = match creator.test(&host.cwd, &id) {
                    Ok(report) => report,
                    Err(issue) => return render_error(&host.io, json, issue),
                };
                let mut text = if report.ok {
                    format!("Plugin {id} passed.\n")
                } else {
                    report.issues.iter().map(|issue| format!("{issue}\n")).collect()
                };
                if let Some(log) = &report.log {
                    text.push_str(&format!("Log: {}\n", log.display()));
                }
                render(&host.io, json, &report, &text)?;
                Ok(i32::from(!report.ok))
            }
        }
    }.await;
    kernel.shutdown().await;
    result
}

fn render_error(io: &CliIo, json: bool, issue: Issue) -> Result<i32, BoxError> {
    if json {
        render(io, true, &error_report(&issue), "")?;
    } else {
        (io.stderr)(&format!("{issue}\n"))?;
    }
    Ok(1)
}

fn render(io: &CliIo, json: bool, value: &impl Serialize, text: &str) -> Result<(), BoxError> {
    if json {
        (io.stdout)(&format!("{}\n", serde_json::to_string(value)?))
    } else {
        (io.stdout)(text)
    }
}
