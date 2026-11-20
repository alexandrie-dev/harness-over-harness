//! The session's working folder. The engine's sandbox is rooted there, so it must be the folder
//! the person chose on the "Правила" step (or the app's own default), never the home folder.
use std::path::{Path, PathBuf};

/// Name the first-run screen shows for the default folder.
pub const DEFAULT_LABEL: &str = "Документы/HoH";
const FILE: &str = "workspace.json";

/// `<Documents>/HoH`: its own folder, so a command in it cannot reach the person's other files.
pub fn default_folder(documents: &Path) -> PathBuf {
    documents.join("HoH")
}

/// The app's default folder. `<Documents>/HoH` when it can exist; when Documents is missing or broken (a dangling
/// link, a read-only parent), `<fallback_base>/HoH` instead, so the app still starts and says which folder it uses.
pub fn usable_default(documents: &Path, fallback_base: &Path) -> std::io::Result<PathBuf> {
    let preferred = default_folder(documents);
    if std::fs::create_dir_all(&preferred).is_ok() && preferred.is_dir() {
        return Ok(preferred);
    }
    let fallback = default_folder(fallback_base);
    std::fs::create_dir_all(&fallback)?;
    Ok(fallback)
}

/// The folder the person chose, as an absolute path. The first-run default label stands for the
/// default folder; any other relative text is refused because it would depend on the process folder.
pub fn resolve_choice(choice: &str, default: &Path) -> Result<PathBuf, String> {
    let choice = choice.trim();
    if choice == DEFAULT_LABEL {
        return Ok(default.to_owned());
    }
    let path = Path::new(choice);
    if !path.is_absolute() || choice.contains('\0') || path.parent().is_none() {
        return Err("Выберите папку для работы".into());
    }
    Ok(path.components().collect())
}

/// Folder remembered from the last first run, if it still is an absolute existing folder.
pub fn load(config_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read(config_dir.join(FILE)).ok()?;
    let folder = PathBuf::from(
        serde_json::from_slice::<serde_json::Value>(&text)
            .ok()?
            .get("folder")?
            .as_str()?,
    );
    (folder.is_absolute() && folder.parent().is_some() && folder.is_dir()).then_some(folder)
}

pub fn save(config_dir: &Path, folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let temporary = config_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&temporary, serde_json::json!({"folder":folder}).to_string())?;
    std::fs::rename(temporary, config_dir.join(FILE))
}

/// The folder a new engine opens its session in; the default one is created when missing.
pub fn startup_folder(config_dir: &Path, default: &Path) -> std::io::Result<PathBuf> {
    if let Some(chosen) = load(config_dir) {
        return Ok(chosen);
    }
    std::fs::create_dir_all(default)?;
    Ok(default.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("hoh-workspace-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn default_label_and_absolute_choices_resolve_everything_else_is_refused() {
        let default = Path::new("/home/u/Documents/HoH");
        assert_eq!(resolve_choice(DEFAULT_LABEL, default).unwrap(), default);
        assert_eq!(
            resolve_choice("/work/project/", default).unwrap(),
            Path::new("/work/project")
        );
        for refused in ["", "Документы/Другая", "work", "../x", "/", "/a\0b"] {
            assert!(resolve_choice(refused, default).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn a_broken_documents_link_falls_back_to_the_app_folder_instead_of_failing() {
        let base = scratch("broken");
        let documents = base.join("Documents");
        std::os::unix::fs::symlink(base.join("nowhere"), &documents).unwrap();
        let config = base.join("config");
        let folder = usable_default(&documents, &config).unwrap();
        assert_eq!(folder, config.join("HoH"));
        assert!(folder.is_dir());
        assert_eq!(startup_folder(&config, &folder).unwrap(), folder);
        // a healthy Documents keeps the usual place
        let healthy = base.join("Healthy");
        assert_eq!(
            usable_default(&healthy, &config).unwrap(),
            healthy.join("HoH")
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn first_start_uses_a_created_default_never_the_home_folder() {
        let base = scratch("first");
        let default = default_folder(&base.join("Documents"));
        let folder = startup_folder(&base.join("config"), &default).unwrap();
        assert_eq!(folder, default);
        assert!(default.is_dir());
        assert_ne!(Some(folder.as_path()), std::env::home_dir().as_deref());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_chosen_folder_is_remembered_and_a_vanished_one_falls_back() {
        let base = scratch("chosen");
        let (config, chosen) = (base.join("config"), base.join("project"));
        let default = default_folder(&base.join("Documents"));
        std::fs::create_dir_all(&chosen).unwrap();
        save(&config, &chosen).unwrap();
        assert_eq!(startup_folder(&config, &default).unwrap(), chosen);
        std::fs::remove_dir_all(&chosen).unwrap();
        assert_eq!(startup_folder(&config, &default).unwrap(), default);
        std::fs::write(config.join(FILE), r#"{"folder":"relative"}"#).unwrap();
        assert_eq!(startup_folder(&config, &default).unwrap(), default);
        std::fs::remove_dir_all(base).unwrap();
    }
}
