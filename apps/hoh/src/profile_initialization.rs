//! Replace custom profiles by staging a complete shipped template beside the target.
use hoh_error::BoxError;
use hoh_official_bundles::PROFILE_TEMPLATES;
use std::path::{Path, PathBuf};
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ProfileReplacementError {
    pub message: String,
    pub previous: PathBuf,
    pub errors: Vec<BoxError>,
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ProfileRecreatedError {
    pub message: String,
    #[source]
    pub cause: BoxError,
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct FsError {
    message: String,
    #[source]
    source: std::io::Error,
}
pub(crate) fn fs_error(
    operation: &str,
    path: &Path,
    dest: Option<&Path>,
    source: std::io::Error,
) -> BoxError {
    Box::new(FsError {
        message: hoh_error::node_fs_error_message(
            &source,
            operation,
            path.to_str(),
            dest.and_then(Path::to_str),
        ),
        source,
    })
}
pub fn initialize_profile_from_default(
    name: &str,
    from_default_profile: &str,
    home: &Path,
    warn: &mut dyn FnMut(String),
) -> Result<(), BoxError> {
    initialize_with(
        name,
        from_default_profile,
        home,
        warn,
        &mut |from, to| std::fs::rename(from, to),
        &mut |path| std::fs::remove_dir_all(path),
    )
}
fn initialize_with(
    name: &str,
    from_default_profile: &str,
    home: &Path,
    warn: &mut dyn FnMut(String),
    rename: &mut dyn FnMut(&Path, &Path) -> std::io::Result<()>,
    remove: &mut dyn FnMut(&Path) -> std::io::Result<()>,
) -> Result<(), BoxError> {
    let dir = hoh_loader::resolve_profile_dir(name, home)?;
    let Some(template) = PROFILE_TEMPLATES.get(from_default_profile) else {
        let mut keys = PROFILE_TEMPLATES.keys().copied().collect::<Vec<_>>();
        keys.sort();
        return Err(format!(
            "dsh: unknown default profile {}; expected one of {}",
            hoh_util_values::js_json_stringify(&from_default_profile.into()),
            keys.iter()
                .map(|n| hoh_util_values::js_json_stringify(&(*n).into()))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    };
    if let Some(shipped) = PROFILE_TEMPLATES
        .keys()
        .find(|n| n.to_lowercase() == name.to_lowercase())
    {
        return Err(format!("dsh: profile {} names shipped profile {} and cannot be a custom profile target; omit --from-default-profile to use it",hoh_util_values::js_json_stringify(&name.into()),hoh_util_values::js_json_stringify(&(*shipped).into())).into());
    }
    let parent = dir.parent().ok_or("profile directory has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| fs_error("mkdir", parent, None, e))?;
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{name}.staging-"))
        .tempdir_in(parent)
        .map_err(|e| fs_error("mkdtemp", parent, None, e))?
        .keep();
    let previous = staging.join("previous");
    let result = (|| {
        let fresh = staging.join("new").join(name);
        hoh_loader::init_profile(&fresh, &template.bundles)?;
        let moved = match rename(&dir, &previous) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(fs_error("rename", &dir, Some(&previous), e)),
        };
        if let Err(error) = rename(&fresh, &dir) {
            let error = fs_error("rename", &fresh, Some(&dir), error);
            if dir.exists() {
                if previous.exists() {
                    remove(&previous).map_err(|e| fs_error("rm", &previous, None, e))?;
                }
                return Err(Box::new(ProfileRecreatedError {
                    message: format!(
                        "dsh: profile {} was recreated concurrently at {}",
                        hoh_util_values::js_json_stringify(&name.into()),
                        dir.display()
                    ),
                    cause: error,
                }) as BoxError);
            }
            if moved && let Err(restore) = rename(&previous, &dir) {
                return Err(Box::new(ProfileReplacementError {
                    message: format!(
                        "dsh: profile replacement failed and {} could not be restored from {}",
                        dir.display(),
                        previous.display()
                    ),
                    previous: previous.clone(),
                    errors: vec![error, fs_error("rename", &previous, Some(&dir), restore)],
                }) as BoxError);
            }
            return Err(error);
        }
        Ok(())
    })();
    if let Err(error) = result {
        if !previous.exists() {
            let _ = remove(&staging);
        }
        return Err(error);
    }
    if let Err(error) = remove(&staging) {
        warn(format!(
            "dsh: replaced profile {} but could not remove {}: Error: {}\n",
            hoh_util_values::js_json_stringify(&name.into()),
            staging.display(),
            hoh_error::node_fs_error_message(&error, "rm", staging.to_str(), None)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn names(path: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
    #[test]
    fn restores_the_previous_target_when_the_fresh_copy_cannot_be_moved_into_place() {
        let home = tempfile::tempdir().unwrap();
        let dir = hoh_loader::resolve_profile_dir("rescue", home.path()).unwrap();
        hoh_loader::init_profile(&dir, &["existing-bundle".into()]).unwrap();
        let before = std::fs::read(dir.join("package.json")).unwrap();
        let error = initialize_with(
            "rescue",
            "web",
            home.path(),
            &mut |_| panic!("warning"),
            &mut |from, to| {
                if to == dir && from.ends_with("new/rescue") {
                    Err(std::io::Error::other("rename failed"))
                } else {
                    std::fs::rename(from, to)
                }
            },
            &mut |p| std::fs::remove_dir_all(p),
        )
        .unwrap_err();
        assert!(error.to_string().contains("rename failed"));
        assert_eq!(std::fs::read(dir.join("package.json")).unwrap(), before);
        assert_eq!(names(dir.parent().unwrap()), ["rescue"]);
    }
    #[test]
    fn keeps_the_previous_target_in_staging_when_restoring_it_also_fails() {
        let home = tempfile::tempdir().unwrap();
        let dir = hoh_loader::resolve_profile_dir("rescue", home.path()).unwrap();
        hoh_loader::init_profile(&dir, &["existing-bundle".into()]).unwrap();
        let before = std::fs::read(dir.join("package.json")).unwrap();
        let error = initialize_with(
            "rescue",
            "web",
            home.path(),
            &mut |_| panic!("warning"),
            &mut |from, to| {
                if to == dir {
                    Err(std::io::Error::other("rename failed"))
                } else {
                    std::fs::rename(from, to)
                }
            },
            &mut |p| std::fs::remove_dir_all(p),
        )
        .unwrap_err();
        let failure = error.downcast_ref::<ProfileReplacementError>().unwrap();
        assert_eq!(failure.errors.len(), 2);
        assert!(
            failure
                .message
                .contains(&failure.previous.to_string_lossy().to_string())
        );
        assert_eq!(
            std::fs::read(failure.previous.join("package.json")).unwrap(),
            before
        );
        let names = names(dir.parent().unwrap());
        assert_eq!(names.len(), 1);
        assert!(names[0].starts_with(".rescue.staging-"));
    }
    #[test]
    fn reports_a_target_recreated_between_the_two_renames_and_keeps_that_copy() {
        let home = tempfile::tempdir().unwrap();
        let dir = hoh_loader::resolve_profile_dir("rescue", home.path()).unwrap();
        hoh_loader::init_profile(&dir, &["existing-bundle".into()]).unwrap();
        let mut recreated = false;
        let error = initialize_with(
            "rescue",
            "web",
            home.path(),
            &mut |_| panic!("warning"),
            &mut |from, to| {
                if to == dir && !recreated {
                    recreated = true;
                    hoh_loader::init_profile(&dir, &["concurrent-bundle".into()]).unwrap();
                }
                std::fs::rename(from, to)
            },
            &mut |p| std::fs::remove_dir_all(p),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("profile \"rescue\" was recreated concurrently")
        );
        assert!(error.downcast_ref::<ProfileRecreatedError>().is_some());
        assert_eq!(
            hoh_loader::read_profile_manifest("test", &dir).unwrap()["dsh"]["profile"]["bundles"],
            serde_json::json!(["concurrent-bundle"])
        );
        assert_eq!(names(dir.parent().unwrap()), ["rescue"]);
    }
}
