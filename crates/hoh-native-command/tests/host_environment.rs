mod common;
use common::*;
use hoh_native_command::{host_environment::platform, *};

#[test]
fn classifies_platform_environment_and_release() {
    for (platform, markers, release, expected) in [
        (
            "win32",
            env(&[("WSL_DISTRO_NAME", "Ubuntu")]),
            "microsoft",
            false,
        ),
        (
            "darwin",
            env(&[("WSL_INTEROP", "/run/WSL/1_interop")]),
            "Darwin",
            false,
        ),
        (
            "linux",
            env(&[("WSL_DISTRO_NAME", "Ubuntu")]),
            "generic",
            true,
        ),
        (
            "linux",
            env(&[("WSL_INTEROP", "/run/WSL/1_interop")]),
            "generic",
            true,
        ),
        (
            "linux",
            env(&[]),
            "5.15.153.1-microsoft-standard-WSL2",
            true,
        ),
        ("linux", env(&[]), "4.4.0-19041-Microsoft", true),
        (
            "linux",
            env(&[("WSL_DISTRO_NAME", ""), ("WSL_INTEROP", "")]),
            "6.8.0-generic",
            false,
        ),
    ] {
        assert_eq!(
            is_wsl(&HostEnvironmentFacts {
                platform: Some(platform.into()),
                env: Some(markers),
                os_release: Some(release.into())
            }),
            expected
        );
    }
}
#[test]
fn uses_the_current_node_platform_when_only_wsl_markers_are_supplied() {
    assert_eq!(
        is_wsl(&HostEnvironmentFacts {
            platform: None,
            env: Some(env(&[("WSL_DISTRO_NAME", "Ubuntu")])),
            os_release: Some("generic".into())
        }),
        platform() == "linux"
    );
}
