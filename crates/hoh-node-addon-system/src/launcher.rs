use std::ffi::OsString;
#[cfg(unix)]
use std::io;
use std::io::Write;

use crate::LAUNCHER_FAILURE_EXIT;

const NOT_ENFORCED: &str = "landlock is not enforced by this kernel (ABI unsupported or disabled)";

#[derive(Default)]
struct Cli {
    probe: bool,
    ro: Vec<OsString>,
    rw: Vec<OsString>,
    command: Vec<OsString>,
}

enum ParseError {
    OutOfMemory,
    ProbeExtras,
    MissingPath(OsString),
    Unknown(OsString),
    MissingCommand,
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli, ParseError> {
    let mut all_args = Vec::new();
    for arg in args {
        all_args
            .try_reserve(1)
            .map_err(|_| ParseError::OutOfMemory)?;
        all_args.push(arg);
    }
    let argc = all_args.len();
    let mut cli = Cli::default();
    // Mirror C's fallible calloc for both grant lists; move rather than clone paths.
    cli.ro
        .try_reserve_exact(argc)
        .map_err(|_| ParseError::OutOfMemory)?;
    cli.rw
        .try_reserve_exact(argc)
        .map_err(|_| ParseError::OutOfMemory)?;
    cli.command
        .try_reserve_exact(argc)
        .map_err(|_| ParseError::OutOfMemory)?;
    let mut args = all_args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--probe" {
            if argc != 1 {
                return Err(ParseError::ProbeExtras);
            }
            cli.probe = true;
        } else if arg == "--ro" || arg == "--rw" {
            let Some(path) = args.next() else {
                return Err(ParseError::MissingPath(arg));
            };
            if arg == "--ro" {
                cli.ro.push(path);
            } else {
                cli.rw.push(path);
            }
        } else if arg == "--" {
            cli.command.extend(args);
            break;
        } else {
            return Err(ParseError::Unknown(arg));
        }
    }
    if !cli.probe && cli.command.is_empty() {
        return Err(ParseError::MissingCommand);
    }
    Ok(cli)
}

fn fail(stderr: &mut dyn Write, message: &[u8]) -> u8 {
    let _ = stderr.write_all(b"landlock-run: ");
    let _ = stderr.write_all(message);
    let _ = stderr.write_all(b"\n");
    LAUNCHER_FAILURE_EXIT
}

// Rust appends the errno number to strerror; the C protocol prints only strerror.
#[cfg(unix)]
fn strerror(error: &io::Error) -> String {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(errno) => text
            .strip_suffix(&format!(" (os error {errno})"))
            .unwrap_or(&text)
            .to_owned(),
        None => text,
    }
}

#[cfg(target_os = "linux")]
fn ruleset_error(error: landlock::RulesetError) -> Vec<u8> {
    use std::error::Error;
    let mut cause: &dyn Error = &error;
    loop {
        if let Some(error) = cause.downcast_ref::<io::Error>() {
            return format!("landlock ruleset error: {}", strerror(error)).into_bytes();
        }
        match cause.source() {
            Some(source) => cause = source,
            None => {
                return format!(
                    "landlock ruleset error: {}",
                    strerror(&io::Error::from_raw_os_error(
                        rustix::io::Errno::INVAL.raw_os_error()
                    ))
                )
                .into_bytes();
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn restrict(cli: &Cli) -> Result<bool, Vec<u8>> {
    use landlock::{
        ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus,
    };

    // V1 is mandatory: detect a disabled/unsupported ABI before opening any grants.
    let ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(ABI::V1))
        .map_err(|_| NOT_ENFORCED.as_bytes().to_vec())?;
    // Request exactly bits 0..15, allowing the crate to intersect with the kernel ABI.
    let mut ruleset = ruleset
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI::V5))
        .map_err(ruleset_error)?
        .create()
        .map_err(ruleset_error)?;
    for (paths, access) in [
        (&cli.ro, AccessFs::from_read(ABI::V5)),
        (&cli.rw, AccessFs::from_all(ABI::V5)),
    ] {
        for path in paths {
            let fd = PathFd::new(path).map_err(|error| {
                let mut message = b"cannot open rule path: ".to_vec();
                message.extend_from_slice(path.as_encoded_bytes());
                message.extend_from_slice(b": ");
                // PathFd errors originate in open(2).
                let source = match error {
                    landlock::PathFdError::OpenCall { source, .. } => source,
                    _ => io::Error::from_raw_os_error(rustix::io::Errno::INVAL.raw_os_error()),
                };
                message.extend_from_slice(strerror(&source).as_bytes());
                message
            })?;
            // Trim before add_rule: automatic trimming would mark full enforcement partial.
            let access = match rustix::fs::fstat(&fd) {
                Ok(stat)
                    if rustix::fs::FileType::from_raw_mode(stat.st_mode)
                        != rustix::fs::FileType::Directory =>
                {
                    access & AccessFs::from_file(ABI::V5)
                }
                _ => access,
            };
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, access))
                .map_err(ruleset_error)?;
        }
    }
    let status = ruleset
        .no_new_privs(true)
        .restrict_self()
        .map_err(ruleset_error)?;
    if !status.no_new_privs || status.ruleset == RulesetStatus::NotEnforced {
        return Err(NOT_ENFORCED.as_bytes().to_vec());
    }
    // The C launcher reports only ABI completeness, independently of grant types.
    Ok(matches!(status.landlock,
        landlock::LandlockStatus::Available { effective_abi, .. } if effective_abi < ABI::V5))
}

#[cfg(not(target_os = "linux"))]
fn restrict(_cli: &Cli) -> Result<bool, Vec<u8>> {
    Err(NOT_ENFORCED.as_bytes().to_vec())
}

/// Execute the launcher protocol. `args` excludes argv[0]; writers receive only
/// launcher output. Parsing failures can be tested without starting a process.
///
/// Successful calls restrict the calling thread permanently, and command mode
/// replaces the process. Call this only in a dedicated launcher process.
pub fn run_launcher(
    args: impl IntoIterator<Item = OsString>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let cli = match parse(args) {
        Ok(cli) => cli,
        Err(error) => {
            let (message, detail): (&[u8], &[u8]) = match &error {
                ParseError::OutOfMemory => return fail(stderr, b"out of memory"),
                ParseError::ProbeExtras => (b"--probe takes no other arguments", b""),
                ParseError::MissingPath(arg) => (arg.as_encoded_bytes(), b" requires a path"),
                ParseError::Unknown(arg) => (b"unknown argument: ", arg.as_encoded_bytes()),
                ParseError::MissingCommand => (b"missing `-- <argv>...` command", b""),
            };
            let _ = stderr.write_all(b"landlock-run: usage error: ");
            let _ = stderr.write_all(message);
            let _ = stderr.write_all(detail);
            let _ = stderr.write_all(b"\n");
            return LAUNCHER_FAILURE_EXIT;
        }
    };
    let probe_cli;
    let profile = if cli.probe {
        probe_cli = Cli {
            ro: vec![OsString::from("/")],
            ..Cli::default()
        };
        &probe_cli
    } else {
        &cli
    };
    let partial = match restrict(profile) {
        Ok(partial) => partial,
        Err(message) => return fail(stderr, &message),
    };
    if cli.probe {
        let _ = writeln!(
            stdout,
            "landlock: {}",
            if partial {
                "partially enforced (older ABI)"
            } else {
                "fully enforced"
            }
        );
        return 0;
    }
    if partial {
        let _ = stderr.write_all(b"landlock-run: partial enforcement (older Landlock ABI)\n");
    }
    exec(&cli.command, stderr)
}

#[cfg(unix)]
fn exec(command: &[OsString], stderr: &mut dyn Write) -> u8 {
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(&command[0])
        .args(&command[1..])
        .exec();
    fail(
        stderr,
        format!("exec failed: {}", strerror(&error)).as_bytes(),
    )
}

#[cfg(not(unix))]
fn exec(_command: &[OsString], stderr: &mut dyn Write) -> u8 {
    fail(stderr, NOT_ENFORCED.as_bytes())
}
