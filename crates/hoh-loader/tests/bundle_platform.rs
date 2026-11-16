mod support;
use hoh_loader::*;
use support::*;
fn load(platform: &'static str, platforms: Option<Vec<String>>, wsl: bool) -> Profile {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["b"]);
    let mut b = bundle("b", "[]");
    b.platforms = platforms;
    let mut environment = env(root.path());
    environment.platform = platform;
    environment.var = std::sync::Arc::new(move |name| {
        (wsl && name == "WSL_INTEROP").then(|| "/run/WSL/1_interop".into())
    });
    Profile::load(
        &dir,
        &Source(vec![b]),
        &root.path().join("home"),
        &[],
        &environment,
    )
    .unwrap()
}
fn list(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|s| (*s).into()).collect())
}
#[test]
fn allows_undeclared_platforms_and_an_explicit_host() {
    for platform in ["darwin", "win32", "linux"] {
        for platforms in [None, list(&[platform]), list(&["darwin", "win32", "linux"])] {
            assert!(load(platform, platforms, false).skipped.is_empty());
        }
    }
}
#[test]
fn rejects_macos_only_bundles_on_other_hosts() {
    for platform in ["win32", "linux"] {
        let p = load(platform, list(&["darwin"]), false);
        assert_eq!(p.skipped.len(), 1);
        assert!(
            p.skipped[0]
                .reason
                .contains(&format!("the DSH Host is running {platform}"))
        );
    }
}
#[test]
fn rejects_malformed_platform_metadata() {
    for platforms in [
        list(&[]),
        list(&[""]),
        list(&["Darwin"]),
        list(&[" darwin"]),
        list(&["macos"]),
        list(&["darwin", "linxu"]),
    ] {
        let p = load("darwin", platforms, false);
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].reason.contains("must be a nonempty list"));
    }
    // Non-string metadata is excluded by Bundle's typed API before the loader sees it.
}
#[test]
fn admits_wsl_independently_of_ordinary_linux_and_native_windows() {
    assert!(
        load("linux", list(&["win32", "wsl"]), true)
            .skipped
            .is_empty()
    );
    assert_eq!(
        load("linux", list(&["win32", "wsl"]), false).skipped.len(),
        1
    );
    assert_eq!(load("win32", list(&["wsl"]), true).skipped.len(), 1);
    assert!(load("linux", list(&["linux"]), true).skipped.is_empty());
    assert_eq!(load("linux", list(&["win32"]), true).skipped.len(), 1);
}

#[test]
fn non_string_metadata_is_rejected_at_the_bundle_deserialization_boundary() {
    for platforms in [
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!("darwin"),
        serde_json::json!([1]),
        serde_json::json!(["darwin", false]),
    ] {
        let error = serde_json::from_value::<Bundle>(
            serde_json::json!({"name":"b","dir":null,"platforms":platforms,"files":[]}),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "dsh.bundle.platforms must be a nonempty list of Node platform names or wsl"
        );
    }
    assert!(
        serde_json::from_value::<Bundle>(serde_json::json!({"name":"b","dir":null,"files":[]}))
            .unwrap()
            .platforms
            .is_none()
    );
}
