use hoh_loader_smoke::{
    EXAMPLE_MODE_ENV, ExampleLaunchOptions, ExampleMode, SourceImport, resolve_example_launch,
    resolve_example_mode,
};
use indexmap::IndexMap;
use std::process::Command;

const SRC_BIN: &str = "/repo/apps/cli/src/bin.ts";
const TSCONFIG: &str = "/repo/tsconfig.json";
const FIXTURE: &str = env!("CARGO_BIN_EXE_loader-smoke-fixture");

fn options(mode: ExampleMode) -> ExampleLaunchOptions {
    ExampleLaunchOptions {
        src_bin: SRC_BIN.into(),
        mode: Some(mode),
        ..Default::default()
    }
}

#[test]
fn defaults_absent_empty_src_to_src() {
    assert_eq!(resolve_example_mode(Some("")).unwrap(), ExampleMode::Src);
    assert_eq!(resolve_example_mode(Some("src")).unwrap(), ExampleMode::Src);
    let output = Command::new(FIXTURE)
        .env("LOADER_SMOKE_RESOLVE_MODE", "1")
        .env_remove(EXAMPLE_MODE_ENV)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "src\n");
}

#[test]
fn accepts_lib() {
    assert_eq!(resolve_example_mode(Some("lib")).unwrap(), ExampleMode::Lib);
}

#[test]
fn throws_on_any_other_value() {
    assert!(
        resolve_example_mode(Some("prod"))
            .unwrap_err()
            .to_string()
            .contains("must be 'src' or 'lib'")
    );
}

#[test]
fn reads_the_environment_when_no_argument_is_given() {
    let output = Command::new(FIXTURE)
        .env("LOADER_SMOKE_RESOLVE_MODE", "1")
        .env(EXAMPLE_MODE_ENV, "lib")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "lib\n");
}

#[test]
fn src_mode_selects_source_bin_and_tsconfig_environment() {
    let launch = resolve_example_launch(ExampleLaunchOptions {
        config_args: vec!["./cordis.yml".into()],
        tsconfig_path: Some(TSCONFIG.into()),
        ..options(ExampleMode::Src)
    })
    .unwrap();
    assert_eq!(launch.command, SRC_BIN);
    assert_eq!(launch.args.last().unwrap(), "./cordis.yml");
    assert_eq!(launch.env["TSX_TSCONFIG_PATH"].as_deref(), Some(TSCONFIG));
}

#[test]
fn src_mode_throws_without_a_tsconfig_path() {
    assert!(
        resolve_example_launch(options(ExampleMode::Src))
            .unwrap_err()
            .to_string()
            .contains("needs tsconfigPath")
    );
}

#[test]
fn src_mode_accepts_an_app_specific_source_import_hook() {
    let launch = resolve_example_launch(ExampleLaunchOptions {
        tsconfig_path: Some(TSCONFIG.into()),
        source_import: Some(SourceImport::TsxEsm),
        ..options(ExampleMode::Src)
    })
    .unwrap();
    assert_eq!(launch.command, SRC_BIN);
}

#[test]
fn lib_mode_selects_derived_bin_without_source_paths_environment() {
    let mut env = IndexMap::new();
    env.insert("DSH_HOME".into(), Some("/tmp/home".into()));
    let launch = resolve_example_launch(ExampleLaunchOptions {
        config_args: vec!["--config".into(), "./cordis.yml".into()],
        env,
        ..options(ExampleMode::Lib)
    })
    .unwrap();
    assert_eq!(launch.command, "/repo/apps/cli/lib/bin.js");
    assert_eq!(launch.args, ["--config", "./cordis.yml"]);
    assert!(!launch.env.contains_key("TSX_TSCONFIG_PATH"));
    assert_eq!(launch.env["DSH_HOME"].as_deref(), Some("/tmp/home"));
}

#[test]
fn lib_mode_uses_an_explicit_bin_when_provided() {
    let fixture = "/repo/fixture";
    let launch = resolve_example_launch(ExampleLaunchOptions {
        src_bin: fixture.into(),
        lib_bin: Some(fixture.into()),
        mode: Some(ExampleMode::Lib),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(launch.command, fixture);
}

#[test]
fn lib_mode_rewrites_only_the_last_src_segment() {
    let launch = resolve_example_launch(ExampleLaunchOptions {
        src_bin: "/repo/src/apps/cli/src/bin.ts".into(),
        mode: Some(ExampleMode::Lib),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(launch.command, "/repo/src/apps/cli/lib/bin.js");
}

#[test]
fn lib_mode_derives_the_built_bin_from_a_windows_source_path() {
    let launch = resolve_example_launch(ExampleLaunchOptions {
        src_bin: r"D:\repo\src\apps\cli\src\bin.ts".into(),
        mode: Some(ExampleMode::Lib),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(launch.command, r"D:\repo\src\apps\cli\lib\bin.js");
}

#[test]
fn lib_mode_throws_when_the_bin_has_no_src_segment() {
    let error = resolve_example_launch(ExampleLaunchOptions {
        src_bin: "/repo/lib/bin.js".into(),
        mode: Some(ExampleMode::Lib),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.to_string().contains("\"/src/\" segment"));
}

#[test]
fn defaults_the_mode_from_the_environment() {
    let output = Command::new(FIXTURE)
        .env("LOADER_SMOKE_RESOLVE_LAUNCH", "1")
        .env(EXAMPLE_MODE_ENV, "lib")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "/repo/apps/cli/lib/bin.js\n"
    );
}

#[test]
fn launch_clears_proxy_environment_before_caller_overrides() {
    let mut env = IndexMap::new();
    env.insert("HTTP_PROXY".into(), Some("http://fixture.invalid".into()));
    let launch = resolve_example_launch(ExampleLaunchOptions {
        env,
        ..options(ExampleMode::Lib)
    })
    .unwrap();
    assert_eq!(
        launch.env["HTTP_PROXY"].as_deref(),
        Some("http://fixture.invalid")
    );
    assert_eq!(launch.env["HTTPS_PROXY"], None);
}
