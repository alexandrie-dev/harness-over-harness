use crate::host_environment::Environment;
use hoh_util_values::js_trim;
use hoh_util_workspace_path::node_path;
use indexmap::IndexMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopIcon {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

pub fn desktop_entry_fields(text: &str) -> IndexMap<String, String> {
    let mut main = false;
    let mut fields = IndexMap::new();
    for line in text.split('\n') {
        let trimmed = js_trim(line);
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
                js_trim(&trimmed[..separator]).into(),
                js_trim(&trimmed[separator + 1..]).into(),
            );
        }
    }
    fields
}

pub fn desktop_data_directories(home: &str, env: &Environment) -> Vec<String> {
    let mut directories = vec![
        env.get("XDG_DATA_HOME")
            .cloned()
            .unwrap_or_else(|| node_path::join(&[home, ".local", "share"])),
    ];
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

async fn read_icon(path: &str) -> Option<DesktopIcon> {
    let name = path;
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
    if node_path::is_absolute(name) {
        return read_icon(name).await;
    }
    for directory in directories {
        for size in ["512x512", "256x256", "128x128", "64x64", "48x48", "32x32"] {
            for extension in ["png", "svg"] {
                if let Some(icon) = read_icon(&node_path::join(&[
                    directory,
                    "icons",
                    "hicolor",
                    size,
                    "apps",
                    &format!("{name}.{extension}"),
                ]))
                .await
                {
                    return Some(icon);
                }
            }
        }
        if let Some(icon) = read_icon(&node_path::join(&[
            directory,
            "icons",
            "hicolor",
            "scalable",
            "apps",
            &format!("{name}.svg"),
        ]))
        .await
        {
            return Some(icon);
        }
        for extension in ["png", "svg"] {
            if let Some(icon) = read_icon(&node_path::join(&[
                directory,
                "pixmaps",
                &format!("{name}.{extension}"),
            ]))
            .await
            {
                return Some(icon);
            }
        }
    }
    None
}
