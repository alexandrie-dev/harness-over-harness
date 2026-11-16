use crate::host_environment::Environment;
use indexmap::IndexMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopIcon {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

pub fn desktop_entry_fields(text: &str) -> IndexMap<String, String> {
    let mut main = false;
    let mut fields = IndexMap::new();
    for line in text.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            main = trimmed == "[Desktop Entry]";
            continue;
        }
        if !main || trimmed.starts_with('#') {
            continue;
        }
        if let Some(separator) = trimmed.find('=')
            && separator > 0
        {
            fields.insert(
                trimmed[..separator].trim().into(),
                trimmed[separator + 1..].trim().into(),
            );
        }
    }
    fields
}

pub fn desktop_data_directories(home: &str, env: &Environment) -> Vec<String> {
    let mut directories = vec![env.get("XDG_DATA_HOME").cloned().unwrap_or_else(|| {
        Path::new(home)
            .join(".local/share")
            .to_string_lossy()
            .into_owned()
    })];
    directories.extend(
        env.get("XDG_DATA_DIRS")
            .map(String::as_str)
            .unwrap_or("/usr/local/share:/usr/share")
            .split(':')
            .filter(|s| !s.is_empty())
            .map(String::from),
    );
    directories
}

async fn read_icon(path: &Path) -> Option<DesktopIcon> {
    let name = path.to_string_lossy();
    let content_type = if name.ends_with(".png") {
        "image/png"
    } else if name.ends_with(".svg") {
        "image/svg+xml"
    } else {
        return None;
    };
    if !tokio::fs::metadata(path).await.ok()?.is_file() {
        return None;
    }
    Some(DesktopIcon {
        bytes: tokio::fs::read(path).await.ok()?,
        content_type,
    })
}

pub async fn desktop_application_icon(name: &str, directories: &[String]) -> Option<DesktopIcon> {
    if Path::new(name).is_absolute() {
        return read_icon(Path::new(name)).await;
    }
    for directory in directories {
        for size in ["512x512", "256x256", "128x128", "64x64", "48x48", "32x32"] {
            for extension in ["png", "svg"] {
                if let Some(icon) = read_icon(
                    &Path::new(directory)
                        .join("icons/hicolor")
                        .join(size)
                        .join("apps")
                        .join(format!("{name}.{extension}")),
                )
                .await
                {
                    return Some(icon);
                }
            }
        }
        if let Some(icon) = read_icon(
            &Path::new(directory)
                .join("icons/hicolor/scalable/apps")
                .join(format!("{name}.svg")),
        )
        .await
        {
            return Some(icon);
        }
        for extension in ["png", "svg"] {
            if let Some(icon) = read_icon(
                &Path::new(directory)
                    .join("pixmaps")
                    .join(format!("{name}.{extension}")),
            )
            .await
            {
                return Some(icon);
            }
        }
    }
    None
}
