mod common;
use common::terminal_spec;
use hoh_subprocess::SubprocessTerminalActivityState::*;
use hoh_subprocess_local::shell_activity::*;
#[test]
fn accepts_only_fresh_complete_records_from_the_original_shell_and_advances_revisions_on_input() {
    let dir = tempfile::tempdir().unwrap();
    let activity = ShellActivity::new(dir.path().into(), vec![], Default::default());
    assert_eq!(activity.inspect(123).state, Unknown);
    let state = dir.path().join("state");
    std::fs::write(&state, "123:1:idle\n").unwrap();
    let idle = activity.inspect(123);
    assert_eq!(idle.state, Idle);
    assert_eq!(activity.inspect(123), idle);
    activity.invalidate();
    let invalid = activity.inspect(123);
    assert_eq!(invalid.state, Unknown);
    assert_eq!(invalid.revision, idle.revision + 1);
    for (record, expected) in [
        ("123:2:busy", Busy),
        ("999:2:idle\n", Unknown),
        ("123:3:id", Unknown),
    ] {
        std::fs::write(&state, record).unwrap();
        assert_eq!(activity.inspect(123).state, expected);
    }
    activity.dispose();
    assert_eq!(activity.inspect(123).state, Unknown);
}
#[test]
fn leaves_unsupported_executables_platforms_and_custom_launch_arguments_untouched() {
    for argv in [
        vec!["/bin/fish", "-i"],
        vec!["/bin/zsh", "-l"],
        vec!["/bin/zsh", "-l", "-i"],
    ] {
        let mut spec = terminal_spec();
        spec.argv = argv.into_iter().map(Into::into).collect();
        assert!(
            prepare_shell_activity(&spec, Default::default(), "linux")
                .unwrap()
                .is_none()
        );
    }
    let mut spec = terminal_spec();
    spec.shell_activity = Some(false);
    assert!(
        prepare_shell_activity(&spec, Default::default(), "linux")
            .unwrap()
            .is_none()
    );
    spec.shell_activity = Some(true);
    assert!(
        prepare_shell_activity(&spec, Default::default(), "win32")
            .unwrap()
            .is_none()
    );
}
#[test]
fn supports_an_unset_original_zdotdir_and_removes_private_files_on_disposal() {
    let mut spec = terminal_spec();
    spec.argv = vec!["/bin/zsh".into(), "-i".into()];
    let activity = prepare_shell_activity(&spec, Default::default(), "linux")
        .unwrap()
        .unwrap();
    let path = activity.env.get("ZDOTDIR").unwrap();
    assert!(std::path::Path::new(path).exists());
    assert!(
        std::fs::read_to_string(activity.directory.join(".zshenv"))
            .unwrap()
            .starts_with("unset ZDOTDIR\n")
    );
    activity.dispose();
    assert!(!std::path::Path::new(path).exists());
}
#[test]
fn restores_a_quoted_original_zdotdir_before_sourcing_user_startup_files() {
    let mut spec = terminal_spec();
    spec.argv = vec!["/bin/zsh".into(), "-i".into()];
    let original = "/shell config/user's $settings";
    let activity = prepare_shell_activity(
        &spec,
        [
            ("ZDOTDIR".into(), original.into()),
            ("OTHER".into(), "preserved".into()),
        ]
        .into(),
        "linux",
    )
    .unwrap()
    .unwrap();
    assert_ne!(activity.env["ZDOTDIR"], original);
    assert_eq!(activity.env["OTHER"], "preserved");
    assert_eq!(activity.argv, spec.argv);
    assert_eq!(
        std::fs::read_to_string(activity.directory.join(".zshenv"))
            .unwrap()
            .lines()
            .take(2)
            .collect::<Vec<_>>(),
        [
            "ZDOTDIR='/shell config/user'\\''s $settings'",
            "[[ ! -r ${ZDOTDIR:-$HOME}/.zshenv ]] || builtin source \"${ZDOTDIR:-$HOME}/.zshenv\""
        ]
    );
    activity.dispose();
    assert!(!activity.directory.exists());
}
