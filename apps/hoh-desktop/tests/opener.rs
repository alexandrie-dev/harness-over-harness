use hoh_desktop::opener::{Kind, check_file, check_url, clicked_link, resolve};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let base = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/dev/shm".into()))
        .join(format!("hoh-opener-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(base.join("work/sub")).unwrap();
    fs::create_dir_all(base.join("outside")).unwrap();
    fs::write(base.join("work/sub/a.txt"), "a").unwrap();
    fs::write(base.join("outside/secret.txt"), "s").unwrap();
    base
}

#[test]
fn only_http_and_https_addresses_pass() {
    for ok in [
        "http://example.com/a",
        "https://example.com/a?b=1#c",
        "HTTPS://Example.com",
    ] {
        assert!(check_url(ok).is_ok(), "{ok}");
    }
    for bad in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,x",
        "ftp://example.com",
        "http://",
        "https://user:pw@example.com",
        "https://exa mple.com",
        "https://example.com/\nx",
        "//example.com",
        "-oProxy",
        "",
    ] {
        assert!(check_url(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_file_inside_the_folder_opens_by_its_canonical_path() {
    let base = scratch("inside");
    let folder = base.join("work");
    let real = fs::canonicalize(folder.join("sub/a.txt")).unwrap();
    assert_eq!(check_file(&folder, "sub/a.txt").unwrap(), real);
    assert_eq!(check_file(&folder, real.to_str().unwrap()).unwrap(), real);
    let reveal = resolve(Kind::Reveal, "sub/a.txt", &folder).unwrap();
    assert_eq!(PathBuf::from(reveal), real.parent().unwrap());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn files_outside_the_folder_are_refused() {
    let base = scratch("outside");
    let folder = base.join("work");
    let secret = base.join("outside/secret.txt");
    for bad in [
        "../outside/secret.txt",
        "sub/../../outside/secret.txt",
        secret.to_str().unwrap(),
        "file:///etc/passwd",
        "javascript:alert(1)",
        "",
        "missing.txt",
    ] {
        assert!(check_file(&folder, bad).is_err(), "{bad:?}");
    }
    // `work-evil` shares the textual prefix of `work` but is not inside it.
    fs::create_dir_all(base.join("work-evil")).unwrap();
    fs::write(base.join("work-evil/x.txt"), "x").unwrap();
    assert!(check_file(&folder, base.join("work-evil/x.txt").to_str().unwrap()).is_err());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_symlink_out_of_the_folder_is_refused_but_one_inside_is_fine() {
    let base = scratch("link");
    let folder = base.join("work");
    symlink(base.join("outside/secret.txt"), folder.join("leak.txt")).unwrap();
    symlink(base.join("outside"), folder.join("leakdir")).unwrap();
    symlink(folder.join("sub/a.txt"), folder.join("same.txt")).unwrap();
    assert!(check_file(&folder, "leak.txt").is_err());
    assert!(check_file(&folder, "leakdir/secret.txt").is_err());
    assert!(check_file(&folder, "same.txt").is_ok());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn the_url_kind_never_reads_the_folder_and_unknown_kinds_fail() {
    assert!(resolve(Kind::Url, "https://example.com", &PathBuf::new()).is_ok());
    assert!(resolve(Kind::Url, "file:///etc/passwd", &PathBuf::new()).is_err());
    assert!(Kind::parse("shell").is_err());
}

#[test]
fn a_bare_name_is_found_below_the_folder_only_when_it_is_unique() {
    let base = scratch("name");
    let folder = base.join("work");
    let real = fs::canonicalize(folder.join("sub/a.txt")).unwrap();
    assert_eq!(check_file(&folder, "a.txt").unwrap(), real);
    fs::create_dir_all(folder.join("other")).unwrap();
    fs::write(folder.join("other/a.txt"), "again").unwrap();
    assert!(
        check_file(&folder, "a.txt").is_err(),
        "two files with one name are not guessed"
    );
    // a file reached only through a symlink to the outside is never found by name
    symlink(base.join("outside"), folder.join("link")).unwrap();
    assert!(check_file(&folder, "secret.txt").is_err());
    assert!(check_file(&folder, "nothing.txt").is_err());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_link_opens_in_the_system_browser_only_after_a_click_and_only_http() {
    let clicked = |url: &str| clicked_link(url, true);
    assert_eq!(
        clicked("https://example.com/docs"),
        Some("https://example.com/docs".into())
    );
    assert_eq!(
        clicked("http://example.com"),
        Some("http://example.com".into())
    );
    // a script on its own (no click, no key press) opens nothing, even a valid address
    assert_eq!(clicked_link("https://example.com/docs", false), None);
    for bad in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,x",
        "tauri://localhost/index.html",
        "about:blank",
        "https://user:secret@example.com/",
        "https://example.com/a b",
        "",
    ] {
        assert_eq!(clicked(bad), None, "{bad}");
    }
}
