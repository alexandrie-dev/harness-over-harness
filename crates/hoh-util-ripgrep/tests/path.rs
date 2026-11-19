use std::collections::HashSet;

#[path = "../src/path.rs"]
mod path;

use path::{Resolution, resolve_from, sidecar_path, unpack_asar};

#[test]
fn uses_the_native_sidecar_beside_the_current_executable() {
    let expected = "/runtime/dsh-rg";
    assert_eq!(sidecar_path("/runtime/dsh", false), expected);
    assert_eq!(
        resolve_from(
            Resolution {
                executable: "/runtime/dsh",
                windows: false,
                packaged: true,
                electron: false,
                dependency: Some("/dependency/rg"),
            },
            |path| path == expected,
        )
        .unwrap(),
        expected
    );
}

#[test]
fn uses_a_conventional_executable_name_for_the_windows_ripgrep_sidecar() {
    let executable = r"C:\runtime\deepseek-harness-sdk-runtime-win-x64.exe";
    let expected = r"C:\runtime\deepseek-harness-sdk-runtime-win-x64-rg.exe";
    assert_eq!(sidecar_path(executable, true), expected);
    assert_eq!(
        resolve_from(
            Resolution {
                executable,
                windows: true,
                packaged: true,
                electron: false,
                dependency: Some(r"C:\dependency\rg.exe"),
            },
            |path| path == expected,
        )
        .unwrap(),
        expected
    );
}

#[test]
fn preserves_windows_executable_names_made_only_of_a_dot_prefix() {
    assert_eq!(
        sidecar_path(r"C:\runtime\.dsh", true),
        r"C:\runtime\.dsh-rg.exe"
    );
    assert_eq!(
        sidecar_path(r"C:\runtime\..", true),
        r"C:\runtime\..-rg.exe"
    );
}

#[test]
fn uses_the_dependency_binary_in_an_ordinary_runtime() {
    let dependency = "/node_modules/@vscode/ripgrep/bin/rg";
    let mut checked = HashSet::new();
    let resolved = resolve_from(
        Resolution {
            executable: "/usr/bin/node",
            windows: false,
            packaged: false,
            electron: false,
            dependency: Some(dependency),
        },
        |path| checked.insert(path.to_owned()),
    )
    .unwrap();
    assert_eq!(resolved, dependency);
    assert!(checked.is_empty());
}

#[test]
fn uses_the_dependency_binary_when_a_packaged_runtime_has_no_sidecar() {
    let dependency = "/node_modules/@vscode/ripgrep/bin/rg";
    let expected_sidecar = "/runtime/dsh-rg";
    let mut checked = Vec::new();
    let resolved = resolve_from(
        Resolution {
            executable: "/runtime/dsh",
            windows: false,
            packaged: true,
            electron: false,
            dependency: Some(dependency),
        },
        |path| {
            checked.push(path.to_owned());
            false
        },
    )
    .unwrap();
    assert_eq!(resolved, dependency);
    assert_eq!(checked, [expected_sidecar]);
}

#[test]
fn uses_the_unpacked_executable_path_for_an_electron_asar_dependency() {
    let dependency = "/Applications/DeepSeek Harness.app/Contents/Resources/app.asar/dsh/node_modules/@vscode/ripgrep/bin/rg";
    let expected = "/Applications/DeepSeek Harness.app/Contents/Resources/app.asar.unpacked/dsh/node_modules/@vscode/ripgrep/bin/rg";
    assert_eq!(unpack_asar(dependency), expected);
    assert_eq!(
        resolve_from(
            Resolution {
                executable: "/Applications/DeepSeek Harness.app/Contents/MacOS/node",
                windows: false,
                packaged: false,
                electron: true,
                dependency: Some(dependency),
            },
            |_| false,
        )
        .unwrap(),
        expected
    );
}

#[test]
fn fails_loud_without_a_packaged_sidecar_or_dependency_binary() {
    let error = resolve_from(
        Resolution {
            executable: "/runtime/hoh",
            windows: false,
            packaged: true,
            electron: false,
            dependency: None,
        },
        |_| false,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "packaged ripgrep executable was not found at /runtime/hoh-rg"
    );
}
