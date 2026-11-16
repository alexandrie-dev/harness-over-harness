#![cfg(target_os = "linux")]
mod common;
use common::*;
use hoh_native_command::{host_environment::Environment, runner::*, *};
use std::os::unix::fs::PermissionsExt;

struct GioRunner {
    env: Environment,
}
impl NativeCommandRunner for GioRunner {
    fn run<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        signal: &'a dyn CommandSignal,
        _window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, NativeCommandError>> {
        Box::pin(async move {
            signal.throw_if_aborted()?;
            let result = tokio::process::Command::new(command)
                .args(args)
                .envs(&self.env)
                .kill_on_drop(true)
                .output()
                .await?;
            if !result.status.success() {
                return Err(NativeCommandError::message(String::from_utf8_lossy(
                    &result.stderr,
                )));
            }
            Ok(CommandOutput {
                stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            })
        })
    }
}
fn quoted(value: &str) -> String {
    let mut text = String::from("\"");
    for c in value.chars() {
        if "\\\"`$".contains(c) {
            text.push('\\');
        }
        text.push(c);
    }
    text.push('"');
    text
}
#[tokio::test]
async fn queries_and_launches_an_installed_linux_desktop_entry_through_gio() {
    if tokio::process::Command::new("gio")
        .arg("help")
        .output()
        .await
        .is_err()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let config = root.path().join("config");
    let empty = root.path().join("empty");
    tokio::fs::create_dir_all(data.join("applications"))
        .await
        .unwrap();
    tokio::fs::create_dir(&config).await.unwrap();
    tokio::fs::create_dir(&empty).await.unwrap();
    let path = root.path().join("测试 ' file.txt");
    let script = root.path().join("handler.sh");
    let marker = root.path().join("opened.txt");
    let pending = root.path().join("opened.pending");
    tokio::fs::write(&path, "plain text").await.unwrap();
    // Marker paths are arguments, so no fixture data becomes executable shell text.
    tokio::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n%s\\n' \"$3\" \"$$\" > \"$1\"\nmv \"$1\" \"$2\"\n",
    )
    .await
    .unwrap();
    tokio::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .await
        .unwrap();
    let desktop = data.join("applications/dsh-test.desktop");
    tokio::fs::write(&desktop,format!("[Desktop Entry]\nType=Application\nName=DSH Test Handler\nExec={} {} {} %f\nMimeType=text/plain;\n",quoted(script.to_str().unwrap()),quoted(pending.to_str().unwrap()),quoted(marker.to_str().unwrap()))).await.unwrap();
    tokio::fs::write(
        data.join("applications/mimeinfo.cache"),
        "[MIME Cache]\ntext/plain=dsh-test.desktop;\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        config.join("mimeapps.list"),
        "[Default Applications]\ntext/plain=dsh-test.desktop;\n",
    )
    .await
    .unwrap();
    let mut environment: Environment = std::env::vars().collect();
    for (key, value) in [
        ("XDG_DATA_HOME", &data),
        ("XDG_DATA_DIRS", &empty),
        ("XDG_CONFIG_HOME", &config),
        ("XDG_CONFIG_DIRS", &empty),
    ] {
        environment.insert(key.into(), value.to_string_lossy().into_owned());
    }
    environment.insert("LC_ALL".into(), "C".into());
    let run = GioRunner {
        env: environment.clone(),
    };
    let mut facts = facts("linux", &run);
    facts.env = Some(environment);
    let signal = Signal::default();
    let apps = native_file_applications(path.to_str().unwrap(), &signal, &facts)
        .await
        .unwrap();
    assert!(apps.contains(&NativeFileApplication {
        id: desktop.to_string_lossy().into_owned(),
        name: "DSH Test Handler".into(),
        default: true,
        icon: None
    }));
    open_native_file_application(
        path.to_str().unwrap(),
        desktop.to_str().unwrap(),
        &signal,
        &facts,
    )
    .await
    .unwrap();
    let opened = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&marker).await {
                break text;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut lines = opened.lines();
    assert_eq!(lines.next().unwrap(), path.to_str().unwrap());
    let pid = rustix::process::Pid::from_raw(lines.next().unwrap().parse().unwrap()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match rustix::process::test_kill_process(pid) {
                Err(rustix::io::Errno::SRCH) => break,
                Ok(()) => tokio::task::yield_now().await,
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .unwrap();
}
