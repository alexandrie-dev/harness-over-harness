//! app-boot.spec.ts #1–33; every filesystem/environment input is private to a test.
use hoh_launch_environment::LaunchEnvironmentSource;
use hoh_loader::cmdline::Subscription;
use hoh_loader::fail_loud::*;
use hoh_loader::{load_env, load_layered_env, resolve_config_path};
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};
const BIN: &str = "dsh-test-bin";
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
#[test]
fn resolves_relative_to_the_given_cwd_outside_replay_mode() {
    assert_eq!(
        resolve_config_path("./cordis.yml", None, "/base"),
        std::path::Path::new("/base/cordis.yml")
    );
    assert_eq!(
        resolve_config_path("conf/app.yaml", Some("record"), "/base"),
        std::path::Path::new("/base/conf/app.yaml")
    );
}
#[test]
fn swaps_a_cordis_yml_yaml_basename_for_cordis_snapshot_yml_in_replay_mode() {
    for input in ["./cordis.yml", "deep/cordis.yaml"] {
        assert!(
            resolve_config_path(input, Some("replay"), "/base").ends_with("cordis.snapshot.yml")
        );
    }
}
#[test]
fn leaves_a_non_cordis_basename_alone_in_replay_mode_and_defaults_cwd_to_the_process_cwd() {
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_config_path("custom.yml", Some("replay"), cwd.path().to_str().unwrap()),
        cwd.path().join("custom.yml")
    );
    assert_eq!(
        resolve_config_path("./x.yml", None, cwd.path().to_str().unwrap()),
        cwd.path().join("x.yml")
    );
}
#[test]
fn loads_variables_from_env_in_the_given_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "DSH_APP_BOOT_SPEC_VAR=loaded\n").unwrap();
    let mut values = IndexMap::new();
    load_env(BIN, dir.path(), &mut values, &mut |_| {
        panic!("unexpected warning")
    });
    assert_eq!(values["DSH_APP_BOOT_SPEC_VAR"], "loaded");
}
#[test]
fn stays_silent_when_no_env_exists_ambient_environment_wins() {
    let dir = tempfile::tempdir().unwrap();
    let mut values = IndexMap::from([("X".into(), "ambient".into())]);
    load_env(BIN, dir.path(), &mut values, &mut |_| {
        panic!("unexpected warning")
    });
    assert_eq!(values["X"], "ambient");
}
#[test]
fn warns_labelled_single_line_when_env_exists_but_cannot_be_loaded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".env")).unwrap();
    let mut warnings = vec![];
    load_env(BIN, dir.path(), &mut IndexMap::new(), &mut |w| {
        warnings.push(w)
    });
    assert_eq!(
        warnings,
        vec![format!(
            "{BIN}: failed to load .env: Error: EISDIR: illegal operation on a directory, read\n"
        )]
    );
}
#[test]
fn defaults_dir_to_the_process_cwd_and_warn_to_a_stderr_write() {
    // Rust's host passes its cwd and stream explicitly; no process-global chdir/setenv in tests.
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join(".env"), "DSH_APP_BOOT_SPEC_DEFAULTS=yes\n").unwrap();
    let mut values = IndexMap::new();
    let mut stderr = String::new();
    load_env(BIN, cwd.path(), &mut values, &mut |w| stderr.push_str(&w));
    assert_eq!(values["DSH_APP_BOOT_SPEC_DEFAULTS"], "yes");
    assert!(stderr.is_empty());
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::create_dir(cwd.path().join(".env")).unwrap();
    load_env(BIN, cwd.path(), &mut values, &mut |w| stderr.push_str(&w));
    assert!(stderr.starts_with(&format!("{BIN}: failed to load .env: ")));
}
struct Envs {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
    values: IndexMap<String, String>,
}
impl Envs {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
            values: IndexMap::new(),
        }
    }
    fn files(&self, user: &str, project: &str) {
        std::fs::write(self.home.path().join(".env"), user).unwrap();
        std::fs::write(self.project.path().join(".env"), project).unwrap();
    }
    fn load(&mut self) -> hoh_launch_environment::LaunchEnvironmentSnapshot {
        load_layered_env(
            BIN,
            self.project.path(),
            self.home.path(),
            &mut self.values,
            &mut |_| panic!("unexpected warning"),
        )
        .unwrap()
    }
}
#[test]
fn layers_user_under_project_under_the_inherited_environment() {
    let mut e = Envs::new();
    e.files(
        "SHARED=user\nUSER=user-only\nINHERITED=user-loses\n",
        "SHARED=project\nPROJECT=project-only\nINHERITED=project-loses\n",
    );
    e.values.insert("INHERITED".into(), "inherited".into());
    let s = e.load();
    for (k, v) in [
        ("SHARED", "project"),
        ("USER", "user-only"),
        ("PROJECT", "project-only"),
        ("INHERITED", "inherited"),
    ] {
        assert_eq!(e.values[k], v);
        assert_eq!(s.get(k).unwrap().value, v);
    }
}
#[test]
fn refuses_to_launch_when_a_env_sets_bootstrap_names_before_applying_anything() {
    for name in [
        "DSH_PERMISSION_MODE",
        "PATH",
        "NODE_OPTIONS",
        "DSH_AGENTS_HOME",
        "HTTPS_PROXY",
        "https_proxy",
        "BROWSER",
        "DEEPSEEK_BASE_URL",
        "DEEPSEEK_SEARCH_BASE_URL",
        "deepseek_base_url",
    ] {
        let mut e = Envs::new();
        e.files(
            "USER=should-not-apply\n",
            &format!("PROJECT=applied-anyway\n{name}=evil\n"),
        );
        let before = e.values.clone();
        let error = load_layered_env(
            BIN,
            e.project.path(),
            e.home.path(),
            &mut e.values,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains(&format!(
            "sets \"{name}\", which only the launching environment may set"
        )));
        assert_eq!(e.values, before);
    }
}
#[test]
fn accepts_the_proxy_names_from_the_harness_home_env_below_an_exported_one() {
    let mut e = Envs::new();
    e.files("HTTP_PROXY=http://from-home:8080\nno_proxy=example.com\nHTTPS_PROXY=http://from-home:8443\n","");
    e.values
        .insert("HTTPS_PROXY".into(), "http://exported:8080".into());
    let s = e.load();
    assert_eq!(
        s.get("HTTP_PROXY").unwrap().source,
        LaunchEnvironmentSource::UserEnv
    );
    assert_eq!(s.get("no_proxy").unwrap().value, "example.com");
    assert_eq!(
        s.get("HTTPS_PROXY").unwrap().source,
        LaunchEnvironmentSource::Process
    );
    assert_eq!(e.values["HTTPS_PROXY"], "http://exported:8080");
}
#[test]
fn accepts_the_deepseek_endpoints_from_the_harness_home_env_below_an_exported_one() {
    let mut e = Envs::new();
    e.files(
        "DEEPSEEK_BASE_URL=https://home/anthropic\nDEEPSEEK_SEARCH_BASE_URL=https://home/v1\n",
        "",
    );
    e.values.insert(
        "DEEPSEEK_BASE_URL".into(),
        "https://exported/anthropic".into(),
    );
    let s = e.load();
    assert_eq!(
        s.get("DEEPSEEK_SEARCH_BASE_URL").unwrap().source,
        LaunchEnvironmentSource::UserEnv
    );
    assert_eq!(e.values["DEEPSEEK_BASE_URL"], "https://exported/anthropic");
}
#[test]
fn refuses_wrong_endpoint_casing_from_the_harness_home_env_and_names_the_exact_spelling() {
    for name in ["deepseek_base_url", "DeepSeek_Search_Base_URL"] {
        let mut e = Envs::new();
        e.files(&format!("{name}=evil\n"), "");
        let error = load_layered_env(
            BIN,
            e.project.path(),
            e.home.path(),
            &mut e.values,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains(&format!(
            "spell it {}, then export it or put it in {}",
            name.to_ascii_uppercase(),
            e.home.path().join(".env").display()
        )));
        assert!(!e.values.contains_key(name));
    }
}
#[test]
fn still_refuses_every_other_bootstrap_name_in_the_harness_home_env() {
    for name in ["SSL_CERT_FILE", "NODE_TLS_REJECT_UNAUTHORIZED", "DSH_HOME"] {
        let mut e = Envs::new();
        e.files(&format!("{name}=evil\n"), "");
        assert!(
            load_layered_env(
                BIN,
                e.project.path(),
                e.home.path(),
                &mut e.values,
                &mut |_| {}
            )
            .unwrap_err()
            .to_string()
            .contains("only the launching environment may set")
        );
        assert!(e.values.is_empty());
    }
}
#[test]
fn names_the_harness_home_file_as_the_way_out_when_a_project_env_sets_a_route() {
    for name in ["HTTP_PROXY", "DEEPSEEK_BASE_URL"] {
        let mut e = Envs::new();
        e.files("", &format!("{name}=evil\n"));
        let error = load_layered_env(
            BIN,
            e.project.path(),
            e.home.path(),
            &mut e.values,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().ends_with(&format!(
            "export {name}, or put it in {}, which does not travel with a repository",
            e.home.path().join(".env").display()
        )));
    }
}
#[test]
fn treats_the_invoking_directory_as_the_harness_home_when_they_are_the_same_directory() {
    let mut e = Envs::new();
    e.files("HTTP_PROXY=http://from-home:8080\n", "");
    let s = load_layered_env(
        BIN,
        e.home.path(),
        e.home.path(),
        &mut e.values,
        &mut |_| panic!("warning"),
    )
    .unwrap();
    assert_eq!(s.get("HTTP_PROXY").unwrap().value, "http://from-home:8080");
}
#[test]
fn reports_each_file_value_with_its_absolute_path() {
    let mut e = Envs::new();
    e.files("USER=u\n", "PROJECT=p\n");
    let s = e.load();
    assert_eq!(
        s.get("USER").unwrap().path.as_deref(),
        e.home.path().join(".env").to_str()
    );
    assert_eq!(
        s.get("PROJECT").unwrap().path.as_deref(),
        e.project.path().join(".env").to_str()
    );
    assert!(
        s.get_from(
            "PROJECT",
            &[
                LaunchEnvironmentSource::Process,
                LaunchEnvironmentSource::UserEnv
            ]
        )
        .is_none()
    );
}
#[test]
fn resolves_the_harness_home_from_the_inherited_environment_never_from_a_file() {
    let mut e = Envs::new();
    e.files("USER=real-home\n", "PROJECT=set-by-project\n");
    e.values.insert(
        "DSH_HOME".into(),
        e.home.path().to_string_lossy().into_owned(),
    );
    let s = e.load();
    assert_eq!(s.get("USER").unwrap().value, "real-home");
    assert_eq!(s.get("PROJECT").unwrap().value, "set-by-project");
}
#[test]
fn warns_and_continues_when_a_layer_exists_but_cannot_be_read() {
    let mut e = Envs::new();
    e.files("", "PROJECT=project-only\n");
    std::fs::remove_file(e.home.path().join(".env")).unwrap();
    std::fs::create_dir(e.home.path().join(".env")).unwrap();
    let mut warnings = vec![];
    let s = load_layered_env(
        BIN,
        e.project.path(),
        e.home.path(),
        &mut e.values,
        &mut |w| warnings.push(w),
    )
    .unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(s.get("PROJECT").unwrap().value, "project-only");
    assert!(s.get("USER").is_none());
}
#[test]
fn reports_to_stderr_when_the_caller_supplies_no_reporter() {
    // The launcher supplies its stderr adapter explicitly in Rust.
    let mut e = Envs::new();
    e.files("", "PROJECT=project-only\n");
    std::fs::remove_file(e.home.path().join(".env")).unwrap();
    std::fs::create_dir(e.home.path().join(".env")).unwrap();
    let mut stderr = String::new();
    let s = load_layered_env(
        BIN,
        e.project.path(),
        e.home.path(),
        &mut e.values,
        &mut |w| stderr.push_str(&w),
    )
    .unwrap();
    assert!(stderr.starts_with(&format!("{BIN}: failed to load .env")));
    assert_eq!(s.get("PROJECT").unwrap().value, "project-only");
}
#[test]
fn passes_over_an_absent_layer_without_reporting_it() {
    let mut e = Envs::new();
    std::fs::write(e.project.path().join(".env"), "PROJECT=project-only\n").unwrap();
    assert_eq!(e.load().get("PROJECT").unwrap().value, "project-only");
}
#[test]
fn carries_only_the_inherited_environment_when_neither_file_exists() {
    let mut e = Envs::new();
    e.values.insert("INHERITED".into(), "inherited".into());
    let s = e.load();
    assert_eq!(
        s.get("INHERITED").unwrap().source,
        LaunchEnvironmentSource::Process
    );
    assert!(s.get("USER").is_none());
}
#[test]
fn reads_a_harness_home_that_is_also_the_invocation_directory_exactly_once() {
    let mut e = Envs::new();
    e.files("PROJECT=one-file\n", "");
    let s = load_layered_env(
        BIN,
        e.home.path(),
        e.home.path(),
        &mut e.values,
        &mut |_| panic!("warning"),
    )
    .unwrap();
    assert_eq!(
        s.get("PROJECT").unwrap().source,
        LaunchEnvironmentSource::ProjectEnv
    );
    assert_eq!(s.get("PROJECT").unwrap().value, "one-file");
}

#[derive(Default)]
struct Process {
    handlers: Arc<Mutex<Vec<(FailLoudEvent, FailureListener)>>>,
    written: Mutex<Vec<String>>,
    exits: Mutex<Vec<i32>>,
}
impl FailLoudProcess for Process {
    fn on(&self, event: FailLoudEvent, listener: FailureListener) -> Subscription {
        lock(&self.handlers).push((event, listener.clone()));
        let handlers = self.handlers.clone();
        Subscription::new(move || {
            lock(&handlers).retain(|(e, l)| *e != event || !Arc::ptr_eq(l, &listener))
        })
    }
    fn write_stderr(&self, text: &str) {
        lock(&self.written).push(text.into());
    }
    fn exit(&self, code: i32) {
        lock(&self.exits).push(code);
    }
}
impl Process {
    fn emit(&self, event: FailLoudEvent, reason: FailLoudReason) {
        let handlers = lock(&self.handlers).clone();
        for (e, l) in handlers {
            if e == event {
                l(reason.clone());
            }
        }
    }
}
fn reason(message: &str) -> FailLoudReason {
    FailLoudReason::new(hoh_error::SharedError::new(hoh_error::BoxError::from(
        message.to_owned(),
    )))
}
async fn turns() {
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}
#[test]
fn writes_one_labelled_line_with_the_stack_and_exits_1_on_an_error_rejection() {
    let p = Arc::new(Process::default());
    let _g = install_fail_loud(BIN, p.clone(), None);
    let mut r = reason("boom");
    r.inspection = "Error: boom\n    at fixture".into();
    p.emit(FailLoudEvent::UnhandledRejection, r);
    assert_eq!(
        *lock(&p.written),
        [format!(
            "{BIN}: fatal load failure: Error: boom\n    at fixture\n"
        )]
    );
    assert_eq!(*lock(&p.exits), [1]);
}
#[test]
fn stringifies_a_non_error_rejection_and_an_error_without_a_stack_falls_back_to_its_message() {
    for text in ["plain failure", "no stack"] {
        let p = Arc::new(Process::default());
        let _g = install_fail_loud(BIN, p.clone(), None);
        p.emit(FailLoudEvent::UnhandledRejection, reason(text));
        assert!(lock(&p.written)[0].contains(text));
        assert_eq!(*lock(&p.exits), [1]);
    }
}
#[test]
fn returns_an_uninstaller_that_removes_the_handler_and_defaults_to_the_real_process() {
    let p = Arc::new(Process::default());
    let guard = install_fail_loud(BIN, p.clone(), None);
    assert_eq!(lock(&p.handlers).len(), 2);
    drop(guard);
    assert!(lock(&p.handlers).is_empty());
}
#[tokio::test(start_paused = true)]
async fn does_not_report_an_activation_rejection_shared_by_entries_in_the_boot_audit() {
    let p = Arc::new(Process::default());
    let guard = install_fail_loud(BIN, p.clone(), None);
    let r = reason("assembled activation failure");
    let a = guard.retain_assembled(r.error.clone());
    let b = guard.retain_assembled(r.error.clone());
    p.emit(FailLoudEvent::UnhandledRejection, r.clone());
    assert!(lock(&p.exits).is_empty());
    drop(a);
    p.emit(FailLoudEvent::UnhandledRejection, r.clone());
    assert!(lock(&p.exits).is_empty());
    drop(b);
    p.emit(FailLoudEvent::UnhandledRejection, r);
    assert_eq!(*lock(&p.exits), [1]);
}
#[tokio::test(start_paused = true)]
async fn awaits_the_release_hook_before_exiting_so_the_terminal_owner_can_restore_it() {
    let p = Arc::new(Process::default());
    let released = Arc::new(Mutex::new(false));
    let flag = released.clone();
    let _g = install_fail_loud(
        BIN,
        p.clone(),
        Some(Arc::new(move || {
            let flag = flag.clone();
            Box::pin(async move {
                tokio::task::yield_now().await;
                *lock(&flag) = true;
                Ok(())
            })
        })),
    );
    p.emit(
        FailLoudEvent::UnhandledRejection,
        reason("sibling entry rejected"),
    );
    assert!(lock(&p.exits).is_empty());
    turns().await;
    assert!(*lock(&released));
    assert_eq!(*lock(&p.exits), [1]);
}
#[tokio::test(start_paused = true)]
async fn still_exits_when_the_release_hook_rejects() {
    let p = Arc::new(Process::default());
    let _g = install_fail_loud(
        BIN,
        p.clone(),
        Some(Arc::new(|| {
            Box::pin(async { Err("terminal stop failed".into()) })
        })),
    );
    p.emit(FailLoudEvent::UnhandledRejection, reason("boom"));
    turns().await;
    assert_eq!(*lock(&p.exits), [1]);
}
#[tokio::test(start_paused = true)]
async fn exits_without_waiting_when_a_release_hook_never_settles() {
    let p = Arc::new(Process::default());
    let _g = install_fail_loud(
        BIN,
        p.clone(),
        Some(Arc::new(|| Box::pin(std::future::pending()))),
    );
    p.emit(FailLoudEvent::UnhandledRejection, reason("boom"));
    turns().await;
    assert!(lock(&p.exits).is_empty());
    tokio::time::advance(std::time::Duration::from_millis(
        FAIL_LOUD_RELEASE_TIMEOUT_MS,
    ))
    .await;
    turns().await;
    assert_eq!(*lock(&p.exits), [1]);
}
#[tokio::test(start_paused = true)]
async fn reports_only_the_first_rejection_and_keeps_handling_later_ones_during_the_release() {
    let p = Arc::new(Process::default());
    let _g = install_fail_loud(
        BIN,
        p.clone(),
        Some(Arc::new(|| {
            Box::pin(async {
                tokio::task::yield_now().await;
                Ok(())
            })
        })),
    );
    for (event, text) in [
        (FailLoudEvent::UnhandledRejection, "first rejection"),
        (FailLoudEvent::UnhandledRejection, "second rejection"),
        (
            FailLoudEvent::UncaughtException,
            "exception during teardown",
        ),
    ] {
        p.emit(event, reason(text));
    }
    assert_eq!(lock(&p.handlers).len(), 2);
    assert_eq!(lock(&p.written).len(), 1);
    assert!(lock(&p.written)[0].contains("first rejection"));
    turns().await;
    assert_eq!(*lock(&p.exits), [1]);
}
#[test]
fn turns_an_uncaught_exception_into_a_labelled_diagnostic_with_its_enumerable_properties_and_exits_1()
 {
    let p = Arc::new(Process::default());
    let _g = install_fail_loud(BIN, p.clone(), None);
    let mut r = reason("ENOENT");
    r.inspection="Error: ENOENT\n    at fixture { syscall: 'open', path: '/tmp/private/out.log', code: 'ENOENT' }".into();
    p.emit(FailLoudEvent::UncaughtException, r.clone());
    assert_eq!(
        *lock(&p.written),
        [format!(
            "{BIN}: fatal uncaught exception: {}\n",
            r.inspection
        )]
    );
    assert_eq!(*lock(&p.exits), [1]);
}
#[test]
fn includes_the_cause_chain_in_the_diagnostic() {
    #[derive(Debug, thiserror::Error)]
    #[error("wrapper")]
    struct Wrapper(#[source] hoh_error::BoxError);
    let p = Arc::new(Process::default());
    let _g = install_fail_loud(BIN, p.clone(), None);
    p.emit(
        FailLoudEvent::UncaughtException,
        FailLoudReason::new(hoh_error::SharedError::new(Wrapper("root cause".into()))),
    );
    let text = lock(&p.written).join("");
    assert!(text.contains("wrapper"));
    assert!(text.contains("root cause"));
    assert_eq!(*lock(&p.exits), [1]);
}
#[tokio::test(start_paused = true)]
async fn awaits_the_release_hook_for_an_uncaught_exception_as_it_does_for_a_rejection() {
    let p = Arc::new(Process::default());
    let released = Arc::new(Mutex::new(false));
    let flag = released.clone();
    let _g = install_fail_loud(
        BIN,
        p.clone(),
        Some(Arc::new(move || {
            let flag = flag.clone();
            Box::pin(async move {
                tokio::task::yield_now().await;
                *lock(&flag) = true;
                Ok(())
            })
        })),
    );
    p.emit(FailLoudEvent::UncaughtException, reason("listener threw"));
    assert!(lock(&p.exits).is_empty());
    turns().await;
    assert!(*lock(&released));
    assert_eq!(*lock(&p.exits), [1]);
}
