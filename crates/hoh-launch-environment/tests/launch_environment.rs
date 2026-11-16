use std::{ffi::OsString, sync::Arc};

use LaunchEnvironmentSource::{Process, ProjectEnv, UserEnv};
use hoh_kernel::Kernel;
use hoh_launch_environment::{
    DSH_LAUNCH_ENVIRONMENT_KEY, LaunchEnvironment, LaunchEnvironmentEntry,
    LaunchEnvironmentLayerInput, LaunchEnvironmentSnapshot, LaunchEnvironmentSource,
    create_inherited_environment_layer, create_launch_environment_snapshot, launch_environment_of,
    launched_through_ssh,
};

fn layer(
    source: LaunchEnvironmentSource,
    path: Option<&str>,
    values: &[(&str, &str)],
) -> LaunchEnvironmentLayerInput {
    LaunchEnvironmentLayerInput {
        source,
        path: path.map(str::to_owned),
        values: values
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect(),
    }
}

fn entry(
    value: &str,
    source: LaunchEnvironmentSource,
    path: Option<&str>,
) -> LaunchEnvironmentEntry {
    LaunchEnvironmentEntry {
        value: value.into(),
        source,
        path: path.map(str::to_owned),
    }
}

fn layered() -> LaunchEnvironmentSnapshot {
    create_launch_environment_snapshot(&[
        layer(
            Process,
            None,
            &[("SHARED", "from-process"), ("ONLY_PROCESS", "p")],
        ),
        layer(
            ProjectEnv,
            Some("/work/.env"),
            &[("SHARED", "from-project"), ("ONLY_PROJECT", "j")],
        ),
        layer(
            UserEnv,
            Some("/home/.dsh/.env"),
            &[("SHARED", "from-user"), ("ONLY_USER", "u")],
        ),
    ])
}

macro_rules! ssh_case {
    ($test:ident, $name:literal, $source:ident) => {
        #[test]
        fn $test() {
            let snapshot = create_launch_environment_snapshot(&[layer(
                $source,
                None,
                &[($name, "ssh-marker")],
            )]);
            assert_eq!(launched_through_ssh(&snapshot), $source == Process);
        }
    };
}

ssh_case!(
    classifies_ssh_connection_from_process,
    "SSH_CONNECTION",
    Process
);
ssh_case!(
    classifies_ssh_connection_from_project_env,
    "SSH_CONNECTION",
    ProjectEnv
);
ssh_case!(
    classifies_ssh_connection_from_user_env,
    "SSH_CONNECTION",
    UserEnv
);
ssh_case!(classifies_ssh_tty_from_process, "SSH_TTY", Process);
ssh_case!(classifies_ssh_tty_from_project_env, "SSH_TTY", ProjectEnv);
ssh_case!(classifies_ssh_tty_from_user_env, "SSH_TTY", UserEnv);

fn assert_markers_local(values: &[(&str, &str)]) {
    let snapshot = create_launch_environment_snapshot(&[
        layer(Process, None, values),
        layer(ProjectEnv, None, &[("SSH_CONNECTION", "stale-connection")]),
        layer(UserEnv, None, &[("SSH_TTY", "stale-tty")]),
    ]);
    assert!(!launched_through_ssh(&snapshot));
}

#[test]
fn keeps_absent_inherited_markers_local() {
    assert_markers_local(&[]);
}

#[test]
fn keeps_empty_inherited_markers_local() {
    assert_markers_local(&[("SSH_CONNECTION", ""), ("SSH_TTY", "")]);
}

#[test]
fn resolves_across_every_layer_most_trusted_first_and_reports_the_winning_source() {
    let layered = layered();
    assert_eq!(
        layered.get("SHARED"),
        Some(entry("from-process", Process, None))
    );
    assert_eq!(
        layered.get("ONLY_PROJECT"),
        Some(entry("j", ProjectEnv, Some("/work/.env")))
    );
    assert_eq!(
        layered.get("ONLY_USER"),
        Some(entry("u", UserEnv, Some("/home/.dsh/.env")))
    );
    assert_eq!(layered.get("ABSENT"), None);
}

#[test]
fn filters_layers_without_changing_their_trust_order() {
    let layered = layered();
    assert_eq!(layered.get_from("ONLY_PROJECT", &[Process, UserEnv]), None);
    assert_eq!(
        layered.get_from("SHARED", &[UserEnv, Process]),
        Some(entry("from-process", Process, None))
    );
    assert_eq!(layered.get_from("SHARED", &[]), None);
}

#[test]
fn copies_each_layer_so_a_later_mutation_of_the_source_object_cannot_change_it() {
    let mut layers = [layer(Process, None, &[("KEY", "first")])];
    let snapshot = create_launch_environment_snapshot(&layers);
    layers[0].values.insert("KEY".into(), "second".into());
    layers[0].values.insert("LATE".into(), "added".into());
    assert_eq!(snapshot.get("KEY"), Some(entry("first", Process, None)));
    assert_eq!(snapshot.get("LATE"), None);
}

#[test]
fn keeps_an_empty_value_as_a_present_value_for_its_owner_to_judge() {
    let snapshot = create_launch_environment_snapshot(&[layer(Process, None, &[("EMPTY", "")])]);
    assert_eq!(snapshot.get("EMPTY"), Some(entry("", Process, None)));
}

#[test]
fn orders_lookups_canonically_regardless_of_construction_order() {
    let reversed = create_launch_environment_snapshot(&[
        layer(UserEnv, Some("/u"), &[("K", "u")]),
        layer(Process, None, &[("K", "p")]),
    ]);
    assert_eq!(reversed.get("K"), Some(entry("p", Process, None)));
}

#[tokio::test(start_paused = true)]
async fn returns_the_launcher_snapshot_when_the_product_cli_provided_one() {
    let kernel = Kernel::start();
    let snapshot = layered();
    assert_eq!(DSH_LAUNCH_ENVIRONMENT_KEY, "launchEnvironment");
    kernel
        .root()
        .provide::<LaunchEnvironment>(Arc::new(snapshot.clone()))
        .await
        .unwrap();
    let resolved = launch_environment_of(&kernel.root());
    for name in [
        "SHARED",
        "ONLY_PROCESS",
        "ONLY_PROJECT",
        "ONLY_USER",
        "ABSENT",
    ] {
        assert_eq!(resolved.get(name), snapshot.get(name));
    }
    kernel.shutdown().await;
}

#[test]
fn falls_back_to_the_inherited_environment_as_the_only_layer() {
    let inherited = create_inherited_environment_layer([(
        OsString::from("DSH_ENV_SPEC_FALLBACK"),
        OsString::from("ambient"),
    )]);
    assert_eq!(inherited.source, Process);
    assert_eq!(inherited.path, None);
    let snapshot = create_launch_environment_snapshot(&[inherited]);
    assert_eq!(
        snapshot.get("DSH_ENV_SPEC_FALLBACK"),
        Some(entry("ambient", Process, None))
    );
    assert_eq!(
        snapshot.get_from("DSH_ENV_SPEC_FALLBACK", &[ProjectEnv, UserEnv]),
        None
    );
}

#[test]
fn accepts_an_empty_inherited_environment() {
    let inherited = create_inherited_environment_layer([]);
    assert_eq!(inherited.source, Process);
    assert_eq!(inherited.path, None);
    assert!(inherited.values.is_empty());
    let snapshot = create_launch_environment_snapshot(&[inherited]);
    assert_eq!(snapshot.get("DSH_ENV_SPEC_FALLBACK"), None);
    assert!(!launched_through_ssh(&snapshot));
}

#[cfg(unix)]
#[test]
fn decodes_non_utf8_inherited_names_and_values_lossily_without_panicking() {
    use std::os::unix::ffi::OsStringExt;

    let name = OsString::from_vec(b"DSH_NAME_\xff".to_vec());
    let value = OsString::from_vec(b"native-value-\xff".to_vec());
    assert!(name.to_str().is_none());
    assert!(value.to_str().is_none());
    let expected_name = name.to_string_lossy().into_owned();
    let expected_value = value.to_string_lossy().into_owned();
    let inherited = create_inherited_environment_layer([(name, value)]);
    assert_eq!(inherited.values.get(&expected_name), Some(&expected_value));
    let snapshot = create_launch_environment_snapshot(&[inherited]);
    assert_eq!(
        snapshot.get(&expected_name),
        Some(entry(&expected_value, Process, None))
    );
    assert_eq!(
        snapshot.get_from(&expected_name, &[ProjectEnv, UserEnv]),
        None
    );
}

#[cfg(unix)]
#[test]
fn last_inherited_pair_wins_when_invalid_names_decode_to_the_same_string() {
    use std::os::unix::ffi::OsStringExt;

    let first = OsString::from_vec(b"DSH_COLLISION_\xff".to_vec());
    let second = OsString::from_vec(b"DSH_COLLISION_\xfe".to_vec());
    assert_ne!(first, second);
    let decoded_name = first.to_string_lossy().into_owned();
    assert_eq!(decoded_name, second.to_string_lossy());
    for (pairs, winner) in [
        (
            [
                (first.clone(), OsString::from("first")),
                (second.clone(), OsString::from("second")),
            ],
            "second",
        ),
        (
            [
                (second, OsString::from("second")),
                (first, OsString::from("first")),
            ],
            "first",
        ),
    ] {
        let inherited = create_inherited_environment_layer(pairs);
        assert_eq!(inherited.values.len(), 1);
        let snapshot = create_launch_environment_snapshot(&[inherited]);
        assert_eq!(
            snapshot.get(&decoded_name),
            Some(entry(winner, Process, None))
        );
    }
}

#[test]
fn later_layer_of_the_same_source_replaces_the_entire_earlier_layer() {
    let snapshot = create_launch_environment_snapshot(&[
        layer(Process, Some("/earlier"), &[("K", "first"), ("OLD", "old")]),
        layer(Process, Some("/later"), &[("K", "last")]),
    ]);
    assert_eq!(
        snapshot.get("K"),
        Some(entry("last", Process, Some("/later")))
    );
    assert_eq!(snapshot.get("OLD"), None);
}

#[test]
fn environment_names_follow_platform_case_matching() {
    let snapshot = create_launch_environment_snapshot(&[
        layer(Process, None, &[("key", "process")]),
        layer(ProjectEnv, None, &[("KEY", "project")]),
    ]);
    #[cfg(windows)]
    assert_eq!(snapshot.get("KEY"), Some(entry("process", Process, None)));
    #[cfg(not(windows))]
    {
        assert_eq!(
            snapshot.get("KEY"),
            Some(entry("project", ProjectEnv, None))
        );
        assert_eq!(snapshot.get("key"), Some(entry("process", Process, None)));
    }
}

#[test]
fn serialized_entries_keep_source_names_field_order_and_absent_paths() {
    assert_eq!(
        serde_json::to_string(&entry("v", ProjectEnv, Some("/p"))).unwrap(),
        r#"{"value":"v","source":"project-env","path":"/p"}"#
    );
    assert_eq!(
        serde_json::to_string(&entry("", Process, None)).unwrap(),
        r#"{"value":"","source":"process"}"#
    );
}
