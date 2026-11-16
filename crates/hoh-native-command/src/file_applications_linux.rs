use crate::{
    desktop_entry::*, host_environment::Environment, runner::*, types::NativeFileApplication,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use indexmap::IndexSet;
use std::path::{Path, PathBuf};

async fn desktop_file(root: &Path, id: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_owned()];
    while let Some(directory) = stack.pop() {
        let Ok(mut reader) = tokio::fs::read_dir(&directory).await else {
            continue;
        };
        let mut directories = Vec::new();
        while let Ok(Some(entry)) = reader.next_entry().await {
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if path
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "-")
                == id
            {
                return Some(path);
            }
        }
        stack.extend(directories.into_iter().rev());
    }
    None
}

pub async fn linux_file_applications(
    path: &str,
    signal: &dyn CommandSignal,
    run: &dyn NativeCommandRunner,
    env: &Environment,
) -> Result<Vec<NativeFileApplication>, NativeCommandError> {
    let info = run
        .run(
            "gio",
            &[
                "info".into(),
                "-a".into(),
                "standard::content-type".into(),
                path.into(),
            ],
            signal,
            Window::Hidden,
        )
        .await?;
    let mime = info
        .stdout
        .split_once("standard::content-type:")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .ok_or_else(|| NativeCommandError::message("GIO did not identify the file content type"))?;
    let result = run
        .run(
            "env",
            &["LC_ALL=C".into(), "gio".into(), "mime".into(), mime.into()],
            signal,
            Window::Hidden,
        )
        .await?;
    let preferred = result.stdout.lines().find_map(|line| {
        if !line.starts_with("Default application") {
            return None;
        }
        let (_, rest) = line.split_once(':')?;
        let rest = rest.trim();
        rest.ends_with(".desktop").then_some(rest.to_owned())
    });
    let mut ids = IndexSet::new();
    if let Some(preferred) = &preferred {
        ids.insert(preferred.clone());
    }
    for line in result.stdout.lines() {
        if line.starts_with(char::is_whitespace) && line.trim().ends_with(".desktop") {
            ids.insert(line.trim().to_owned());
        }
    }
    #[allow(deprecated)]
    let home = env.get("HOME").cloned().unwrap_or_else(|| {
        std::env::home_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let directories = desktop_data_directories(&home, env);
    let locale = env
        .get("LC_ALL")
        .or_else(|| env.get("LC_MESSAGES"))
        .or_else(|| env.get("LANG"))
        .map(String::as_str)
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("");
    let language = locale.split('_').next().unwrap_or("");
    let mut applications = Vec::new();
    for id in ids {
        signal.throw_if_aborted()?;
        let mut entry_path = None;
        for directory in &directories {
            entry_path = desktop_file(&Path::new(directory).join("applications"), &id).await;
            if entry_path.is_some() {
                break;
            }
        }
        let Some(entry_path) = entry_path else {
            continue;
        };
        let fields = desktop_entry_fields(&String::from_utf8_lossy(
            &tokio::fs::read(&entry_path).await?,
        ));
        let name = fields
            .get(&format!("Name[{locale}]"))
            .or_else(|| fields.get(&format!("Name[{language}]")))
            .or_else(|| fields.get("Name"));
        let Some(name) = name else {
            continue;
        };
        if fields.get("Hidden").is_some_and(|s| s == "true") {
            continue;
        }
        let icon = match fields.get("Icon") {
            Some(name) => desktop_application_icon(name, &directories).await,
            None => None,
        };
        applications.push(NativeFileApplication {
            id: entry_path.to_string_lossy().into_owned(),
            name: name.clone(),
            default: preferred.as_ref() == Some(&id),
            icon: icon.map(|icon| {
                format!(
                    "data:{};base64,{}",
                    icon.content_type,
                    STANDARD.encode(icon.bytes)
                )
            }),
        });
    }
    Ok(applications)
}
