//! Provider-private single-threaded exec bootstrap; never starts a Tokio runtime.
fn main() -> std::process::ExitCode {
    let Some(selection) =
        std::env::var_os(hoh_subprocess_local::runner_launch::SUBPROCESS_RUNNER_ENV)
    else {
        return std::process::ExitCode::from(127);
    };
    let args = std::env::args_os()
        .skip(1)
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let code = hoh_subprocess_local::spawn_runner::run_selected_subprocess_runner(
        &selection.to_string_lossy(),
        &args,
    );
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(127))
}
