use hoh_node_addon_system::run_launcher;
use std::{
    env::args_os,
    io::{stderr, stdout},
    process::ExitCode,
};

fn main() -> ExitCode {
    run_launcher(args_os().skip(1), &mut stdout(), &mut stderr()).into()
}
