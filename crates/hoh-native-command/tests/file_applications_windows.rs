#![cfg(windows)]
mod common;
use base64::{Engine, engine::general_purpose::STANDARD};
use common::*;
use hoh_native_command::{runner::*, *};

fn literal(value: &str) -> String {
    format!(
        "[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))",
        STANDARD.encode(value)
    )
}
async fn script(text: &str) {
    let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    run_native_command(
        "powershell.exe",
        &[
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-EncodedCommand".into(),
            STANDARD.encode(utf16),
        ],
        &signal(),
        Window::Hidden,
    )
    .await
    .unwrap();
}
struct Registration {
    extension: String,
    prog_id: String,
    app_name: String,
}
impl Registration {
    fn cleanup(&self) -> String {
        format!(
            "$root = [Microsoft.Win32.Registry]::CurrentUser\n$root.DeleteSubKeyTree('Software\\Classes\\{}', $false)\n$root.DeleteSubKeyTree('Software\\Classes\\{}', $false)\n$root.DeleteSubKeyTree('Software\\Classes\\Applications\\{}', $false)",
            self.extension, self.prog_id, self.app_name
        )
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        let bytes: Vec<u8> = self
            .cleanup()
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let _ = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &STANDARD.encode(bytes),
            ])
            .output();
    }
}
#[tokio::test]
async fn queries_and_invokes_a_registered_windows_handler_through_the_system_shell() {
    let root = tempfile::tempdir().unwrap();
    let suffix = root
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .replace('.', "");
    let registration = Registration {
        extension: format!(".dsh{suffix}"),
        prog_id: format!("DSH.Test.{suffix}"),
        app_name: format!("dsh-handler-{suffix}.exe"),
    };
    let executable = root.path().join(&registration.app_name);
    let path = root
        .path()
        .join(format!("测试 ' audio{}", registration.extension));
    let marker = root.path().join("opened.txt");
    let pending = root.path().join("opened.pending");
    tokio::fs::write(&path, "test").await.unwrap();
    let quoted = |path: &std::path::Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    script(&format!("$ErrorActionPreference = 'Stop'\nAdd-Type -ReferencedAssemblies System -OutputAssembly ({}) -OutputType ConsoleApplication -TypeDefinition @'\nusing System;\nusing System.IO;\nusing System.Diagnostics;\npublic static class Handler {{\n public static void Main(string[] args) {{\n File.WriteAllLines({}, new string[] {{ args.Length == 0 ? \"(no file argument)\" : args[0], Process.GetCurrentProcess().Id.ToString() }});\n File.Move({}, {});\n }}\n}}\n'@\n", literal(executable.to_str().unwrap()), quoted(&pending), quoted(&pending), quoted(&marker))).await;
    run_native_command(
        executable.to_str().unwrap(),
        &[path.to_string_lossy().into_owned()],
        &signal(),
        Window::Hidden,
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::fs::read_to_string(&marker)
            .await
            .unwrap()
            .lines()
            .next()
            .unwrap(),
        path.to_str().unwrap()
    );
    tokio::fs::remove_file(&marker).await.unwrap();
    let command = literal(&format!("\"{}\" \"%1\"", executable.display()));
    let Registration {
        ref extension,
        ref prog_id,
        ref app_name,
    } = registration;
    script(&format!("$ErrorActionPreference = 'Stop'\n$root = [Microsoft.Win32.Registry]::CurrentUser\n$key = $root.CreateSubKey('Software\\Classes\\{extension}')\n$key.SetValue('', '{prog_id}'); $key.Dispose()\n$key = $root.CreateSubKey('Software\\Classes\\{extension}\\OpenWithProgids')\n$key.SetValue('{prog_id}', ''); $key.Dispose()\n$key = $root.CreateSubKey('Software\\Classes\\{prog_id}\\shell\\open\\command')\n$key.SetValue('', {command}); $key.Dispose()\n$key = $root.CreateSubKey('Software\\Classes\\Applications\\{app_name}\\shell\\open\\command')\n$key.SetValue('', {command}); $key.Dispose()\n$key = $root.CreateSubKey('Software\\Classes\\Applications\\{app_name}\\SupportedTypes')\n$key.SetValue('{extension}', ''); $key.Dispose()\n")).await;
    let signal = signal();
    let applications =
        native_file_applications(path.to_str().unwrap(), &signal, &Default::default())
            .await
            .unwrap();
    let expected = applications
        .iter()
        .find(|app| app.id.eq_ignore_ascii_case(executable.to_str().unwrap()))
        .unwrap();
    assert!(expected.default);
    open_native_file_application(
        path.to_str().unwrap(),
        &expected.id,
        &signal,
        &Default::default(),
    )
    .await
    .unwrap();
    let opened = tokio::time::timeout(std::time::Duration::from_secs(10), async {
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
    assert_eq!(
        tokio::fs::canonicalize(lines.next().unwrap())
            .await
            .unwrap(),
        tokio::fs::canonicalize(&path).await.unwrap()
    );
    let pid: u32 = lines.next().unwrap().parse().unwrap();
    assert!(pid > 0);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let result = run_native_command(
                "powershell.exe",
                &[
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    format!(
                        "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 1 }}"
                    ),
                ],
                &signal,
                Window::Hidden,
            )
            .await;
            match result {
                Ok(_) => break,
                Err(error) if detail(&error).code == Some(CommandCode::Exit(1)) => {
                    tokio::task::yield_now().await
                }
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap();
}
