//! «Add project → Local folder» of the T3 client (lead 22.11 20:43): names of subfolders only, never
//! files, and only to the main window.
use hoh_desktop::folders::{FolderEntry, list_folders};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let base = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/dev/shm".into()))
        .join(format!("hoh-folders-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    for dir in [
        "home/Projects/alpha",
        "home/Projects/Beta",
        "home/Projects/.hidden",
        "home/Pictures",
        "elsewhere",
    ] {
        fs::create_dir_all(base.join(dir)).unwrap();
    }
    fs::write(base.join("home/Projects/notes.txt"), "secret").unwrap();
    fs::write(base.join("home/Projects/alpha.txt"), "secret").unwrap();
    symlink(
        base.join("elsewhere"),
        base.join("home/Projects/link-to-elsewhere"),
    )
    .unwrap();
    base
}

fn names(entries: &[FolderEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.name.as_str()).collect()
}

#[test]
fn only_folders_are_listed_never_files_or_links() {
    let base = scratch("only");
    let home = base.join("home");
    let listing = list_folders("~/Projects/", None, &home).unwrap();
    assert_eq!(
        listing.parent_path,
        home.join("Projects").display().to_string()
    );
    // A trailing separator shows hidden folders, as in T3; files and the link are never there.
    assert_eq!(names(&listing.entries), [".hidden", "alpha", "Beta"]);
    assert_eq!(
        listing.entries[1].full_path,
        home.join("Projects/alpha").display().to_string()
    );
    let json = serde_json::to_value(&listing).unwrap();
    assert!(json["parentPath"].is_string() && json["entries"][0]["fullPath"].is_string());
    assert!(!json.to_string().contains("notes.txt") && !json.to_string().contains("alpha.txt"));
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn the_typed_tail_filters_without_case_and_hides_dot_folders() {
    let base = scratch("tail");
    let home = base.join("home");
    let projects = home.join("Projects");
    let absolute = format!("{}/a", projects.display());
    assert_eq!(
        names(&list_folders(&absolute, None, &home).unwrap().entries),
        ["alpha"]
    );
    let upper = format!("{}/B", projects.display());
    assert_eq!(
        names(&list_folders(&upper, None, &home).unwrap().entries),
        ["Beta"]
    );
    assert_eq!(
        names(&list_folders("~/P", None, &home).unwrap().entries),
        ["Pictures", "Projects"]
    );
    let dot = format!("{}/.h", projects.display());
    assert_eq!(
        names(&list_folders(&dot, None, &home).unwrap().entries),
        [".hidden"]
    );
    assert_eq!(
        list_folders("~", None, &home).unwrap().parent_path,
        home.display().to_string()
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn relative_paths_need_the_current_project_and_bad_paths_are_refused() {
    let base = scratch("relative");
    let home = base.join("home");
    let projects = home.join("Projects").display().to_string();
    assert!(
        list_folders("./a", None, &home)
            .unwrap_err()
            .contains("current project")
    );
    let listing = list_folders("../al", Some(&format!("{projects}/Beta")), &home).unwrap();
    assert_eq!(names(&listing.entries), ["alpha"]);
    assert!(
        list_folders("C:\\Users", None, &home)
            .unwrap_err()
            .contains("Windows")
    );
    assert!(list_folders(&format!("{projects}/missing/"), None, &home).is_err());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn only_the_main_window_browses_folders() {
    for label in ["approval-1", "executor", "claude", ""] {
        let result = tauri::async_runtime::block_on(hoh_desktop::native::browse_folders_for(
            label,
            "~/".into(),
            None,
        ));
        assert!(result.is_err(), "{label}");
    }
    assert!(
        tauri::async_runtime::block_on(hoh_desktop::native::browse_folders_for(
            "main",
            "/".into(),
            None
        ))
        .is_ok()
    );
}

#[test]
fn explicit_window_home_is_isolated_and_invalid_values_fail_closed() {
    let base = scratch("window-home");
    let isolated = base.join("home");
    let ambient = base.join("elsewhere");
    assert_eq!(
        hoh_desktop::native::folder_browser_home(
            Some(isolated.clone().into_os_string()),
            Some(ambient.clone().into_os_string()),
        )
        .unwrap(),
        isolated.canonicalize().unwrap()
    );
    for value in ["", "relative/home", "/definitely/missing/hoh-window-home"] {
        assert!(
            hoh_desktop::native::folder_browser_home(
                Some(value.into()),
                Some(ambient.clone().into_os_string()),
            )
            .is_err(),
            "explicit {value:?} must not fall back to ambient HOME"
        );
    }
    assert_eq!(
        hoh_desktop::native::folder_browser_home(None, Some(ambient.clone().into_os_string()))
            .unwrap(),
        ambient
    );
    fs::remove_dir_all(base).unwrap();
}
