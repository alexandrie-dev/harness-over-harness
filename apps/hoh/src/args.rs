//! The launcher's flags stop at the first app argument.
use hoh_commander::{
    Command, CommanderError, InvalidArgumentError, Options, Output, ParseError, ParseOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum DshInvocation {
    Profile {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_default_profile: Option<String>,
        patches: Vec<String>,
        args: Vec<String>,
    },
    DumpConfig {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_default_profile: Option<String>,
        default_only: bool,
        patches: Vec<String>,
    },
    DumpConfigSchema {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_default_profile: Option<String>,
        patches: Vec<String>,
    },
    Plugin {
        profile: String,
        args: Vec<String>,
    },
    PluginNew {
        id: String,
    },
}
const HELP_EXAMPLES: &str = "\nExamples:\n  dsh web                                   boot the web profile (same as: dsh --profile web)\n  dsh rescue --from-default-profile web\n                                            recreate rescue from the shipped web template, then boot it\n  dsh headless \"run the tests\"              answer one task, print the result, and exit\n  dsh tui --patch ./extra.yml               boot a custom profile with one extra overlay\n  dsh tui --resume <session>                arguments after the launcher flags reach the app\n  dsh web --help                            the web app's own flags and help\n  hoh plugin new <id>                       create an in-repository Rust plugin scaffold\n  dsh plugin --profile tui add <package>    manage a profile plugin (unavailable in this build)\n";
fn select_profile(value: &str, previous: Option<&Value>) -> Result<Value, InvalidArgumentError> {
    if previous.is_some() {
        return Err(InvalidArgumentError::new("select a profile only once"));
    }
    Ok(value.into())
}
fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
fn reject_desktop(program: &Command, profile: &str) -> Result<(), CommanderError> {
    if profile.to_lowercase() == "desktop" {
        return Err(program.error(
            "error: profile \"desktop\" is managed exclusively by the Electron application",
        ));
    }
    Ok(())
}
fn resolve_boot(
    program: &Command,
    profile: String,
    options: &Options,
    args: Vec<String>,
) -> Result<DshInvocation, CommanderError> {
    let patches = strings(options.get("patch"));
    let from_default_profile = options
        .get("fromDefaultProfile")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if patches.iter().any(String::is_empty) {
        return Err(program.error("error: --patch needs a path"));
    }
    if from_default_profile.as_deref() == Some("") {
        return Err(program.error("error: --from-default-profile needs a name"));
    }
    let dump = ["dumpConfig", "dumpDefaultConfig", "dumpConfigSchema"]
        .iter()
        .filter(|k| options.get(**k) == Some(&Value::Bool(true)))
        .count();
    if dump == 0 {
        return Ok(DshInvocation::Profile {
            profile,
            from_default_profile,
            patches,
            args,
        });
    }
    if dump > 1 {
        return Err(program.error("error: --dump-config, --dump-default-config, and --dump-config-schema are mutually exclusive"));
    }
    if !args.is_empty() {
        return Err(program.error(&format!(
            "error: config dumps take no app arguments, got {}",
            args.iter()
                .map(|a| hoh_util_values::js_json_stringify(&Value::String(a.clone())))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    if options.get("dumpConfigSchema") == Some(&Value::Bool(true)) {
        return Ok(DshInvocation::DumpConfigSchema {
            profile,
            from_default_profile,
            patches,
        });
    }
    let default_only = options.get("dumpDefaultConfig") == Some(&Value::Bool(true));
    if default_only && !patches.is_empty() {
        return Err(program
            .error("error: --dump-default-config prints the bundle layers and takes no --patch"));
    }
    Ok(DshInvocation::DumpConfig {
        profile,
        from_default_profile,
        default_only,
        patches,
    })
}
/// Help/version/rejection return Commander's exit, after writing through output.
pub fn parse_dsh_args(
    argv: &[String],
    version: &str,
    manage_desktop_profile: bool,
    output: Output,
) -> Result<DshInvocation, ParseError> {
    if argv.first().map(String::as_str) == Some("plugin")
        && argv.get(1).map(String::as_str) == Some("new")
    {
        if argv.len() != 3 {
            return Err(ParseError::Other("usage: hoh plugin new <id>".into()));
        }
        return Ok(DshInvocation::PluginNew {
            id: argv[2].clone(),
        });
    }
    let first = argv.first().map(String::as_str);
    let resolved = Arc::new(Mutex::new(None));
    let result = resolved.clone();
    let mut program = Command::new();
    program.name("dsh").version_with_flags(version,"-V, --version","output the version number")
        .usage("[--profile] <name> [options] [app-args...]\n       dsh plugin --profile <name> <pnpm-args...>")
        .description("dsh: boot a DeepSeek Harness profile — an ordered stack of plugin-bundle patch layers under your own overrides.")
        .add_help_text("after",HELP_EXAMPLES).exit_override().configure_output(output)
        .help_option_enabled(false).help_command(false).allow_unknown_option(true).pass_through_options(true).enable_positional_options(true)
        .argument("[args...]","arguments for the booted profile's app (see: dsh --profile <name> --help)")
        .option_with_parser("--profile <name>","the profile under $DSH_HOME/profiles to boot",select_profile)
        .option("--from-default-profile <name>","replace a custom profile with a fresh copy of a shipped profile template")
        .option_with_parser("--patch <path>","extra patch-list overlay applied after the profile layer (repeatable)",|value,previous|{let mut items=strings(previous);items.push(value.into());Ok(items.into_iter().map(Value::String).collect::<Vec<_>>().into())})
        .option("--dump-config","print the composed profile tree and exit")
        .option("--dump-config-schema","print JSON Schema for profile entries and patches without mounting")
        .option("--dump-default-config","print the profile tree without its user layer or --patch overlays and exit")
        .action(move|arguments,options,program|{
            let args=strings(arguments.first().and_then(Option::as_ref));
            let Some(profile)=options.get("profile").and_then(Value::as_str)else{
                if args.iter().any(|a|matches!(a.as_str(),"-h"|"--help")){return Err(program.help().into());}
                return Err(program.error("error: --profile <name> is required").into());
            };
            if profile.is_empty(){return Err(program.error("error: --profile needs a name").into());}
            reject_desktop(program,profile)?;
            *result.lock().unwrap_or_else(|e|e.into_inner())=Some(resolve_boot(program,profile.into(),options,args)?);Ok(())
        });
    if first == Some("plugin") {
        let result = resolved.clone();
        program.command("plugin").description("manage a profile's plugins by forwarding the remaining arguments to pnpm in the profile directory")
            .help_option("-h, --help","show this help").pass_through_options(true)
            .required_option_with_parser("--profile <name>","the profile whose plugins to manage (initialized on first use)",select_profile)
            .allow_unknown_option(true).argument("[args...]","pnpm arguments, forwarded verbatim (add <pkg>, remove <pkg>, why <pkg>, ...)")
            .action(move|arguments,options,program|{
                let profile=options.get("profile").and_then(Value::as_str).unwrap_or_default();
                if profile.is_empty(){return Err(program.error("error: --profile needs a name").into());}
                if !manage_desktop_profile{reject_desktop(program,profile)?;}
                let args=strings(arguments.first().and_then(Option::as_ref));
                if args.is_empty(){return Err(program.error("error: plugin needs pnpm arguments to forward (e.g. add <package>)").into());}
                let profile=if profile.to_lowercase()=="desktop"{String::from("desktop")}else{profile.into()};
                *result.lock().unwrap_or_else(|e|e.into_inner())=Some(DshInvocation::Plugin{profile,args});Ok(())
            });
    }
    let expanded = if first.is_some_and(|f| !f.starts_with('-') && f != "plugin") {
        std::iter::once("--profile".into())
            .chain(argv.iter().cloned())
            .collect()
    } else {
        argv.to_vec()
    };
    program.parse(expanded, ParseOptions::USER)?;
    resolved
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or_else(|| ParseError::Other("dsh: no invocation resolved".into()))
}
