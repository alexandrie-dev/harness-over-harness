use hoh_loader_smoke::{ExampleLaunchOptions, resolve_example_launch, resolve_example_mode};
use serde_json::json;
use std::{env, io::Read, process::ExitCode, thread, time::Duration};

fn main() -> ExitCode {
    if env::var_os("LOADER_SMOKE_RESOLVE_MODE").is_some() {
        match resolve_example_mode(None) {
            Ok(mode) => println!("{}", mode.as_str()),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }
    if env::var_os("LOADER_SMOKE_RESOLVE_LAUNCH").is_some() {
        match resolve_example_launch(ExampleLaunchOptions {
            src_bin: "/repo/apps/cli/src/bin.ts".into(),
            ..Default::default()
        }) {
            Ok(launch) => println!("{}", launch.command),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    match env::var("LOADER_SMOKE_FIXTURE").as_deref() {
        Ok("fail") => {
            eprintln!("fixture failed");
            ExitCode::from(7)
        }
        Ok("hang") => {
            println!("fixture started");
            thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        Ok("delay") => {
            thread::sleep(Duration::from_millis(25));
            println!("fixture delayed");
            ExitCode::SUCCESS
        }
        Ok("signal") => std::process::abort(),
        _ => {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .expect("read fixture stdin");
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "args": env::args().skip(1).collect::<Vec<_>>(),
                    "cwd": env::current_dir().expect("fixture cwd"),
                    "dshHome": env::var("DSH_HOME").ok(),
                    "agentsHome": env::var("DSH_AGENTS_HOME").ok(),
                    "marker": env::var("LOADER_SMOKE_MARKER").ok(),
                    "input": input,
                }))
                .expect("fixture output is JSON")
            );
            eprintln!("fixture stderr");
            ExitCode::SUCCESS
        }
    }
}
