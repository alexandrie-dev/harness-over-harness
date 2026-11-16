use crate::{
    desktop_entry::*, host_environment::Environment, runner::*, types::NativeFileApplication,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hoh_error::SharedError;
use hoh_timeout::AbortSignal;
use hoh_util_values::{JS_WHITE_SPACE_SET, js_trim};
use hoh_util_workspace_path::node_path;
use indexmap::IndexSet;
use regex::Regex;

async fn desktop_file(root: &str, id: &str) -> Option<String> {
    let cwd = std::env::current_dir().ok()?.to_string_lossy().into_owned();
    let mut stack = vec![root.to_owned()];
    while let Some(directory) = stack.pop() {
        let Ok(mut reader) = tokio::fs::read_dir(&directory).await else {
            continue;
        };
        let mut entries = Vec::new();
        while let Ok(Some(entry)) = reader.next_entry().await {
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            entries.push((entry.file_name(), kind.is_dir()));
        }
        #[cfg(unix)]
        entries.sort_by(|(a, _), (b, _)| {
            use std::os::unix::ffi::OsStrExt;
            a.as_bytes().cmp(b.as_bytes())
        });
        #[cfg(not(unix))]
        entries.sort_by(|(a, _), (b, _)| {
            a.to_string_lossy()
                .as_bytes()
                .cmp(b.to_string_lossy().as_bytes())
        });
        // Node scans the complete sorted list for files before descending into directories.
        for (name, directory_entry) in &entries {
            if !directory_entry {
                let path = node_path::join(&[&directory, &name.to_string_lossy()]);
                if node_path::relative(&cwd, root, &path).replace('/', "-") == id {
                    return Some(path);
                }
            }
        }
        stack.extend(
            entries
                .into_iter()
                .rev()
                .filter(|(_, is_dir)| *is_dir)
                .map(|(name, _)| node_path::join(&[&directory, &name.to_string_lossy()])),
        );
    }
    None
}

pub async fn linux_file_applications(
    path: &str,
    signal: &AbortSignal,
    run: &dyn NativeCommandRunner,
    env: &Environment,
) -> Result<Vec<NativeFileApplication>, SharedError> {
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
    // Fixed source expressions; the shared class is JavaScript's \s, not regex's Unicode \s.
    let content_type = Regex::new(&format!(
        r"standard::content-type:[{JS_WHITE_SPACE_SET}]*([^{JS_WHITE_SPACE_SET}]+)"
    ))
    .expect("fixed regex");
    let mime = content_type
        .captures(&info.stdout)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .ok_or_else(|| NativeCommandError::message("GIO did not identify the file content type"))?;
    let result = run
        .run(
            "env",
            &["LC_ALL=C".into(), "gio".into(), "mime".into(), mime.into()],
            signal,
            Window::Hidden,
        )
        .await?;
    let dot = r"[^\n\r\u{2028}\u{2029}]";
    let line_boundary = r"[\r\u{2028}\u{2029}]";
    // JavaScript's multiline anchors also see CR, U+2028 and U+2029 as line boundaries.
    let preferred_pattern = Regex::new(&format!(r"(?m)(?:^|{line_boundary})Default application{dot}*:[{JS_WHITE_SPACE_SET}]*({dot}+\.desktop)[{JS_WHITE_SPACE_SET}]*(?:$|{line_boundary})")).expect("fixed regex");
    let registered_pattern = Regex::new(&format!(
        r"^[{JS_WHITE_SPACE_SET}]+{dot}*\.desktop[{JS_WHITE_SPACE_SET}]*$"
    ))
    .expect("fixed regex");
    let preferred = preferred_pattern
        .captures(&result.stdout)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned());
    let mut ids = IndexSet::new();
    if let Some(preferred) = &preferred {
        ids.insert(preferred.clone());
    }
    // Source split(/\r?\n/) has no special treatment for other JavaScript line terminators.
    for line in result
        .stdout
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
    {
        if registered_pattern.is_match(line) {
            ids.insert(js_trim(line).to_owned());
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
            entry_path = desktop_file(&node_path::join(&[directory, "applications"]), &id).await;
            if entry_path.is_some() {
                break;
            }
        }
        let Some(entry_path) = entry_path else {
            continue;
        };
        let fields = desktop_entry_fields(&String::from_utf8_lossy(
            &tokio::fs::read(&entry_path)
                .await
                .map_err(|e| NativeCommandError::fs(e, "open", Some(&entry_path)))?,
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
            id: entry_path,
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
