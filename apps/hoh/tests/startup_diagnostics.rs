use hoh::startup_diagnostics::*;
use hoh_error::BoxError;
use hoh_loader::startup_diagnostics::{ActivationErrors, StartupEntryDiagnostic, StartupOutcome};
use hoh_loader::{StartupCompositionDiagnostic, StartupError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
fn error(reason: BoxError) -> StartupError {
    StartupError {
        message: "dsh: startup failed: 1 required plugin did not activate".into(),
        configuration_path: Some("/example/cordis.yml".into()),
        composition: StartupCompositionDiagnostic {
            plugins: vec![],
            services: vec![],
            omitted_plugins: 0,
            omitted_services: 0,
        },
        failures: vec!["activation failed".into()],
        messages: vec!["captured loader failure".into()],
        entries: vec![
            StartupEntryDiagnostic {
                id: "webserver".into(),
                module: "./webserver.mjs".into(),
                required: true,
                fiber_state: Some(hoh_kernel::FiberState::Failed),
                outcome: StartupOutcome::Failed {
                    error: reason.to_string(),
                },
            },
            StartupEntryDiagnostic {
                id: "waiting".into(),
                module: "./waiting.mjs".into(),
                required: false,
                fiber_state: Some(hoh_kernel::FiberState::Pending),
                outcome: StartupOutcome::Pending {
                    missing: vec!["webServer".into()],
                },
            },
        ],
        cause: Some(ActivationErrors {
            errors: vec![hoh_kernel::KernelError::Failed(Arc::new(reason))],
        }),
    }
}
fn context(root: &std::path::Path) -> StartupDiagnosticContext {
    StartupDiagnosticContext {
        home: root.into(),
        version: "1.2.3".into(),
        profile: "web".into(),
    }
}
fn facts(id: &str) -> ReportFacts {
    ReportFacts {
        timestamp: "2026-01-02T03:04:05.678Z".into(),
        uuid: id.into(),
        platform: "linux".into(),
        arch: "x64".into(),
    }
}
const ID: &str = "00000000-0000-4000-8000-000000000001";
#[tokio::test(start_paused = true)]
async fn prints_the_summary_and_saved_path_to_stderr_by_default() {
    let root = tempfile::tempdir().unwrap();
    let mut stderr = String::new();
    let error = error("failed".into());
    report_startup_failure_with_facts(&error, &context(root.path()), &facts(ID), |text| {
        stderr.push_str(&text);
        std::future::ready(Ok(()))
    })
    .await
    .unwrap();
    let path = root
        .path()
        .join(format!("logs/startup-2026-01-02T03-04-05.678Z-{ID}.log"));
    assert_eq!(
        stderr,
        format!(
            "{}\n\nДиагностика: {}\n",
            error.message,
            path.display()
        )
    );
    assert!(path.exists());
}
#[tokio::test(start_paused = true)]
async fn waits_for_stderr_completion_before_resolving() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let error = error("failed".into());
    let (tx, mut rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, tokio::sync::oneshot::Sender<()>)>();
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let job = tokio::spawn(async move {
        report_startup_failure_with_facts(&error, &ctx, &facts(ID), move |text| {
            let (ack, wait) = tokio::sync::oneshot::channel();
            tx.send((text, ack)).unwrap();
            async move {
                wait.await.unwrap();
                Ok(())
            }
        })
        .await
        .unwrap();
        done.store(true, Ordering::Release);
    });
    let (first, ack) = rx.recv().await.unwrap();
    assert_eq!(
        first,
        "dsh: startup failed: 1 required plugin did not activate\n"
    );
    assert!(!root.path().join("logs").exists());
    ack.send(()).unwrap();
    let (second, ack) = rx.recv().await.unwrap();
    assert!(second.starts_with("\nДиагностика: "));
    assert!(!finished.load(Ordering::Acquire));
    ack.send(()).unwrap();
    job.await.unwrap();
    assert!(finished.load(Ordering::Acquire));
}
#[tokio::test(start_paused = true)]
async fn rejects_when_stderr_cannot_complete_the_write() {
    let root = tempfile::tempdir().unwrap();
    let error = error("failed".into());
    let failure = Box::new(std::io::Error::other("stderr closed"));
    let pointer = (&*failure as *const std::io::Error) as usize;
    let mut failure = Some(failure);
    let actual =
        report_startup_failure_with_facts(&error, &context(root.path()), &facts(ID), |_| {
            std::future::ready(Err(failure.take().unwrap() as BoxError))
        })
        .await
        .unwrap_err()
        .downcast::<std::io::Error>()
        .unwrap();
    assert_eq!((&*actual as *const std::io::Error) as usize, pointer);
    assert_eq!(actual.to_string(), "stderr closed");
    assert!(!root.path().join("logs").exists());
}
#[derive(thiserror::Error)]
#[error("cannot listen")]
struct Detail {
    code: &'static str,
    payload: String,
    hidden_detail: String,
    values: Vec<String>,
}
impl std::fmt::Debug for Detail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detail")
            .field("code", &self.code)
            .field("payload", &self.payload)
            .field("hidden_detail", &self.hidden_detail)
            .field("values", &self.values)
            .finish()
    }
}
#[derive(Debug)]
struct Cyclic;
impl std::fmt::Display for Cyclic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cyclic cause")
    }
}
impl std::error::Error for Cyclic {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self)
    }
}
#[tokio::test(start_paused = true)]
async fn native_source_cycles_are_reported_once_without_recursion() {
    let root = tempfile::tempdir().unwrap();
    let error = error(Box::new(Cyclic));
    report_startup_failure_with_facts(&error, &context(root.path()), &facts(ID), |_| {
        std::future::ready(Ok(()))
    })
    .await
    .unwrap();
    let path = root
        .path()
        .join(format!("logs/startup-2026-01-02T03-04-05.678Z-{ID}.log"));
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("cyclic cause"));
    assert_eq!(
        hoh_loader::startup_diagnostics::format_activation_error(&error)
            .matches("cyclic cause")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn retains_original_error_properties_causes_aggregate_members_cycles_and_long_values() {
    let root = tempfile::tempdir().unwrap();
    let large = format!("{}END_OF_VALUE", "x".repeat(12_000));
    let leaf: BoxError = Box::new(Detail {
        code: "EADDRINUSE",
        payload: large.clone(),
        hidden_detail: "non-enumerable detail".into(),
        values: (0..105).map(|i| format!("value-{i}")).collect(),
    });
    let mut error = error(leaf);
    error
        .cause
        .as_mut()
        .unwrap()
        .errors
        .push(hoh_kernel::KernelError::Failed(Arc::new(
            "transport: closed".into(),
        )));
    report_startup_failure_with_facts(&error, &context(root.path()), &facts(ID), |_| {
        std::future::ready(Ok(()))
    })
    .await
    .unwrap();
    let files = std::fs::read_dir(root.path().join("logs"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(files.len(), 1);
    let report = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(report.starts_with(WARNING));
    for text in [
        "dshVersion: \"1.2.3\"",
        "profile: \"web\"",
        "configurationPath:",
        "/example/cordis.yml",
        "./waiting.mjs",
        "required: false",
        "webServer",
        "messages:",
        "EADDRINUSE",
        "non-enumerable detail",
        "value-104",
        "transport: closed",
        large.as_str(),
    ] {
        assert!(report.contains(text), "missing {text}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(files[0].path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(root.path().join("logs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn equal_messages_from_distinct_errors_survive_a_shared_address_aggregate_wrapper() {
    #[derive(Debug)]
    #[repr(transparent)]
    struct Wrapper(ActivationErrors);
    impl std::fmt::Display for Wrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Display::fmt(&self.0, f)
        }
    }
    impl std::error::Error for Wrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let error = Wrapper(ActivationErrors {
        errors: (0..2)
            .map(|_| {
                hoh_kernel::KernelError::Failed(Arc::new(Box::new(std::io::Error::other(
                    "equal leaf",
                )) as BoxError))
            })
            .collect(),
    });
    let text = hoh_loader::startup_diagnostics::format_activation_error(&error);
    assert_eq!(text.matches("Plugin activation failures").count(), 1);
    assert_eq!(text.matches("equal leaf").count(), 2);
}
#[tokio::test(start_paused = true)]
async fn creates_distinct_files_for_concurrent_failures_without_replacing_earlier_reports() {
    let root = tempfile::tempdir().unwrap();
    let a = error("first".into());
    let b = error("second".into());
    let ctx = context(root.path());
    let f1 = facts(ID);
    let f2 = facts("00000000-0000-4000-8000-000000000002");
    let (a, b) = tokio::join!(
        report_startup_failure_with_facts(&a, &ctx, &f1, |_| std::future::ready(Ok(()))),
        report_startup_failure_with_facts(&b, &ctx, &f2, |_| std::future::ready(Ok(())))
    );
    a.unwrap();
    b.unwrap();
    let reports = std::fs::read_dir(root.path().join("logs"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports.iter().filter(|r| r.contains("first")).count(), 1);
    assert_eq!(reports.iter().filter(|r| r.contains("second")).count(), 1);
}
#[tokio::test(start_paused = true)]
async fn prints_the_complete_report_when_the_logs_directory_cannot_be_created() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("logs"), "blocked").unwrap();
    let error = error("CUSTOM: original details".into());
    let mut output = String::new();
    report_startup_failure_with_facts(&error, &context(root.path()), &facts(ID), |text| {
        output.push_str(&text);
        std::future::ready(Ok(()))
    })
    .await
    .unwrap();
    for text in [
        "dsh: startup failed:",
        "HoH: не удалось сохранить отчёт о запуске:",
        "Диагностика:\n",
        "CUSTOM",
        "original details",
    ] {
        assert!(output.contains(text));
    }
    assert!(!output.contains("Диагностика: "));
    assert_eq!(
        std::fs::read_to_string(root.path().join("logs")).unwrap(),
        "blocked"
    );
}
#[tokio::test(start_paused = true)]
async fn an_existing_unique_report_is_never_replaced_even_if_host_facts_collide() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let first = error("first".into());
    report_startup_failure_with_facts(&first, &ctx, &facts(ID), |_| std::future::ready(Ok(())))
        .await
        .unwrap();
    let path = root
        .path()
        .join(format!("logs/startup-2026-01-02T03-04-05.678Z-{ID}.log"));
    let before = std::fs::read(&path).unwrap();
    let second = error("second".into());
    let mut output = String::new();
    report_startup_failure_with_facts(&second, &ctx, &facts(ID), |s| {
        output.push_str(&s);
        std::future::ready(Ok(()))
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(output.contains("Диагностика:\n"));
    assert!(output.contains("second"));
}
