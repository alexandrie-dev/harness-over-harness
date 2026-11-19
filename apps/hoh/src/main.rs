use hoh as application;
use std::{io::Write, sync::Arc};
fn main() -> std::process::ExitCode {
    let result = match std::thread::Builder::new()
        .name("hoh".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(run)
    {
        Ok(worker) => match worker.join() {
            Ok(result) => result,
            Err(payload) => Err(payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).into()))
                .unwrap_or_else(|| "launcher panicked".into())
                .into()),
        },
        Err(error) => Err(Box::new(error) as hoh_error::BoxError),
    };
    match result {
        // Like Node and Unix process statuses, the portable status retains the low eight bits.
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn run() -> Result<i32, hoh_error::BoxError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(4)
        .thread_stack_size(64 * 1024 * 1024)
        .build()?;
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let args = std::env::args_os()
        .skip(1)
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let io = application::bin::CliIo {
        stdout: Arc::new(|text| {
            let mut stream = std::io::stdout().lock();
            stream.write_all(text.as_bytes())?;
            stream.flush()?;
            Ok(())
        }),
        stderr: Arc::new(|text| {
            let mut stream = std::io::stderr().lock();
            stream.write_all(text.as_bytes())?;
            stream.flush()?;
            Ok(())
        }),
    };
    let host_io = io.clone();
    runtime.block_on(application::bin::run_cli_with_host(
        &args,
        env!("CARGO_PKG_VERSION"),
        io,
        move || {
            let environment = std::env::vars_os()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
                .collect::<indexmap::IndexMap<_, _>>();
            let inherited = environment
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::HashMap<_, _>>();
            Ok(application::bin::CliHost {
                cwd: std::env::current_dir()?,
                home: hoh_home_paths::resolve_dsh_home(None, Some(&inherited))?,
                exe: std::env::current_exe()?,
                platform,
                environment,
                io: host_io,
                force_exit: Arc::new(|code| std::process::exit(code)),
            })
        },
    ))
}
