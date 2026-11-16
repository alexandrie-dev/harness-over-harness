use crate::runner::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use hoh_error::SharedError;
use hoh_timeout::AbortSignal;

pub async fn windows_file_applications(
    path: &str,
    application: Option<&str>,
    signal: &AbortSignal,
    run: &dyn NativeCommandRunner,
) -> Result<String, SharedError> {
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
    let directory = tokio::task::spawn_blocking(|| {
        tempfile::Builder::new()
            .prefix("dsh-native-command-")
            .tempdir()
    })
    .await
    .map_err(SharedError::new)?
    .map_err(|e| NativeCommandError::fs(e, "mkdtemp", None))?;
    let script_path = hoh_util_workspace_path::node_path::join(&[
        &directory.path().to_string_lossy(),
        "associations.ps1",
    ]);
    let write = tokio::fs::write(&script_path, script)
        .await
        .map_err(|e| SharedError::new(NativeCommandError::fs(e, "open", Some(&script_path))));
    let result = match write {
        Err(error) => Err(error),
        Ok(()) => {
            run.run(
                "powershell.exe",
                &[
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-STA".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-File".into(),
                    script_path.clone(),
                ],
                signal,
                Window::Hidden,
            )
            .await
        }
    };
    tokio::task::spawn_blocking(move || directory.close())
        .await
        .map_err(SharedError::new)?
        .map_err(|e| NativeCommandError::fs(e, "rm", None))?;
    Ok(result?.stdout)
}
