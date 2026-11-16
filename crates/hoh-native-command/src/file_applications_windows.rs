use crate::runner::*;
use base64::{Engine, engine::general_purpose::STANDARD};

pub async fn windows_file_applications(
    path: &str,
    application: Option<&str>,
    signal: &dyn CommandSignal,
    run: &dyn NativeCommandRunner,
) -> Result<String, NativeCommandError> {
    let encoded_path = STANDARD.encode(path);
    let encoded_application = STANDARD.encode(application.unwrap_or(""));
    let operation = if application.is_none() {
        "ConvertTo-Json -InputObject @([DshFileAssociations]::List($path)) -Depth 4 -Compress"
    } else {
        "[DshFileAssociations]::Open($path, $application)"
    };
    let script = format!(
        "\u{feff}$ErrorActionPreference = 'Stop'\n[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)\nAdd-Type -ReferencedAssemblies System,System.Core,System.Drawing -TypeDefinition @'\n{}\n'@\n$path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded_path}'))\n$application = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded_application}'))\n{operation}\n",
        include_str!("windows_associations.cs").trim_end_matches('\n')
    );
    let directory = tempfile::Builder::new()
        .prefix("dsh-native-command-")
        .tempdir()?;
    let script_path = directory.path().join("associations.ps1");
    tokio::fs::write(&script_path, script).await?;
    let result = run
        .run(
            "powershell.exe",
            &[
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-STA".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script_path.to_string_lossy().into_owned(),
            ],
            signal,
            Window::Hidden,
        )
        .await;
    directory.close()?;
    Ok(result?.stdout)
}
