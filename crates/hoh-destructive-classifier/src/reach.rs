//! What a shell command reaches besides the folder it runs in: every word that may name a file
//! (arguments, option values, assignment values, redirection targets, program paths) and the
//! network programs it runs. The external harness slot answers a command without a question only
//! when all of these stay inside the session folder and there is no network (HOH-18). A command
//! line it cannot read for sure is not understood (`None`): the caller asks.

use crate::classify::{KEYWORDS, is_assignment};
use crate::shell::{self, Command, RedirKind, Word};

/// What [`command_reach`] found in a command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandReach {
    /// Every word that may name a file, as written: relative ones are relative to the folder the
    /// command runs in, `~` is kept as written (the home folder). Words that are not file names
    /// (`foo` in `rg foo src`) are listed too: they resolve inside the folder.
    pub paths: Vec<String>,
    /// The network programs it runs (`curl`, `git push`, `npm install`).
    pub network: Vec<String>,
}

/// The files and the network a shell command line reaches; `None` when the line is not
/// understood: it does not parse, a word is only known when the shell runs (a variable, a
/// substitution, a glob, `~` expansion, braces), the folder changes (`cd`), or a command runs
/// code the line does not show (`sudo`, `env`, `xargs`, `sh -c`, `python -c`, `awk`,
/// `find -exec`).
pub fn command_reach(command: &str) -> Option<CommandReach> {
    let parsed = shell::parse(command).ok()?;
    if !parsed.extra_subst.is_empty() {
        return None;
    }
    let mut reach = CommandReach::default();
    for command in &parsed.commands {
        simple(command, &mut reach)?;
    }
    Some(reach)
}

/// The special files a redirection may name without touching any folder.
const DEVICES: &[&str] = &["/dev/null", "/dev/stdin", "/dev/stdout", "/dev/stderr"];

/// Commands that run another command, change the folder, or take code the line does not show.
const UNCLEAR: &[&str] = &[
    "cd",
    "pushd",
    "popd",
    "sudo",
    "doas",
    "su",
    "runuser",
    "pkexec",
    "env",
    "xargs",
    "eval",
    "exec",
    "source",
    ".",
    "command",
    "builtin",
    "nohup",
    "nice",
    "ionice",
    "timeout",
    "watch",
    "strace",
    "ltrace",
    "chroot",
    "unshare",
    "setsid",
    "stdbuf",
    "script",
    "flock",
    "parallel",
    "busybox",
    "fakeroot",
    "firejail",
    "systemd-run",
    "at",
    "batch",
    "crontab",
    "sg",
    "tmux",
    "screen",
    "trap",
    "alias",
    "awk",
    "gawk",
    "mawk",
    "nawk",
];

/// Interpreters: they run a script file named as their first argument; code given inline
/// (`-c`, `-e`, `-m`) or on standard input is not understood.
const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "csh",
    "tcsh",
    "python",
    "python2",
    "python3",
    "node",
    "nodejs",
    "deno",
    "bun",
    "perl",
    "ruby",
    "php",
    "lua",
    "luajit",
    "Rscript",
    "R",
    "osascript",
    "pwsh",
    "powershell",
    "tclsh",
    "expect",
];

/// Programs that always use the network.
const NETWORK: &[&str] = &[
    "curl",
    "wget",
    "nc",
    "ncat",
    "netcat",
    "socat",
    "telnet",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "ftp",
    "lftp",
    "tftp",
    "aria2c",
    "http",
    "https",
    "xh",
    "ping",
    "ping6",
    "dig",
    "nslookup",
    "host",
    "whois",
    "traceroute",
    "mtr",
    "ssh-copy-id",
    "sshpass",
    "apt",
    "apt-get",
    "yum",
    "dnf",
    "pacman",
    "brew",
    "snap",
    "flatpak",
    "docker",
    "podman",
    "kubectl",
    "gh",
    "glab",
    "aws",
    "gcloud",
    "az",
    "npx",
    "pnpx",
    "bunx",
    "pipx",
];

/// Programs whose subcommands named here use the network (`git push`, `npm install`).
const NETWORK_SUBCOMMANDS: &[(&str, &[&str])] = &[
    (
        "git",
        &[
            "push",
            "pull",
            "fetch",
            "clone",
            "ls-remote",
            "remote",
            "submodule",
            "send-email",
            "request-pull",
        ],
    ),
    (
        "npm",
        &[
            "install", "i", "add", "ci", "update", "upgrade", "publish", "exec", "x", "create",
            "init", "audit", "outdated", "view", "info", "search", "login", "adduser",
        ],
    ),
    (
        "pnpm",
        &[
            "install", "i", "add", "update", "up", "publish", "dlx", "exec", "create", "audit",
            "outdated",
        ],
    ),
    (
        "yarn",
        &[
            "install", "add", "upgrade", "up", "publish", "dlx", "create", "audit", "outdated",
            "info",
        ],
    ),
    ("pip", &["install", "download", "search", "index"]),
    ("pip3", &["install", "download", "search", "index"]),
    (
        "uv",
        &["pip", "add", "sync", "lock", "tool", "run", "publish"],
    ),
    (
        "poetry",
        &["install", "add", "update", "lock", "publish", "self"],
    ),
    (
        "cargo",
        &[
            "install", "publish", "fetch", "update", "add", "search", "login", "owner", "yank",
        ],
    ),
    ("gem", &["install", "push", "fetch", "update", "sources"]),
    ("go", &["get", "install", "mod"]),
    (
        "composer",
        &["install", "require", "update", "create-project"],
    ),
    ("bundle", &["install", "update", "add"]),
];

/// Options that take the next word as their value, before a subcommand.
const VALUE_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--prefix",
    "--cwd",
    "--dir",
];

fn simple(command: &Command, reach: &mut CommandReach) -> Option<()> {
    let words = &command.words;
    if words.iter().any(unknown) || command.redirs.iter().any(|(_, word)| unknown(word)) {
        return None;
    }
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let text = word.text.as_str();
        if KEYWORDS.contains(&text) {
            index += 1;
        } else if is_assignment(text) {
            // `GIT_DIR=/x git status`: the value is a path the command uses.
            reach
                .paths
                .extend(text.split_once('=').map(|(_, value)| value.to_owned()));
            index += 1;
        } else {
            break;
        }
    }
    if let Some(program) = words.get(index) {
        let args = &words[index + 1..];
        let name = program.text.rsplit('/').next().unwrap_or(&program.text);
        if program.text.contains('/') {
            reach.paths.push(program.text.clone());
        }
        if UNCLEAR.contains(&name)
            || (INTERPRETERS.contains(&name)
                && args.first().is_none_or(|first| first.text.starts_with('-')))
            || (name == "sed" && !reads_lines(args))
            || (name == "find"
                && args.iter().any(|arg| {
                    arg.text.starts_with("-exec")
                        || arg.text.starts_with("-ok")
                        || arg.text.starts_with("-fprint")
                        || arg.text == "-fls"
                        || arg.text == "-delete"
                }))
        {
            return None;
        }
        if let Some(network) = network(name, args) {
            reach.network.push(network);
        }
        let mut options = true;
        for arg in args {
            let text = arg.text.as_str();
            if options && text == "--" {
                options = false;
                continue;
            }
            arg_paths(text, options, &mut reach.paths);
        }
    }
    for (kind, target) in &command.redirs {
        let text = target.text.as_str();
        match kind {
            RedirKind::Heredoc => {}
            RedirKind::Dup if text == "-" || text.chars().all(|c| c.is_ascii_digit()) => {}
            _ if DEVICES.contains(&text) => {}
            _ => reach.paths.push(text.to_owned()),
        }
    }
    Some(())
}

/// A word whose value is only known when the shell runs.
fn unknown(word: &Word) -> bool {
    word.dynamic || word.glob || word.lead_dynamic || !word.subst.is_empty()
}

/// The words of one argument that may name a file: the argument itself, the value after `=`
/// (`of=/dev/sda`, `--prefix=/usr`), the value glued to short options (`-C/etc`, `-o../x`,
/// `-xzf/tmp/a.tar`). The separate value of an option is the next argument and is listed as one.
fn arg_paths(text: &str, options: bool, paths: &mut Vec<String>) {
    if options && text.starts_with('-') && text != "-" {
        if let Some(long) = text.strip_prefix("--") {
            paths.extend(long.split_once('=').map(|(_, value)| value.to_owned()));
        } else if let Some(at) = text[1..].find(['/', '~', '.']) {
            paths.push(text[1 + at..].to_owned());
        }
        return;
    }
    paths.push(text.to_owned());
    if let Some((_, value)) = text.split_once('=') {
        paths.push(value.to_owned());
    }
}

/// `sed -n '<lines>p' …`: only printing lines is understood (a sed script can write files and
/// run commands: `w`, `e`).
fn reads_lines(args: &[Word]) -> bool {
    let mut quiet = false;
    let mut scripts = 0;
    for arg in args {
        let text = arg.text.as_str();
        if text == "-n" || text == "--quiet" || text == "--silent" {
            quiet = true;
        } else if text.starts_with('-') {
            return false;
        } else if scripts == 0 {
            if !is_line_print(text) {
                return false;
            }
            scripts += 1;
        }
    }
    quiet && scripts == 1
}

/// `12p`, `1,200p`, `$p`, `5,$p`.
fn is_line_print(script: &str) -> bool {
    let Some(range) = script.trim().strip_suffix('p') else {
        return false;
    };
    let line = |part: &str| {
        let part = part.trim();
        part == "$" || (!part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    };
    match range.split_once(',') {
        Some((from, to)) => line(from) && line(to),
        None => line(range),
    }
}

/// The network program or subcommand a command runs, as `curl` or `git push`.
fn network(name: &str, args: &[Word]) -> Option<String> {
    if NETWORK.contains(&name) {
        return Some(name.to_owned());
    }
    let (_, subcommands) = NETWORK_SUBCOMMANDS
        .iter()
        .find(|(program, _)| *program == name)?;
    let mut words = args.iter().map(|arg| arg.text.as_str());
    let subcommand = loop {
        match words.next() {
            // `yarn` alone installs.
            None => return (name == "yarn").then(|| name.to_owned()),
            Some(option) if VALUE_OPTIONS.contains(&option) => {
                words.next();
            }
            Some(option) if option.starts_with('-') => {}
            Some(word) => break word,
        }
    };
    subcommands
        .contains(&subcommand)
        .then(|| format!("{name} {subcommand}"))
}
