mod support;
use hoh_kernel::{Cfg, Context, Error, Kernel, KernelError, Plugin, Undo};
use hoh_loader::startup_diagnostics::*;
use hoh_loader::*;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{live::*, *};

async fn rejected(text: &str) -> (Kernel, LoadError) {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("cordis.yml");
    std::fs::write(&base, text).unwrap();
    let kernel = Kernel::start();
    let mut registry = PluginRegistry::default();
    registry.add::<Noop>("noop").add::<Fails>("fails");
    let error = boot(
        &kernel.root(),
        BootOptions {
            registry: Arc::new(registry),
            base,
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .err()
    .unwrap();
    (kernel, error)
}
#[tokio::test(start_paused = true)]
async fn ignores_active_disabled_and_absent_required_entries() {
    for text in [
        "[]",
        "- {id: agent-loop, name: noop}",
        "- {id: agent-loop, name: fails, disabled: true}",
    ] {
        let world = World::new(text).await;
        let mut warnings = vec![];
        audit_startup_entries(&world.loader, "dsh-test-bin", &mut |warning| {
            warnings.push(warning)
        })
        .await
        .unwrap();
        assert!(warnings.is_empty());
        world.close().await;
    }
}
#[tokio::test(start_paused = true)]
async fn warns_once_for_optional_import_apply_and_dependency_failures() {
    let world=World::new("- {id: missing-tool, name: absent}\n- {id: tool-todo, name: fails}\n- {id: waiting-tool, name: noop, inject: [missing]}\n").await;
    let mut warnings = vec![];
    audit_startup_entries(&world.loader, "dsh-test-bin", &mut |warning| {
        warnings.push(warning)
    })
    .await
    .unwrap();
    assert_eq!(warnings,vec!["dsh-test-bin: warning: 3 entries did not activate\nmissing-tool (absent): failed to import: cannot resolve plugin absent\ntool-todo (fails): activation boom\nwaiting-tool (noop): pending (waiting for service: missing)\n".to_owned()]);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn reports_a_throwing_disabled_expression_on_tool_todo_required_false() {
    let world =
        World::new("- id: tool-todo\n  name: noop\n  disabled: !!js ctx.absent.value\n").await;
    let mut warnings = vec![];
    audit_startup_entries(&world.loader, "test", &mut |warning| warnings.push(warning))
        .await
        .unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("tool-todo (noop): disabled expression failed:"));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn reports_a_throwing_disabled_expression_on_webserver_required_true() {
    let (kernel, error) =
        rejected("- id: webserver\n  name: noop\n  disabled: !!js ctx.absent.value\n").await;
    assert!(
        error
            .to_string()
            .contains("webserver (required)\n    Package: noop\n    disabled expression failed:")
    );
    kernel.shutdown().await;
}
#[derive(Debug, thiserror::Error)]
#[error("plugin activation failed")]
struct Wrapped(#[source] KernelError);
struct Aggregate;
impl Plugin for Aggregate {
    const NAME: &'static str = "aggregate";
    type Config = Value;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<Value>) -> Result<(), Error> {
        let leaf = Arc::new(Error::from("tool discovery failed"));
        Err(Box::new(Wrapped(KernelError::Listeners {
            event: "connection failed",
            errors: vec![leaf.clone(), leaf, Arc::new("transport closed".into())],
        })))
    }
}
#[tokio::test(start_paused = true)]
async fn preserves_nested_activation_causes_and_aggregate_member_failures() {
    let world = World::registry("- {id: wrapped-plugin, name: aggregate}", |r| {
        r.add::<Aggregate>("aggregate");
    })
    .await;
    let mut warnings = vec![];
    audit_startup_entries(&world.loader, "test", &mut |warning| warnings.push(warning))
        .await
        .unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("plugin activation failed"));
    assert!(warnings[0].contains("transport closed"));
    assert_eq!(warnings[0].matches("tool discovery failed").count(), 1);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn combines_required_and_optional_failures_without_a_separate_warning() {
    for id in [
        "agent-loop",
        "tool-executor-native",
        "tool-executor-ptc",
        "webserver",
        "web-runtime",
        "modules",
        "connection",
        "headless-runner",
        "acp",
        "sdk-jsonrpc-server",
    ] {
        let (kernel, error) = rejected(&format!(
            "- {{id: {id}, name: fails}}\n- {{id: tool-todo, name: fails}}\n"
        ))
        .await;
        let LoadError::Startup(error) = error else {
            panic!("expected startup error")
        };
        assert!(
            error
                .message
                .contains("hoh: startup failed: 1 required plugin did not activate")
        );
        assert!(
            error
                .message
                .contains(&format!("  {id} (required)\n    Package: fails"))
        );
        assert!(error.message.contains("  tool-todo\n    Package: fails"));
        assert_eq!(error.cause.unwrap().errors.len(), 2);
        kernel.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn omits_an_error_cause_when_required_plugins_are_only_waiting_for_services() {
    let (kernel, error) = rejected("- {id: connection, name: noop, inject: [webStartup]}").await;
    let LoadError::Startup(error) = error else {
        panic!("expected startup error")
    };
    assert!(error.cause.is_none());
    assert!(error.message.contains("Plugins waiting for services (1):"));
    assert!(!format!("{error:?}").contains("ActivationErrors"));
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_diagnostic_metadata_available_without_expanding_it_in_ordinary_error_inspection() {
    let (kernel, error) = rejected("- {id: connection, name: noop, inject: [webStartup]}").await;
    let LoadError::Startup(error) = error else {
        panic!("expected startup error")
    };
    assert_eq!(error.entries[0].module, "noop");
    assert!(!format!("{error:?}").contains("StartupPluginState"));
    assert!(!format!("{error:?}").contains("include:connection"));
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_nested_and_shared_errors_in_a_fatal_diagnostic_without_duplicating_them() {
    let world = World::registry("- {id: optional, name: aggregate}", |r| {
        r.add::<Aggregate>("aggregate");
    })
    .await;
    let error = world
        .patch("- insert: [{id: webserver, name: aggregate}]")
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string().matches("tool discovery failed").count(),
        1
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn groups_original_failure_stacks_and_pending_services_in_one_startup_diagnostic() {
    let(kernel,error)=rejected("- {id: web-runtime, name: noop, inject: [webServer]}\n- {id: webserver, name: fails}\n- {id: connection, name: noop, inject: [webStartup]}\n- {id: unknown, name: noop, inject: [unknown]}\n").await;
    assert_eq!(
        error.to_string(),
        "hoh: startup failed: 3 required plugins did not activate\n\nFailed plugins (1):\n  webserver (required)\n    Package: fails\n    activation boom\n\nPlugins waiting for services (3):\n  Plugin                  Missing services\n  web-runtime (required)  webServer\n  connection (required)   webStartup\n  unknown                 unknown"
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_required_entry_pending_on_an_injected_service() {
    let (kernel, error) =
        rejected("- {id: headless-runner, name: noop, inject: [headlessStartup]}").await;
    assert!(
        error
            .to_string()
            .contains("headless-runner (required)  headlessStartup")
    );
    kernel.shutdown().await;
}
#[test]
fn loads_expressions_and_rejects_missing_malformed_non_array_and_non_mapping_overlays() {
    assert_eq!(
        patches("- id: target\n  config:\n    value: !!js process.env.VALUE\n")[0].set["config"],
        json!({"value":{"__jsExpr":"process.env.VALUE"}})
    );
    for text in ["x: [", "{}", "- scalar"] {
        assert!(parse_patches(text, std::path::Path::new("/overlay.yml")).is_err());
    }
}
#[tokio::test(start_paused = true)]
async fn retains_import_errors_and_inactive_entry_metadata_after_disposing_the_startup_tree() {
    let (kernel, error) = rejected(
        "- {id: webserver, name: missing}\n- {id: waiting, name: noop, inject: [missingService]}",
    )
    .await;
    let LoadError::Startup(error) = error else {
        panic!("expected startup error")
    };
    assert!(kernel.root().get::<LoaderKey>().is_none());
    assert!(
        error
            .messages
            .iter()
            .any(|message| message == "cannot resolve plugin missing")
    );
    assert!(
        error
            .composition
            .plugins
            .iter()
            .any(|p| p.id == "include:waiting" && p.missing == ["missingService"])
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_warnings_and_errors_from_asynchronous_failed_startup_cleanup() {
    struct Cleanup;
    impl Plugin for Cleanup {
        const NAME: &'static str = "cleanup";
        type Config = Value;
        type Deps = ();
        async fn apply(ctx: Context, _: (), _: Cfg<Value>) -> Result<(), Error> {
            let log = ctx.logger();
            ctx.effect("cleanup diagnostic", move || {
                Undo::future(move || async move {
                    tokio::task::yield_now().await;
                    log.warn("cleanup warning");
                    log.error("cleanup error");
                    Ok(())
                })
            })?;
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("cordis.yml");
    std::fs::write(
        &base,
        "- {id: cleanup, name: cleanup}\n- {id: webserver, name: fails}",
    )
    .unwrap();
    let kernel = Kernel::start();
    let mut registry = PluginRegistry::default();
    registry.add::<Cleanup>("cleanup").add::<Fails>("fails");
    let error = boot(
        &kernel.root(),
        BootOptions {
            registry: Arc::new(registry),
            base,
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .err()
    .unwrap();
    let LoadError::Startup(error) = error else {
        panic!("expected startup error")
    };
    assert!(error.messages.contains(&"cleanup warning".into()));
    assert!(error.messages.contains(&"cleanup error".into()));
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn boots_a_leaf_config_through_the_real_loader_and_settles_the_tree() {
    let world = World::new("- {id: leaf, name: noop, config: {value: booted}}").await;
    assert!(world.active("leaf"));
    assert_eq!(
        *locked(&world.observed.values),
        vec![json!({"value":"booted"})]
    );
    assert!(world.loader.audit(&[]).inactive.is_empty());
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn exposes_dsh_home_path_and_dsh_path_to_loader_config_expressions() {
    let world=World::new("- id: capture\n  name: noop\n  config:\n    path: !!js dshHomePath('sessions')\n    state: !!js dshHomePath('state', 'capture')\n").await;
    let cfg = world
        .row("capture")
        .fiber
        .unwrap()
        .effective_config()
        .unwrap();
    assert_eq!(
        cfg["path"],
        world.dir.path().join("home/sessions").to_str().unwrap()
    );
    assert_eq!(
        cfg["state"],
        world
            .dir
            .path()
            .join("home/state/capture")
            .to_str()
            .unwrap()
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_successful_entries_and_warns_about_optional_import_config_disabled_sync_apply_async_apply_and_dependency_failures()
 {
    let world=World::new("- {id: good, name: noop}\n- {id: absent, name: missing}\n- {id: apply, name: fails}\n- {id: config, name: objects, config: {fixed: 3}}\n- id: disabled\n  name: noop\n  disabled: !!js ctx.absent.value\n- {id: pending, name: noop, inject: [missing]}\n").await;
    assert!(world.active("good"));
    let mut warnings = vec![];
    audit_startup_entries(&world.loader, "test", &mut |warning| warnings.push(warning))
        .await
        .unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with("test: warning: 5 entries did not activate\n"));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn disposes_successful_entries_and_rejects_when_a_required_entry_fails() {
    let (kernel, error) =
        rejected("- {id: good, name: noop}\n- {id: webserver, name: fails}").await;
    assert!(matches!(error, LoadError::Startup(_)));
    let snapshot = kernel.snapshot().await;
    assert!(snapshot.fibers.is_empty());
    assert!(kernel.root().get::<LoaderKey>().is_none());
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn runs_host_preparation_before_the_loader_tree_mounts() {
    hoh_kernel::service!(Prepared: () = "prepared");
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("cordis.yml");
    std::fs::write(
        &base,
        "- {id: leaf, name: noop, inject: [prepared], config: {value: mounted}}\n",
    )
    .unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root().with_tag(Observed::default());
    let observed = ctx.tag::<Observed>().unwrap();
    ctx.provide::<Prepared>(Arc::new(())).await.unwrap();
    locked(&observed.values).push(json!("prepared"));
    let mut registry = PluginRegistry::default();
    registry.add::<Noop>("noop");
    boot(
        &ctx,
        BootOptions {
            registry: Arc::new(registry),
            base,
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        *locked(&observed.values),
        vec![json!("prepared"), json!({"value":"mounted"})]
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_root_configuration() {
    for text in [None, Some("invalid: [unclosed\n"), Some("entries: []\n")] {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cordis.yml");
        if let Some(text) = text {
            std::fs::write(&base, text).unwrap();
        }
        let kernel = Kernel::start();
        let error = boot(
            &kernel.root(),
            BootOptions {
                registry: Arc::new(PluginRegistry::default()),
                base: base.clone(),
                layers: vec![],
                env: env(dir.path()),
            },
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(error, LoadError::Startup(_)), "{error}");
        let message = match text {
            None => "config file not found",
            Some("invalid: [unclosed\n") => "unexpected end",
            _ => "top-level array",
        };
        assert!(error.to_string().contains(message), "{error}");
        assert!(matches!(
            kernel.root().ensure_active(),
            Err(KernelError::Closed)
        ));
        kernel.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn disposes_startup_after_a_required_failure() {
    struct AsyncFails;
    impl Plugin for AsyncFails {
        const NAME: &'static str = "async-fails";
        type Config = Value;
        type Deps = ();
        async fn apply(_: Context, _: (), _: Cfg<Value>) -> Result<(), Error> {
            tokio::task::yield_now().await;
            Err("async failure".into())
        }
    }
    for (name, extra, message) in [
        ("missing", "", "cannot resolve plugin missing"),
        ("objects", ", config: {fixed: 3}", "expected string"),
        (
            "noop",
            ", config: {value: {__jsExpr: \"JSON.parse('invalid')\"}}",
            "Unexpected token",
        ),
        (
            "noop",
            ", disabled: {__jsExpr: \"JSON.parse('invalid')\"}",
            "disabled expression failed:",
        ),
        ("fails", "", "activation boom"),
        ("async", "", "async failure"),
        (
            "noop",
            ", inject: [missingRequiredService]",
            "missingRequiredService",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cordis.yml");
        std::fs::write(&base, format!("- {{id: webserver, name: {name}{extra}}}\n")).unwrap();
        let kernel = Kernel::start();
        let disposed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let copy = disposed.clone();
        kernel
            .root()
            .effect("host cleanup", move || {
                Undo::sync(move || copy.store(true, std::sync::atomic::Ordering::Release))
            })
            .unwrap();
        let mut registry = PluginRegistry::default();
        registry
            .add::<Noop>("noop")
            .add::<Objects>("objects")
            .add::<Fails>("fails")
            .add::<AsyncFails>("async");
        let error = boot(
            &kernel.root(),
            BootOptions {
                registry: Arc::new(registry),
                base,
                layers: vec![],
                env: env(dir.path()),
            },
        )
        .await
        .err()
        .unwrap();
        assert!(error.to_string().contains(message), "{error}");
        assert!(disposed.load(std::sync::atomic::Ordering::Acquire));
        kernel.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn returns_instead_of_asserting_over_a_tree_a_surface_disposed_mid_startup() {
    struct Exits;
    impl Plugin for Exits {
        const NAME: &'static str = "exits";
        type Config = Value;
        type Deps = ();
        async fn apply(ctx: Context, _: (), _: Cfg<Value>) -> Result<(), Error> {
            ctx.root().fiber().dispose().await;
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("cordis.yml");
    std::fs::write(&base, "- {id: exiting, name: exits}").unwrap();
    let kernel = Kernel::start();
    let mut registry = PluginRegistry::default();
    registry.add::<Exits>("exits");
    let error = boot(
        &kernel.root(),
        BootOptions {
            registry: Arc::new(registry),
            base,
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .err()
    .unwrap();
    assert_eq!(
        error.to_string(),
        "hoh: plugin tree failed to load: inactive context"
    );
    assert!(
        matches!(error,LoadError::Boot(source) if matches!(*source,LoadError::Kernel(KernelError::Inactive)))
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn returns_when_disposal_completes_before_root_entry_creation_returns() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("cordis.yml");
    std::fs::write(&base, "[]").unwrap();
    let kernel = Kernel::start();
    kernel.root().fiber().dispose().await;
    let error = boot(
        &kernel.root(),
        BootOptions {
            registry: Arc::new(PluginRegistry::default()),
            base,
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .err()
    .unwrap();
    assert_eq!(
        error.to_string(),
        "hoh: plugin tree failed to load: kernel has shut down"
    );
    assert!(
        matches!(error,LoadError::Boot(source) if matches!(*source,LoadError::Kernel(KernelError::Closed)))
    );
    kernel.shutdown().await;
}
#[path = "support/launcher_helpers.rs"]
mod launcher_helpers;
