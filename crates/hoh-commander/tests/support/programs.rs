//! Declarations copied from the six actual commander programs in dsh @39163dd3.
//! Gateway has no commander program; `plugin` is the sixth declaration.
use hoh_commander::{Command, InvalidArgumentError, Opt};
use serde_json::Value;

fn collect(value: &str, previous: Option<&Value>) -> Result<Value, InvalidArgumentError> {
    let mut values = previous
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    values.push(Value::String(value.into()));
    Ok(Value::Array(values))
}
fn select_profile(value: &str, previous: Option<&Value>) -> Result<Value, InvalidArgumentError> {
    if previous.is_some() {
        Err(InvalidArgumentError::new("select a profile only once"))
    } else {
        Ok(Value::String(value.into()))
    }
}

pub fn dsh() -> Command {
    let mut p = Command::new();
    p.name("dsh")
        .version_with_flags("1.2.3", "-V, --version", "output the version number")
        .usage("[--profile] <name> [options] [app-args...]\n       dsh plugin --profile <name> <pnpm-args...>")
        .description("dsh: boot a DeepSeek Harness profile — an ordered stack of plugin-bundle patch layers under your own overrides.")
        .add_help_text("after", "\nExamples:\n  dsh web                                   boot the web profile (same as: dsh --profile web)\n  dsh rescue --from-default-profile web\n                                            recreate rescue from the shipped web template, then boot it\n  dsh headless \"run the tests\"              answer one task, print the result, and exit\n  dsh tui --patch ./extra.yml               boot a custom profile with one extra overlay\n  dsh tui --resume <session>                arguments after the launcher flags reach the app\n  dsh web --help                            the web app's own flags and help\n  dsh plugin --profile tui add <package>    install a plugin into the tui profile\n")
        .exit_override()
        .help_option_enabled(false)
        .help_command(false)
        .allow_unknown_option(true)
        .pass_through_options(true)
        .enable_positional_options(true)
        .argument("[args...]", "arguments for the booted profile's app (see: dsh --profile <name> --help)")
        .option_with_parser("--profile <name>", "the profile under $DSH_HOME/profiles to boot", select_profile)
        .option("--from-default-profile <name>", "replace a custom profile with a fresh copy of a shipped profile template")
        .option_with_parser("--patch <path>", "extra patch-list overlay applied after the profile layer (repeatable)", collect)
        .option("--dump-config", "print the composed profile tree and exit")
        .option("--dump-config-schema", "print JSON Schema for profile entries and patches without mounting")
        .option("--dump-default-config", "print the profile tree without its user layer or --patch overlays and exit")
        .action(|_, opts, command| {
            let Some(profile) = opts.get("profile").and_then(Value::as_str) else {
                if command.args.iter().any(|s| s == "-h" || s == "--help") { return Err(command.help().into()); }
                return Err(command.error("error: --profile <name> is required").into());
            };
            if profile.is_empty() { return Err(command.error("error: --profile needs a name").into()); }
            if profile.eq_ignore_ascii_case("desktop") { return Err(command.error("error: profile \"desktop\" is managed exclusively by the Electron application").into()); }
            if opts.get("patch").and_then(Value::as_array).is_some_and(|v| v.iter().any(|v| v == "")) { return Err(command.error("error: --patch needs a path").into()); }
            if opts.get("fromDefaultProfile").is_some_and(|v| v == "") { return Err(command.error("error: --from-default-profile needs a name").into()); }
            let dumps = ["dumpConfig", "dumpDefaultConfig", "dumpConfigSchema"].iter().filter(|k| opts.get(**k) == Some(&Value::Bool(true))).count();
            if dumps > 1 { return Err(command.error("error: --dump-config, --dump-default-config, and --dump-config-schema are mutually exclusive").into()); }
            if dumps > 0 && !command.args.is_empty() {
                let details = command.args.iter().map(|s| serde_json::to_string(s).unwrap()).collect::<Vec<_>>().join(" ");
                return Err(command.error(&format!("error: config dumps take no app arguments, got {details}")).into());
            }
            if opts.get("dumpDefaultConfig") == Some(&Value::Bool(true)) && opts.get("patch").and_then(Value::as_array).is_some_and(|v| !v.is_empty()) { return Err(command.error("error: --dump-default-config prints the bundle layers and takes no --patch").into()); }
            Ok(())
        });
    p
}

pub fn plugin() -> Command {
    let mut p = dsh();
    p.command("plugin")
        .description("manage a profile's plugins by forwarding the remaining arguments to pnpm in the profile directory")
        .help_option("-h, --help", "show this help")
        .pass_through_options(true)
        .required_option_with_parser("--profile <name>", "the profile whose plugins to manage (initialized on first use)", select_profile)
        .allow_unknown_option(true)
        .argument("[args...]", "pnpm arguments, forwarded verbatim (add <pkg>, remove <pkg>, why <pkg>, ...)")
        .action(|_, opts, command| {
            let profile = opts.get("profile").and_then(Value::as_str).unwrap_or_default();
            if profile.is_empty() { return Err(command.error("error: --profile needs a name").into()); }
            if profile.eq_ignore_ascii_case("desktop") { return Err(command.error("error: profile \"desktop\" is managed exclusively by the Electron application").into()); }
            if command.args.is_empty() { return Err(command.error("error: plugin needs pnpm arguments to forward (e.g. add <package>)").into()); }
            Ok(())
        });
    p
}

pub fn headless() -> Command {
    let mut p = Command::new();
    p.name("dsh --profile headless")
        .description("Answer one task and exit; the answer goes to stdout and diagnostics to stderr.")
        .help_option("-h, --help", "show this help")
        .option("--json", "write newline-delimited run events to stdout instead of the final message")
        .option("--session-id <id>", "adopt the persisted Session with this id; an unknown id is an error")
        .option("--permission-mode <preset>", "select a configured permission preset for this task, including on resume")
        .add_option(Opt::new("--approval-policy <policy>", "set approval behavior after selecting the permission preset").choices(["ask", "never"]))
        .option_with_parser("--add-dir <path>", "allow an additional physical absolute directory under workspace-write (repeatable)", collect)
        .argument("[task...]", "the task text; multiple words are joined by spaces, and `-` reads stdin")
        .add_help_text("after", "\nExamples:\n  dsh --profile headless \"run the tests\"          answer one task and exit\n  echo \"run the tests\" | dsh --profile headless   read the task from stdin\n  dsh --profile headless --json \"run the tests\"   emit machine-readable run events\n  dsh --profile headless --session-id session-… \"continue\"   resume an existing Session\n  dsh --profile headless --permission-mode workspace-write --add-dir /repo/.git \"commit the fix\"\n                                                        grant shared Git metadata for this task\n")
        .action(|_, opts, command| {
            if command.args.len() > 1 && command.args.iter().any(|s| s == "-") { return Err(command.error("error: `-` must be the only task argument").into()); }
            if !command.args.is_empty() && command.args.join(" ").trim().is_empty() { return Err(command.error("error: a task is required, for example: dsh --profile headless \"run the tests\"").into()); }
            for (key, text) in [("sessionId", "error: --session-id requires a non-empty session id"), ("permissionMode", "error: --permission-mode requires a non-empty preset name")] {
                if opts.get(key).and_then(Value::as_str).is_some_and(|s| s.trim().is_empty()) { return Err(command.error(text).into()); }
            }
            if opts.get("addDir").and_then(Value::as_array).is_some_and(|a| a.iter().filter_map(Value::as_str).any(|s| !std::path::Path::new(s).is_absolute() || s.contains('\0'))) { return Err(command.error("error: --add-dir requires an absolute directory path in the execution environment").into()); }
            Ok(())
        });
    p
}

pub fn sdk() -> Command {
    let mut p = Command::new();
    p.name("dsh --profile sdk")
        .description("Serve DeepSeek Harness SDK clients over stdio JSON-RPC.")
        .help_option("-h, --help", "show this help")
        .add_help_text("after", "\nExample:\n  dsh --profile sdk     serve one SDK runtime until its client disconnects\n")
        .action(|_, _, _| Ok(()));
    p
}

pub fn acp() -> Command {
    let mut p = Command::new();
    p.name("dsh --profile acp")
        .description("Serve automation clients over Agent Client Protocol stdio.")
        .help_option("-h, --help", "show this help")
        .add_help_text(
            "after",
            "\nExample:\n  dsh --profile acp     serve ACP until the client disconnects\n",
        )
        .action(|_, _, _| Ok(()));
    p
}

pub fn web() -> Command {
    let mut p = Command::new();
    p.name("dsh --profile web")
        .description("Serve the DeepSeek Harness browser UI.")
        .help_option("-h, --help", "show this help")
        .option("--host <host>", "bind address: one concrete IPv4 or IPv6 literal of a local interface; wildcard addresses are rejected")
        .option("--no-open", "do not open the Web UI in the default browser")
        .option("--port <port>", "listen port; pass 0 to let the OS pick a free one")
        .option("--public-url <url>", "advertise this HTTP(S) root in the printed, opened, runtime-context, and DSH_WEB_URL forms; grants no trust")
        .option("--trusted-host <authority...>", "extra authority the /api browser-trust fence accepts (host or host:port; repeatable)")
        .option("--tls-cert <file>", "PEM server certificate chain; requires --tls-key")
        .option("--tls-key <file>", "unencrypted PEM private key; requires --tls-cert")
        .add_help_text("after", "\nExamples:\n  dsh --profile web                          serve on the composed host and port\n  dsh --profile web --no-open                serve without opening a browser\n  dsh --profile web --port 8080              serve on another port\n  dsh --profile web --host 10.0.0.7          bind one local interface address\n  dsh --profile web --public-url https://app.example/ui/ --trusted-host app.example\n                                             advertise a prefix-stripping HTTPS proxy entry and admit its authority\n  dsh --profile web --tls-cert ./server-chain.pem --tls-key ./server-key.pem\n                                             serve HTTPS with an existing certificate and key\n")
        .action(|_, opts, command| {
            if let Some(port) = opts.get("port").and_then(Value::as_str) && (port.is_empty() || !port.bytes().all(|c| c.is_ascii_digit())) { return Err(command.error(&format!("error: --port must be a number, got {}", serde_json::to_string(port).unwrap())).into()); }
            if opts.contains_key("tlsCert") != opts.contains_key("tlsKey") { return Err(command.error("error: --tls-cert and --tls-key must be supplied together").into()); }
            Ok(())
        });
    p
}
