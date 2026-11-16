//! Landlock launcher and non-blocking flock (port of native/system of DeepSeek Harness).

mod entry;
mod flock;
mod launcher;

pub use entry::{
    LAUNCHER_BIN, LAUNCHER_FAILURE_EXIT, LandlockEnforcement, LauncherGrants, grant_args,
    launcher_path, probe,
};
pub use flock::try_lock_exclusive;
pub use launcher::run_launcher;
