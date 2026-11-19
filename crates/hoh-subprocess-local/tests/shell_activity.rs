mod common;
use common::*;
use hoh_kernel::Kernel;
use hoh_subprocess::*;
use hoh_subprocess_local::LocalSubprocessRuntime;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;

struct Shell {
    kernel: Option<Kernel>,
    runtime: Arc<LocalSubprocessRuntime>,
    home: tempfile::TempDir,
    control: Arc<dyn SubprocessTerminalControl>,
    output: Arc<Mutex<String>>,
    reader: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Shell {
    fn drop(&mut self) {
        self.runtime.terminate_for_host_exit();
    }
}
impl Shell {
    async fn start(path: &str, rc: &str, env_file: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zshenv"), env_file).unwrap();
        std::fs::write(
            home.path().join(".zshrc"),
            format!("PROMPT='READY> '\n{rc}\n"),
        )
        .unwrap();
        std::fs::write(
            home.path().join(".bashrc"),
            format!("PS1='READY> '\n{rc}\n"),
        )
        .unwrap();
        let (kernel, _, runtime) = setup();
        let mut spec = terminal_spec();
        spec.argv = vec![path.into(), "-i".into()];
        spec.cwd = home.path().to_string_lossy().into_owned();
        spec.env = Some(
            [
                ("HOME".into(), spec.cwd.clone()),
                ("ZDOTDIR".into(), spec.cwd.clone()),
            ]
            .into(),
        );
        spec.grace_ms = 200.0;
        let mut handle = runtime.spawn_terminal(spec).await.unwrap();
        let output = Arc::new(Mutex::new(String::new()));
        let recorded = output.clone();
        let reader = tokio::spawn(async move {
            let mut bytes = [0; 4096];
            loop {
                let count = handle.output.read(&mut bytes).await.unwrap();
                if count == 0 {
                    break;
                }
                recorded
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&bytes[..count]));
            }
        });
        let shell = Self {
            kernel: Some(kernel),
            runtime,
            home,
            control: handle.lifecycle,
            output,
            reader: Some(reader),
        };
        shell.until("READY>").await;
        shell
    }
    fn text(&self) -> String {
        self.output.lock().unwrap().clone()
    }
    async fn until(&self, expected: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.text().contains(expected) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("missing {expected:?}: {:?}", self.text()));
    }
    async fn state(&self, expected: SubprocessTerminalActivityState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.control.inspect_activity().await.unwrap().state != expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("activity did not reach {expected:?}: {:?}", self.text()));
    }
    async fn write(&self, value: &str) {
        self.control.write(value).await.unwrap();
    }
    async fn finish(mut self) {
        self.control.terminate().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.reader.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        shutdown(self.kernel.take().unwrap()).await;
    }
}
use SubprocessTerminalActivityState::{Busy, Idle, Unknown};
// Same platform/executable precondition as the source's describe.skipIf.
fn zsh_available() -> bool {
    std::path::Path::new("/bin/zsh").exists()
}

#[tokio::test]
async fn keeps_silent_foreground_work_builtin_loops_read_and_background_jobs_busy_until_a_new_prompt_is_idle()
 {
    if !zsh_available() {
        return;
    }
    let shell = Shell::start("/bin/zsh", "", "").await;
    shell.state(Idle).await;
    for command in [
        "sleep 600",
        "while :; do :; done",
        "read answer",
        "sleep 600 &",
    ] {
        shell.write(&format!("{command}\r")).await;
        shell.state(Busy).await;
        shell
            .write(if command.ends_with('&') {
                "kill %1\r"
            } else {
                "\x03"
            })
            .await;
        shell.state(Idle).await;
    }
    shell.finish().await;
}
#[tokio::test]
async fn does_not_confuse_vared_or_multiline_editing_with_a_top_level_empty_prompt() {
    if !zsh_available() {
        return;
    }
    let shell = Shell::start("/bin/zsh", "", "").await;
    shell.state(Idle).await;
    shell.write("value=abc; vared value\r").await;
    shell.state(Busy).await;
    shell.write("\x03").await;
    shell.state(Idle).await;
    shell.write("if true; then\r").await;
    shell.state(Busy).await;
    shell.write("fi\r").await;
    shell.state(Idle).await;
    shell.write("partial").await;
    assert_eq!(
        shell.control.inspect_activity().await.unwrap().state,
        Unknown
    );
    shell.finish().await;
}
#[tokio::test]
async fn preserves_startup_options_and_zdotdir_and_supports_noclobber_status_updates() {
    if !zsh_available() {
        return;
    }
    let shell = Shell::start(
        "/bin/zsh",
        "setopt noclobber\nprint RC-LOADED",
        "print ENV-LOADED",
    )
    .await;
    assert!(shell.text().contains("RC-LOADED"));
    assert!(shell.text().contains("ENV-LOADED"));
    shell.state(Idle).await;
    shell.write("print -r -- \"DIRECTORY:$ZDOTDIR\"\r").await;
    shell
        .until(&format!("DIRECTORY:{}", shell.home.path().display()))
        .await;
    shell.state(Idle).await;
    assert!(!shell.text().contains("file exists"));
    shell.finish().await;
}
#[tokio::test]
async fn protects_a_stopped_job_even_after_the_shell_returns_to_the_prompt() {
    if !zsh_available() {
        return;
    }
    let shell = Shell::start("/bin/zsh", "", "").await;
    shell.state(Idle).await;
    shell.write("sleep 600\r").await;
    shell.state(Busy).await;
    shell.write("\x1a").await;
    shell.until("suspended").await;
    assert_eq!(shell.control.inspect_activity().await.unwrap().state, Busy);
    shell.write("kill -KILL %1\r").await;
    shell.state(Idle).await;
    shell.finish().await;
}
#[tokio::test]
async fn refuses_idle_evidence_when_custom_signal_traps_may_run_without_terminal_input() {
    if !zsh_available() {
        return;
    }
    let shell = Shell::start("/bin/zsh", "trap ':' USR1", "").await;
    assert_eq!(
        shell.control.inspect_activity().await.unwrap().state,
        Unknown
    );
    shell.write("trap - USR1\r").await;
    shell.state(Idle).await;
    shell.finish().await;
}
#[tokio::test]
async fn leaves_disabled_startup_files_disabled_and_restores_zdotdir_immediately() {
    if !zsh_available() {
        return;
    }
    let shell = Shell::start(
        "/bin/zsh",
        "print UNEXPECTED-RC",
        "unsetopt rcs\nPROMPT='READY> '",
    )
    .await;
    assert!(!shell.text().contains("UNEXPECTED-RC"));
    shell.write("print -r -- \"DIRECTORY:$ZDOTDIR\"\r").await;
    shell
        .until(&format!("DIRECTORY:{}", shell.home.path().display()))
        .await;
    shell.finish().await;
}
#[tokio::test]
async fn preserves_scalar_prompt_hooks_and_uses_unknown_on_shells_without_ps0() {
    let shell = Shell::start(
        "/bin/bash",
        "PROMPT_COMMAND='printf HOOK; # user comment'\nset -C",
        "",
    )
    .await;
    assert!(shell.text().contains("HOOK"));
    shell
        .write("printf 'VERSION:%s\\n' \"$BASH_VERSION\"\r")
        .await;
    shell.until("VERSION:").await;
    let (major, minor) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(version) = shell.text().split("VERSION:").find_map(|tail| {
                let (major, tail) = tail.split_once('.')?;
                let minor = tail
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>();
                Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?))
            }) {
                return version;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    if major < 4 || (major == 4 && minor < 4) {
        assert_eq!(
            shell.control.inspect_activity().await.unwrap().state,
            Unknown
        );
        shell.finish().await;
        return;
    }
    shell.state(Idle).await;
    for command in ["sleep 600", "while :; do :; done", "read answer"] {
        shell.write(&format!("{command}\r")).await;
        shell.state(Busy).await;
        shell.write("\x03").await;
        shell.state(Idle).await;
    }
    assert!(!shell.text().contains("syntax error"));
    assert!(!shell.text().contains("file exists"));
    shell.finish().await;
}
