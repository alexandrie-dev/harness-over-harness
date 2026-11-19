use futures_util::FutureExt;
use hoh_error::SharedError;
use hoh_subprocess::SubprocessOutcome;
use hoh_subprocess_local::{linux_scope::*, managed_owner::BoundProcessOwner, runner_protocol::*};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
fn result(state: &str) -> SystemctlResult {
    SystemctlResult {
        status: Some(0),
        stdout: format!("LoadState=loaded\nActiveState={state}\n"),
        stderr: String::new(),
        error: None,
    }
}
fn missing() -> SystemctlResult {
    SystemctlResult {
        status: Some(1),
        stdout: String::new(),
        stderr: "Unit dsh.scope could not be found.".into(),
        error: None,
    }
}
fn fake_owner(
    unit: String,
    startup: LinuxScopeStartup,
    direct: DirectRange,
    mut internals: LinuxScopeInternals,
) -> SystemdScopeOwner {
    let run = internals.run_sync.clone();
    internals.systemctl_query = Arc::new(move |file, args| {
        let run = run.clone();
        async move { run(&file, &args, &manager_environment()) }.boxed()
    });
    SystemdScopeOwner::new(unit, startup, direct, internals)
}
fn startup() -> LinuxScopeStartup {
    LinuxScopeStartup::new(
        create_linux_launch_files(&LinuxLaunchRequest {
            cwd: "/target".into(),
            env: Default::default(),
            pipe_fds: None,
            control: None,
            control_path: None,
        })
        .unwrap(),
        LinuxScopeKind::Subprocess,
    )
}
fn owner(startup: LinuxScopeStartup, run: RunSync, running: Arc<AtomicBool>) -> SystemdScopeOwner {
    fake_owner(
        "dsh.scope".into(),
        startup,
        DirectRange {
            running,
            signal: Arc::new(|_| true),
            settled: async { Ok(()) }.boxed().shared(),
        },
        LinuxScopeInternals {
            run_sync: run,
            ..Default::default()
        },
    )
}
#[test]
fn drops_the_expansion_opt_out_only_for_a_systemd_run_that_rejects_it() {
    let calls = Arc::new(Mutex::new(vec![]));
    let recorded = calls.clone();
    let internals = LinuxScopeInternals {
        run_sync: Arc::new(move |_, args, _| {
            let mut calls = recorded.lock().unwrap();
            calls.push(args.to_vec());
            if calls.len() == 1 {
                SystemctlResult {
                    status: Some(1),
                    stdout: String::new(),
                    stderr: "systemd-run: unrecognized option '--expand-environment=no'".into(),
                    error: None,
                }
            } else {
                result("active")
            }
        }),
        ..Default::default()
    };
    assert_eq!(probe_linux_scope(&internals).unwrap(), Some(vec![]));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].contains(&"--expand-environment=no".into()));
    assert!(!calls[1].contains(&"--expand-environment=no".into()));
    let internals = LinuxScopeInternals {
        run_sync: Arc::new(|_, _, _| missing()),
        ..Default::default()
    };
    assert!(probe_linux_scope(&internals).unwrap().is_none());
}
#[test]
fn keeps_quieting_on_the_transient_scope_probe_but_preserves_manager_diagnostics() {
    let calls = Arc::new(Mutex::new(vec![]));
    let recorded = calls.clone();
    let internals = LinuxScopeInternals {
        run_sync: Arc::new(move |file, args, env| {
            recorded
                .lock()
                .unwrap()
                .push((file.to_owned(), args.to_vec(), env.clone()));
            result("active")
        }),
        ..Default::default()
    };
    assert!(probe_linux_scope(&internals).unwrap().is_some());
    assert!(probe_linux_manager(&internals));
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls[0].2.get("SYSTEMD_LOG_TARGET").map(String::as_str),
        Some("null")
    );
    assert!(!calls[1].2.contains_key("SYSTEMD_LOG_TARGET"));
    assert_eq!(calls[1].2.get("LC_ALL").map(String::as_str), Some("C"));
}
#[test]
fn matches_a_socket_path_outside_ascii_in_the_kernel_socket_table() {
    let dir = tempfile::tempdir().unwrap();
    let table = dir.path().join("unix");
    let path = "/run/é/systemd/private";
    std::fs::write(&table,format!("Num RefCount Protocol Flags Type St Inode Path\n000: 2 0 00010000 0001 01 1234 {path}\n")).unwrap();
    assert_eq!(unix_socket_listener(path, &table).as_deref(), Some("1234"));
    assert!(unix_socket_listener("/run/other/systemd/private", &table).is_none());
    std::fs::write(&table, format!("000: 2 0 00000000 0001 01 1234 {path}\n")).unwrap();
    assert!(unix_socket_listener(path, &table).is_none());
}
#[test]
fn rejects_malformed_duplicate_incomplete_and_extra_manager_state_fields() {
    for (text, message) in [
        ("oops", "malformed state"),
        (
            "LoadState=loaded\nLoadState=loaded\nActiveState=active",
            "duplicate LoadState",
        ),
        ("LoadState=loaded", "incomplete state"),
        (
            "LoadState=loaded\nActiveState=active\nExtra=x",
            "incomplete state",
        ),
    ] {
        assert!(
            parse_unit_state("dsh.scope", text)
                .unwrap_err()
                .to_string()
                .contains(message)
        );
    }
}
#[test]
fn rejects_a_manager_process_count_that_is_neither_numeric_nor_the_unset_sentinel() {
    assert!(
        parse_unit_state(
            "dsh.scope",
            "LoadState=loaded\nActiveState=active\nTasksCurrent=-1"
        )
        .unwrap_err()
        .to_string()
        .contains("non-numeric TasksCurrent")
    );
    assert_eq!(
        parse_unit_state(
            "dsh.scope",
            "LoadState=loaded\nActiveState=active\nTasksCurrent=[not set]"
        )
        .unwrap()
        .tasks_current,
        None
    );
    assert_eq!(
        parse_unit_state(
            "dsh.scope",
            "LoadState=loaded\nActiveState=active\nTasksCurrent=0"
        )
        .unwrap()
        .tasks_current,
        Some(0)
    );
}
#[test]
fn preserves_a_recorded_pre_exec_failure_even_when_cancellation_also_terminates_the_bootstrap() {
    let startup = startup();
    startup
        .termination_signals
        .lock()
        .unwrap()
        .insert("SIGTERM".into());
    write_linux_startup_error(
        &startup.files,
        &LinuxStartupError::Error {
            error: SerializedRunnerError {
                name: "Error".into(),
                message: "spawn tool ENOENT".into(),
                code: Some("ENOENT".into()),
                syscall: Some("spawn tool".into()),
                path: Some("tool".into()),
            },
        },
    )
    .unwrap();
    let failure = startup
        .resolve_outcome(SubprocessOutcome {
            exit_code: None,
            signal: Some("SIGTERM".into()),
        })
        .unwrap_err();
    assert_eq!(failure.to_string(), "spawn tool ENOENT");
    cleanup_linux_launch_files(&startup.files);
}
#[test]
fn uses_default_owner_dependencies_and_rejects_an_unconsumed_request() {
    let startup = startup();
    let failure = startup
        .resolve_outcome(SubprocessOutcome {
            exit_code: Some(0),
            signal: None,
        })
        .unwrap_err();
    assert!(failure.is::<LinuxScopeNotStartedError>());
    assert_eq!(
        failure.to_string(),
        "subprocess scope exited before its bootstrap consumed the launch request; the provider probes native containment again before its next command"
    );
    cleanup_linux_launch_files(&startup.files);
}
#[test]
fn retains_unrelated_bootstrap_failure_after_a_termination_request() {
    for outcome in [
        SubprocessOutcome {
            exit_code: Some(1),
            signal: None,
        },
        SubprocessOutcome {
            exit_code: None,
            signal: Some("SIGSEGV".into()),
        },
    ] {
        let startup = startup();
        startup
            .termination_signals
            .lock()
            .unwrap()
            .insert("SIGTERM".into());
        assert!(
            startup
                .resolve_outcome(outcome)
                .unwrap_err()
                .is::<LinuxScopeNotStartedError>()
        );
        cleanup_linux_launch_files(&startup.files);
    }
}
#[tokio::test]
async fn uses_manager_observed_unit_existence_as_establishment_proof() {
    let startup = startup();
    let files = startup.files.clone();
    let owner = owner(
        startup,
        Arc::new(|_, _, _| result("inactive")),
        Arc::new(AtomicBool::new(true)),
    );
    owner.wait_for_exit().await.unwrap();
    owner.cleanup().unwrap();
    assert!(!files.directory.exists());
}
#[tokio::test]
async fn does_not_mistake_pre_establishment_unit_absence_for_quiescence_and_settles_an_empty_range_after_cancellation()
 {
    let startup = startup();
    let running = Arc::new(AtomicBool::new(true));
    let owner = owner(startup, Arc::new(|_, _, _| missing()), running.clone());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), owner.wait_for_exit())
            .await
            .is_err()
    );
    owner.signal(rustix::process::Signal::TERM);
    running.store(false, Ordering::Release);
    owner.wait_for_exit().await.unwrap();
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn releases_an_active_scope_left_with_no_processes_once_its_client_has_gone() {
    let startup = startup();
    let stops = Arc::new(Mutex::new(0));
    let recorded = stops.clone();
    let owner = owner(
        startup,
        Arc::new(move |_, args, _| {
            if args.contains(&"stop".into()) {
                *recorded.lock().unwrap() += 1;
            }
            SystemctlResult {
                stdout: "LoadState=loaded\nActiveState=active\nTasksCurrent=0\n".into(),
                ..result("active")
            }
        }),
        Arc::new(AtomicBool::new(false)),
    );
    owner.signal(rustix::process::Signal::TERM);
    owner.wait_for_exit().await.unwrap();
    assert_eq!(*stops.lock().unwrap(), 1);
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn retries_a_failed_state_query_and_rejects_unknown_states_or_failed_final_kills() {
    let startup = startup();
    let sequence = Arc::new(Mutex::new(vec![
        SystemctlResult {
            stderr: String::new(),
            error: Some(hoh_error::SharedError::new(
                hoh_subprocess_local::LocalError::Error("query failed".into()),
            )),
            ..missing()
        },
        result("inactive"),
    ]));
    let recorded = sequence.clone();
    let owner = owner(
        startup,
        Arc::new(move |_, _, _| recorded.lock().unwrap().remove(0)),
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(
        owner.wait_for_exit().await.unwrap_err().to_string(),
        "query failed"
    );
    owner.wait_for_exit().await.unwrap();
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn reports_unknown_active_state() {
    let startup = startup();
    let owner = owner(
        startup,
        Arc::new(|_, _, _| result("mystery")),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(
        owner
            .wait_for_exit()
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown ActiveState")
    );
    owner.cleanup().unwrap();
}

#[tokio::test]
async fn polls_promptly_before_establishment_and_backs_off_established_active_scopes() {
    let startup = startup();
    let states = Arc::new(Mutex::new(std::collections::VecDeque::from([
        missing(),
        result("active"),
        result("active"),
        result("active"),
        result("active"),
        result("active"),
        result("active"),
        result("active"),
        result("active"),
        result("inactive"),
    ])));
    let delays = Arc::new(Mutex::new(vec![]));
    let recorded = delays.clone();
    let sequence = states.clone();
    let owner = fake_owner(
        "dsh.scope".into(),
        startup,
        DirectRange {
            running: Arc::new(AtomicBool::new(true)),
            signal: Arc::new(|_| true),
            settled: async { Ok(()) }.boxed().shared(),
        },
        LinuxScopeInternals {
            run_sync: Arc::new(move |_, _, _| sequence.lock().unwrap().pop_front().unwrap()),
            sleep: Arc::new(move |duration| {
                recorded.lock().unwrap().push(duration.as_millis());
                async {}.boxed()
            }),
            ..Default::default()
        },
    );
    owner.wait_for_exit().await.unwrap();
    assert_eq!(
        *delays.lock().unwrap(),
        [50, 50, 100, 200, 400, 800, 1600, 3200, 5000]
    );
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn keeps_waiting_while_the_unit_is_absent_and_the_direct_launcher_is_still_running() {
    let startup = startup();
    let calls = Arc::new(Mutex::new(0));
    let recorded = calls.clone();
    let owner = owner(
        startup,
        Arc::new(move |_, _, _| {
            let mut calls = recorded.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                missing()
            } else {
                result("inactive")
            }
        }),
        Arc::new(AtomicBool::new(true)),
    );
    owner.wait_for_exit().await.unwrap();
    assert_eq!(*calls.lock().unwrap(), 2);
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn uses_the_scope_alone_after_establishment_and_the_direct_range_only_when_scope_kill_fails()
{
    for signal in [rustix::process::Signal::TERM, rustix::process::Signal::KILL] {
        let startup = startup();
        consume_linux_launch_request(&startup.files.request_path).unwrap();
        let delivered = Arc::new(Mutex::new(vec![]));
        let captured = delivered.clone();
        let calls = Arc::new(Mutex::new(0));
        let observed = calls.clone();
        let owner = fake_owner(
            "dsh.scope".into(),
            startup,
            DirectRange {
                running: Arc::new(AtomicBool::new(true)),
                signal: Arc::new(move |signal| {
                    captured.lock().unwrap().push(signal);
                    true
                }),
                settled: async { Ok(()) }.boxed().shared(),
            },
            LinuxScopeInternals {
                run_sync: Arc::new(move |_, _, _| {
                    let mut count = observed.lock().unwrap();
                    *count += 1;
                    if *count == 1 {
                        result("active")
                    } else {
                        SystemctlResult {
                            stderr: "scope signal failed".into(),
                            ..missing()
                        }
                    }
                }),
                ..Default::default()
            },
        );
        owner.signal(rustix::process::Signal::TERM);
        assert!(delivered.lock().unwrap().is_empty());
        owner.signal(signal);
        assert_eq!(*delivered.lock().unwrap(), [signal]);
        assert_eq!(*calls.lock().unwrap(), 2);
        owner.cleanup().unwrap();
    }
}
#[tokio::test]
async fn accepts_a_confirmed_empty_range_after_a_failed_final_kill() {
    let startup = startup();
    let owner = owner(
        startup,
        Arc::new(|_, args, _| {
            if args.contains(&"kill".into()) {
                SystemctlResult {
                    stderr: "Invalid argument".into(),
                    ..missing()
                }
            } else {
                result("inactive")
            }
        }),
        Arc::new(AtomicBool::new(false)),
    );
    owner.signal(rustix::process::Signal::KILL);
    owner.wait_for_exit().await.unwrap();
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn reports_failed_scope_and_direct_kill_submission_without_awaiting_direct_exit() {
    let startup = startup();
    let owner = fake_owner(
        "dsh.scope".into(),
        startup,
        DirectRange {
            running: Arc::new(AtomicBool::new(true)),
            signal: Arc::new(|_| false),
            settled: futures_util::future::pending().boxed().shared(),
        },
        LinuxScopeInternals {
            run_sync: Arc::new(|_, args, _| {
                if args.contains(&"kill".into()) {
                    SystemctlResult {
                        stderr: "permission denied".into(),
                        ..missing()
                    }
                } else {
                    result("active")
                }
            }),
            ..Default::default()
        },
    );
    owner.signal(rustix::process::Signal::KILL);
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), owner.wait_for_exit())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("permission denied"));
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn observes_native_task_counts_independently_from_process_tree_membership() {
    let startup = startup();
    let response = Arc::new(Mutex::new(result("active")));
    let seen = response.clone();
    let owner = owner(
        startup,
        Arc::new(move |_, _, _| seen.lock().unwrap().clone()),
        Arc::new(AtomicBool::new(true)),
    );
    for tasks in [1, 2, 0] {
        *response.lock().unwrap() = SystemctlResult {
            stdout: format!("LoadState=loaded\nActiveState=active\nTasksCurrent={tasks}\n"),
            ..result("active")
        };
        assert_eq!(owner.inspect_task_count().unwrap(), Some(tasks));
    }
    for value in [
        SystemctlResult {
            stdout: "LoadState=loaded\nActiveState=active\nTasksCurrent=[not set]\n".into(),
            ..result("active")
        },
        SystemctlResult {
            stdout: "LoadState=not-found\nActiveState=inactive\n".into(),
            ..result("inactive")
        },
        result("inactive"),
        missing(),
        SystemctlResult {
            error: Some(SharedError::new(hoh_subprocess_local::LocalError::Error(
                "manager unavailable".into(),
            ))),
            ..missing()
        },
    ] {
        *response.lock().unwrap() = value;
        assert_eq!(owner.inspect_task_count().unwrap(), None);
    }
    *response.lock().unwrap() = SystemctlResult {
        stdout: "LoadState=loaded\nActiveState=active\nTasksCurrent=invalid\n".into(),
        ..result("active")
    };
    assert!(
        owner
            .inspect_task_count()
            .unwrap_err()
            .to_string()
            .contains("non-numeric")
    );
    owner.cleanup().unwrap();
}

fn failed_kill() -> SystemctlResult {
    SystemctlResult {
        stderr: "Invalid argument".into(),
        ..missing()
    }
}
fn tasks(value: &str) -> SystemctlResult {
    SystemctlResult {
        stdout: format!("LoadState=loaded\nActiveState=active\nTasksCurrent={value}\n"),
        ..result("active")
    }
}
#[tokio::test]
async fn keeps_reloading_scopes_active_and_lets_terminate_wake_a_backed_off_observation() {
    let startup = startup();
    consume_linux_launch_request(&startup.files.request_path).unwrap();
    let queries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = queries.clone();
    let sleeps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sleeping = sleeps.clone();
    let owner = SystemdScopeOwner::new(
        "dsh.scope".into(),
        startup,
        DirectRange {
            running: Arc::new(AtomicBool::new(true)),
            signal: Arc::new(|_| true),
            settled: futures_util::future::pending().boxed().shared(),
        },
        LinuxScopeInternals {
            run_sync: Arc::new(|_, _, _| result("active")),
            systemctl_query: Arc::new(move |_, _| {
                let state = if count.fetch_add(1, Ordering::AcqRel) == 0 {
                    "reloading"
                } else {
                    "inactive"
                };
                async move { result(state) }.boxed()
            }),
            sleep: Arc::new(move |duration| {
                assert_eq!(duration.as_millis(), 50);
                sleeping.fetch_add(1, Ordering::AcqRel);
                futures_util::future::pending().boxed()
            }),
            ..Default::default()
        },
    );
    let pending = owner.wait_for_exit();
    assert!(pending.clone().now_or_never().is_none());
    assert_eq!(sleeps.load(Ordering::Acquire), 1);
    owner.signal(rustix::process::Signal::TERM);
    pending.await.unwrap();
    assert_eq!(queries.load(Ordering::Acquire), 2);
    assert_eq!(sleeps.load(Ordering::Acquire), 1);
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn skips_the_next_poll_delay_when_terminate_arrives_during_a_manager_query() {
    let startup = startup();
    consume_linux_launch_request(&startup.files.request_path).unwrap();
    let (send, receive) = tokio::sync::oneshot::channel::<SystemctlResult>();
    let first = Arc::new(Mutex::new(Some(receive)));
    let owner = SystemdScopeOwner::new(
        "dsh.scope".into(),
        startup,
        DirectRange {
            running: Arc::new(AtomicBool::new(true)),
            signal: Arc::new(|_| true),
            settled: futures_util::future::pending().boxed().shared(),
        },
        LinuxScopeInternals {
            run_sync: Arc::new(|_, _, _| result("active")),
            systemctl_query: Arc::new(move |_, _| {
                let first = first.lock().unwrap().take();
                async move {
                    match first {
                        Some(first) => first.await.unwrap(),
                        None => result("inactive"),
                    }
                }
                .boxed()
            }),
            sleep: Arc::new(|_| panic!("stale query must not schedule a poll delay")),
            ..Default::default()
        },
    );
    let pending = owner.wait_for_exit();
    assert!(pending.clone().now_or_never().is_none());
    owner.signal(rustix::process::Signal::TERM);
    send.send(result("active"))
        .unwrap_or_else(|_| panic!("query abandoned"));
    pending.await.unwrap();
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn joins_a_delivered_or_already_absent_direct_kill_before_deciding_the_fresh_scope() {
    // A delivered signal and ESRCH both accept direct submission. Group delivery does not
    // change that boundary: the final KILL must join the direct process in all four cases.
    for delivery in ["delivered", "already absent"] {
        for group_accepted in [false, true] {
            for (fresh, settles) in [
                (tasks("0"), true),
                (result("inactive"), true),
                (tasks("1"), false),
                (tasks("[not set]"), false),
            ] {
                let startup = startup();
                consume_linux_launch_request(&startup.files.request_path).unwrap();
                let running = Arc::new(AtomicBool::new(true));
                let (send, receive) = tokio::sync::oneshot::channel();
                let queries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let count = queries.clone();
                let signals = Arc::new(Mutex::new(vec![]));
                let delivered = signals.clone();
                let owner = SystemdScopeOwner::new(
                    "dsh.scope".into(),
                    startup,
                    DirectRange {
                        running: running.clone(),
                        signal: Arc::new(move |signal| {
                            delivered.lock().unwrap().push(signal);
                            true
                        }),
                        settled: async move {
                            receive.await.unwrap();
                            Ok(())
                        }
                        .boxed()
                        .shared(),
                    },
                    LinuxScopeInternals {
                        run_sync: Arc::new(|_, args, _| {
                            if args.contains(&"kill".into()) {
                                failed_kill()
                            } else {
                                result("active")
                            }
                        }),
                        systemctl_query: Arc::new(move |_, _| {
                            let answer = if count.fetch_add(1, Ordering::AcqRel) == 0 {
                                tasks("1")
                            } else {
                                fresh.clone()
                            };
                            async move { answer }.boxed()
                        }),
                        sleep: Arc::new(|_| {
                            panic!("must join direct settlement before a fresh query")
                        }),
                        ..Default::default()
                    },
                );
                owner.signal(rustix::process::Signal::KILL);
                let pending = owner.wait_for_exit();
                assert!(
                    pending.clone().now_or_never().is_none(),
                    "{delivery}, group={group_accepted}"
                );
                assert_eq!(*signals.lock().unwrap(), [rustix::process::Signal::KILL]);
                assert_eq!(queries.load(Ordering::Acquire), 1);
                running.store(false, Ordering::Release);
                send.send(()).unwrap();
                let result = pending.await;
                assert_eq!(result.is_ok(), settles);
                if let Err(error) = result {
                    assert!(error.to_string().contains("Invalid argument"));
                }
                assert_eq!(queries.load(Ordering::Acquire), 2);
                owner.cleanup().unwrap();
            }
        }
    }
}
#[tokio::test]
async fn does_not_accept_a_pre_signal_empty_observation_when_the_fresh_range_remains_populated() {
    for (before, after, success) in [
        (result("inactive"), tasks("1"), false),
        (tasks("1"), result("inactive"), true),
    ] {
        let startup = startup();
        consume_linux_launch_request(&startup.files.request_path).unwrap();
        let (send, receive) = tokio::sync::oneshot::channel::<SystemctlResult>();
        let first = Arc::new(Mutex::new(Some(receive)));
        let queries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = queries.clone();
        let running = Arc::new(AtomicBool::new(true));
        let owner = SystemdScopeOwner::new(
            "dsh.scope".into(),
            startup,
            DirectRange {
                running: running.clone(),
                signal: Arc::new(|_| true),
                settled: async { Ok(()) }.boxed().shared(),
            },
            LinuxScopeInternals {
                run_sync: Arc::new(|_, _, _| failed_kill()),
                systemctl_query: Arc::new(move |_, _| {
                    count.fetch_add(1, Ordering::AcqRel);
                    let first = first.lock().unwrap().take();
                    let after = after.clone();
                    async move {
                        match first {
                            Some(first) => first.await.unwrap(),
                            None => after,
                        }
                    }
                    .boxed()
                }),
                sleep: Arc::new(|_| panic!("pre-signal state must be discarded")),
                ..Default::default()
            },
        );
        let pending = owner.wait_for_exit();
        assert!(pending.clone().now_or_never().is_none());
        owner.signal(rustix::process::Signal::KILL);
        running.store(false, Ordering::Release);
        send.send(before)
            .unwrap_or_else(|_| panic!("query abandoned"));
        assert_eq!(pending.await.is_ok(), success);
        assert_eq!(queries.load(Ordering::Acquire), 2);
        owner.cleanup().unwrap();
    }
}
#[tokio::test]
async fn retains_state_query_errors_without_awaiting_direct_settlement() {
    let startup = startup();
    consume_linux_launch_request(&startup.files.request_path).unwrap();
    let failure = SharedError::new(hoh_subprocess_local::LocalError::Error(
        "manager unreachable".into(),
    ));
    let original = failure.clone();
    let owner = SystemdScopeOwner::new(
        "dsh.scope".into(),
        startup,
        DirectRange {
            running: Arc::new(AtomicBool::new(true)),
            signal: Arc::new(|_| true),
            settled: futures_util::future::pending().boxed().shared(),
        },
        LinuxScopeInternals {
            run_sync: Arc::new(|_, _, _| failed_kill()),
            systemctl_query: Arc::new(move |_, _| {
                let failure = failure.clone();
                async move {
                    SystemctlResult {
                        status: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        error: Some(failure),
                    }
                }
                .boxed()
            }),
            ..Default::default()
        },
    );
    owner.signal(rustix::process::Signal::KILL);
    assert!(owner.wait_for_exit().await.unwrap_err().ptr_eq(&original));
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn keeps_waiting_for_empty_or_unset_counts_without_sufficient_termination_proof() {
    for (count, terminate) in [("0", false), ("0", true), ("[not set]", false)] {
        let startup = startup();
        let states = Arc::new(Mutex::new(std::collections::VecDeque::from([
            tasks(count),
            result("inactive"),
        ])));
        let queries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = queries.clone();
        let commands = Arc::new(Mutex::new(vec![]));
        let recorded = commands.clone();
        let owner = SystemdScopeOwner::new(
            "dsh.scope".into(),
            startup,
            DirectRange {
                running: Arc::new(AtomicBool::new(true)),
                signal: Arc::new(|_| true),
                settled: futures_util::future::pending().boxed().shared(),
            },
            LinuxScopeInternals {
                run_sync: Arc::new(move |_, args, _| {
                    recorded.lock().unwrap().push(args[1].clone());
                    result("active")
                }),
                systemctl_query: Arc::new(move |_, _| {
                    observed.fetch_add(1, Ordering::AcqRel);
                    let next = states.lock().unwrap().pop_front().unwrap();
                    async move { next }.boxed()
                }),
                sleep: Arc::new(|_| async {}.boxed()),
                ..Default::default()
            },
        );
        if terminate {
            owner.signal(rustix::process::Signal::TERM);
        }
        owner.wait_for_exit().await.unwrap();
        assert_eq!(queries.load(Ordering::Acquire), 2);
        assert_eq!(
            *commands.lock().unwrap(),
            if terminate { vec!["kill"] } else { vec![] }
        );
        owner.cleanup().unwrap();
    }
}
#[tokio::test]
async fn reports_command_query_failures_from_the_default_systemctl_adapter() {
    let success = query_systemctl(
        "/bin/sh".into(),
        vec![
            "-c".into(),
            "printf 'LoadState=loaded\\nActiveState=inactive\\n'; printf diagnostic >&2".into(),
        ],
    )
    .await;
    assert_eq!(success.status, Some(0));
    assert!(success.error.is_none());
    assert_eq!(success.stderr, "diagnostic");
    let failed = query_systemctl("/missing-systemctl-hoh".into(), vec![]).await;
    assert_eq!(failed.status, None);
    assert_eq!(
        failed.error.unwrap().to_string(),
        "spawn /missing-systemctl-hoh ENOENT"
    );
    let failed = query_systemctl(
        "/bin/sh".into(),
        vec!["-c".into(), "printf denied >&2; exit 5".into()],
    )
    .await;
    assert_eq!(failed.status, Some(5));
    assert_eq!(failed.stderr, "denied");
    assert!(failed.error.unwrap().to_string().ends_with("\ndenied"));
}

fn terminal_spec() -> hoh_subprocess::SubprocessTerminalSpawnSpec {
    hoh_subprocess::SubprocessTerminalSpawnSpec {
        argv: vec!["bash".into(), "--noprofile".into()],
        cwd: "/target".into(),
        env: None,
        rows: 24,
        cols: 80,
        terminal_type: "dumb".into(),
        grace_ms: 100.0,
        signal: None,
        shell_activity: None,
    }
}
fn terminal_internals() -> LinuxScopeInternals {
    LinuxScopeInternals {
        systemd_run: "/bin/systemd-run".into(),
        systemctl: "/bin/systemctl".into(),
        runner_invocation: vec!["/private/runner".into()],
        run_sync: Arc::new(|_, _, _| missing()),
        systemctl_query: Arc::new(|_, _| async { missing() }.boxed()),
        ..Default::default()
    }
}
#[tokio::test]
async fn preserves_term_and_kill_before_pty_bootstrap_consumption_and_joins_the_empty_scope() {
    for signal in [rustix::process::Signal::TERM, rustix::process::Signal::KILL] {
        let launch = prepare_linux_terminal_scope(
            &terminal_spec(),
            [("TARGET".into(), "yes".into())].into(),
            &["--expand-environment=no".into()],
            terminal_internals(),
        )
        .unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let signals = Arc::new(Mutex::new(vec![]));
        let record = signals.clone();
        let owner = launch.bind_owner(DirectRange {
            running: running.clone(),
            signal: Arc::new(move |signal| {
                record.lock().unwrap().push(signal);
                true
            }),
            settled: async { Ok(()) }.boxed().shared(),
        });
        owner.signal(signal);
        assert_eq!(*signals.lock().unwrap(), [signal]);
        running.store(false, Ordering::Release);
        assert!(launch.startup.files.request_path.exists());
        let outcome = SubprocessOutcome {
            exit_code: Some(0),
            signal: hoh_subprocess_local::spawn::signal_name(signal.as_raw()).map(Into::into),
        };
        assert_eq!(launch.resolve_outcome(outcome.clone()).unwrap(), outcome);
        owner.wait_for_exit().await.unwrap();
        launch.cleanup();
        assert!(!launch.startup.files.directory.exists());
    }
}
#[tokio::test]
async fn uses_the_same_pty_request_bootstrap_preserves_argv_and_cleans_after_owner_settlement() {
    let launch = prepare_linux_terminal_scope(
        &terminal_spec(),
        [("TARGET".into(), "yes".into())].into(),
        &[],
        terminal_internals(),
    )
    .unwrap();
    assert_eq!(launch.command, "/bin/systemd-run");
    assert_eq!(
        &launch.args[..4],
        ["--user", "--scope", "--quiet", "--collect"]
    );
    assert!(launch.args[4].starts_with("--unit=dsh-terminal-"));
    assert_eq!(
        &launch.args[5..],
        ["--", "/private/runner", "--", "bash", "--noprofile"]
    );
    let request = consume_linux_launch_request(&launch.startup.files.request_path).unwrap();
    assert_eq!(request.cwd, "/target");
    assert_eq!(request.env["TARGET"], "yes");
    assert_eq!(request.pipe_fds, None);
    let owner = launch.bind_owner(DirectRange {
        running: Arc::new(AtomicBool::new(false)),
        signal: Arc::new(|_| true),
        settled: async { Ok(()) }.boxed().shared(),
    });
    owner.wait_for_exit().await.unwrap();
    let outcome = SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    };
    assert_eq!(launch.resolve_outcome(outcome.clone()).unwrap(), outcome);
    launch.cleanup();
    assert!(!launch.startup.files.directory.exists());
}
#[test]
fn surfaces_pty_pre_exec_errors_instead_of_launcher_outcomes() {
    let launch = prepare_linux_terminal_scope(
        &terminal_spec(),
        Default::default(),
        &[],
        terminal_internals(),
    )
    .unwrap();
    consume_linux_launch_request(&launch.startup.files.request_path).unwrap();
    write_linux_startup_error(
        &launch.startup.files,
        &LinuxStartupError::Error {
            error: SerializedRunnerError {
                name: "Error".into(),
                message: "bad cwd".into(),
                code: Some("ENOENT".into()),
                syscall: None,
                path: None,
            },
        },
    )
    .unwrap();
    assert_eq!(
        launch
            .resolve_outcome(SubprocessOutcome {
                exit_code: Some(127),
                signal: None
            })
            .unwrap_err()
            .to_string(),
        "bad cwd"
    );
    launch.cleanup();
}
#[tokio::test]
async fn reads_a_real_listening_socket_from_the_kernel_socket_table() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    assert!(
        unix_socket_listener(
            path.to_str().unwrap(),
            std::path::Path::new("/proc/net/unix")
        )
        .is_some()
    );
    drop(listener);
    assert!(
        unix_socket_listener(
            path.to_str().unwrap(),
            std::path::Path::new("/proc/net/unix")
        )
        .is_none()
    );
}
