//! Detached startup reports never retain configs, service values or contexts.
use crate::{EntryStatus, LoadError, Loader, LoaderKey};
use hoh_kernel::{Context, FiberState, Snapshot};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct StartupPluginState {
    pub id: String,
    pub module: String,
    pub version: String,
    pub state: String,
    pub source: String,
    pub missing: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct StartupProviderState {
    pub entry: String,
    pub state: String,
    pub visibility: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct StartupServiceState {
    pub consumer: String,
    pub service: String,
    pub providers: Vec<StartupProviderState>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupCompositionDiagnostic {
    pub plugins: Vec<StartupPluginState>,
    pub services: Vec<StartupServiceState>,
    pub omitted_plugins: usize,
    pub omitted_services: usize,
}
#[derive(thiserror::Error)]
#[error("{message}")]
pub struct StartupError {
    pub message: String,
    pub configuration_path: Option<std::path::PathBuf>,
    pub composition: StartupCompositionDiagnostic,
    pub failures: Vec<String>,
    pub messages: Vec<String>,
    pub entries: Vec<StartupEntryDiagnostic>,
    #[source]
    pub cause: Option<ActivationErrors>,
}
impl std::fmt::Debug for StartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StartupError")
            .field("message", &self.message)
            .field("cause", &self.cause)
            .finish()
    }
}
#[derive(Debug, thiserror::Error)]
#[error("Plugin activation failures")]
pub struct ActivationErrors {
    pub errors: Vec<hoh_kernel::KernelError>,
}
#[derive(Clone, Debug)]
pub struct StartupEntryDiagnostic {
    pub id: String,
    pub module: String,
    pub required: bool,
    pub fiber_state: Option<FiberState>,
    pub outcome: StartupOutcome,
}
#[derive(Clone, Debug)]
pub enum StartupOutcome {
    Failed { error: String },
    Pending { missing: Vec<String> },
}
/// Preserve nested causes and aggregate members once each, without retaining configurations.
pub fn format_activation_error(error: &(dyn std::error::Error + 'static)) -> String {
    let mut pending = vec![error];
    let mut seen = std::collections::HashSet::new();
    let mut rendered = std::collections::HashSet::new();
    let mut lines = vec![];
    while let Some(error) = pending.pop() {
        // An error and its first source field can share an address; retain the trait metadata too.
        if !seen.insert(std::ptr::from_ref(error)) {
            continue;
        }
        if let Some(hoh_kernel::KernelError::Failed(inner)) =
            error.downcast_ref::<hoh_kernel::KernelError>()
        {
            pending.push(inner.as_ref().as_ref());
            continue;
        }
        // Trait upcasts may use different vtables for the same concrete error.
        // Keep walking those nodes (including aggregates), but render their message once.
        let message = error.to_string();
        if rendered.insert((std::ptr::from_ref(error).cast::<()>(), message.clone())) {
            lines.push(message);
        }
        if let Some(aggregate) = error.downcast_ref::<ActivationErrors>() {
            for error in aggregate.errors.iter().rev() {
                pending.push(error);
            }
        }
        if let Some(hoh_kernel::KernelError::Listeners { errors, .. }) =
            error.downcast_ref::<hoh_kernel::KernelError>()
        {
            for error in errors.iter().rev() {
                pending.push(error.as_ref().as_ref());
            }
        }
        if let Some(source) = error.source() {
            pending.push(source);
        }
    }
    lines.join("\n")
}
pub(crate) fn state_name(state: FiberState) -> &'static str {
    match state {
        FiberState::Pending => "pending",
        FiberState::Loading => "loading",
        FiberState::Active => "active",
        FiberState::Failed => "failed",
        FiberState::Disposed => "disposed",
        FiberState::Unloading => "unloading",
    }
}
pub async fn capture_startup_composition(
    ctx: &Context,
) -> Result<StartupCompositionDiagnostic, LoadError> {
    let loader = ctx
        .get::<LoaderKey>()
        .ok_or_else(|| LoadError::Runtime("Startup composition diagnostic unavailable".into()))?;
    Ok(capture(&loader).await)
}
pub async fn capture(loader: &Loader) -> StartupCompositionDiagnostic {
    let audit = loader.audit(&[]);
    let mut plugins: Vec<_> = loader
        .records()
        .into_iter()
        .map(|node| {
            let entry = node.info;
            let state = if entry.disabled {
                "disabled"
            } else if entry.status == EntryStatus::Unresolved {
                "failed"
            } else {
                entry
                    .fiber
                    .as_ref()
                    .map(|f| state_name(f.state()))
                    .unwrap_or("failed")
            };
            let missing = if state == "pending" {
                audit
                    .inactive
                    .iter()
                    .find(|f| {
                        entry
                            .fiber
                            .as_ref()
                            .is_some_and(|fiber| fiber.uid() == f.uid)
                    })
                    .map(|f| f.missing.iter().map(ToString::to_string).collect())
                    .unwrap_or_default()
            } else {
                vec![]
            };
            StartupPluginState {
                id: entry.id.clone(),
                module: entry.name,
                version: "unknown".into(),
                state: state.into(),
                source: node.source.to_string_lossy().into_owned(),
                missing,
            }
        })
        .collect();
    plugins.sort_by_key(|p| p.state == "active");
    let all_services: Vec<_> = plugins
        .iter()
        .flat_map(|plugin| {
            plugin.missing.iter().map(|service| StartupServiceState {
                consumer: plugin.id.clone(),
                service: service.clone(),
                providers: vec![],
            })
        })
        .collect();
    let omitted_plugins = plugins.len().saturating_sub(256);
    let omitted_services = all_services.len().saturating_sub(256);
    plugins.truncate(256);
    let report = StartupCompositionDiagnostic {
        plugins,
        services: all_services.into_iter().take(256).collect(),
        omitted_plugins,
        omitted_services,
    };
    with_providers(loader, &loader.provision_store().await, report)
}
fn with_providers(
    loader: &Loader,
    snapshot: &Snapshot,
    mut report: StartupCompositionDiagnostic,
) -> StartupCompositionDiagnostic {
    for service in &mut report.services {
        for provision in snapshot
            .provisions
            .iter()
            .filter(|p| p.name.as_ref() == service.service)
        {
            let Some(provider) = snapshot.fibers.iter().find(|f| f.uid == provision.owner) else {
                continue;
            };
            let mut owner = provider;
            let mut visited = indexmap::IndexSet::new();
            while owner.entry_id.is_none() && owner.parent != owner.uid && visited.insert(owner.uid)
            {
                let Some(parent) = snapshot.fibers.iter().find(|f| f.uid == owner.parent) else {
                    break;
                };
                owner = parent;
            }
            service.providers.push(StartupProviderState {
                entry: owner
                    .entry_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "unknown".into()),
                state: state_name(provider.state).into(),
                visibility: if loader.realm(&service.consumer, &service.service)
                    == Some(provision.realm)
                {
                    "same-scope".into()
                } else {
                    "other-scope".into()
                },
            });
        }
    }
    report
}

const REQUIRED: &[&str] = &[
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
];

/// Application startup policy: missing and disabled required ids are harmless; present inactive ones reject.
pub async fn audit_startup_entries(
    loader: &Loader,
    bin_name: &str,
    warn: &mut dyn FnMut(String),
) -> Result<(), LoadError> {
    loader.settled().await;
    let failures: Vec<_> = loader
        .records()
        .into_iter()
        .filter(|node| !node.info.disabled)
        .filter_map(|node| {
            node.failure().map(|failure| {
                let cause = node.cause();
                (node.info, failure, cause)
            })
        })
        .collect();
    let required: Vec<_> = failures
        .iter()
        .filter(|(e, _, _)| e.id == "include" || REQUIRED.contains(&e.options.id.as_str()))
        .collect();
    if !required.is_empty() {
        let mut lines = vec![format!(
            "{bin_name}: startup failed: {} required {} did not activate",
            required.len(),
            if required.len() == 1 {
                "plugin"
            } else {
                "plugins"
            }
        )];
        let mut failed = vec![];
        let mut pending = vec![];
        for (entry, failure, _) in &failures {
            let label = format!(
                "{}{}",
                entry.options.id,
                if entry.id == "include" || REQUIRED.contains(&entry.options.id.as_str()) {
                    " (required)"
                } else {
                    ""
                }
            );
            if entry
                .fiber
                .as_ref()
                .is_some_and(|f| f.state() == FiberState::Pending)
            {
                let audit = loader.audit(&[]);
                let missing = audit
                    .inactive
                    .iter()
                    .find(|f| {
                        entry
                            .fiber
                            .as_ref()
                            .is_some_and(|fiber| fiber.uid() == f.uid)
                    })
                    .map(|f| {
                        f.missing
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                pending.push((
                    label,
                    if missing.is_empty() {
                        "unknown".into()
                    } else {
                        missing
                    },
                ));
            } else {
                failed.push((
                    label,
                    entry.name.clone(),
                    failure
                        .split_once(": ")
                        .map(|(_, detail)| detail)
                        .unwrap_or(failure)
                        .to_owned(),
                ));
            }
        }
        if !failed.is_empty() {
            lines.push(String::new());
            lines.push(format!("Failed plugins ({}):", failed.len()));
            for (label, package, detail) in failed {
                lines.push(format!("  {label}"));
                lines.push(format!("    Package: {package}"));
                lines.extend(detail.lines().map(|line| format!("    {line}")));
            }
        }
        if !pending.is_empty() {
            pending.sort_by_key(|(label, _)| !label.ends_with(" (required)"));
            let width = pending
                .iter()
                .map(|(label, _)| label.encode_utf16().count())
                .chain([6])
                .max()
                .unwrap_or(6)
                + 2;
            lines.push(String::new());
            lines.push(format!("Plugins waiting for services ({}):", pending.len()));
            lines.push(format!("  Plugin{}Missing services", " ".repeat(width - 6)));
            for (label, missing) in pending {
                let padding = width - label.encode_utf16().count();
                lines.push(format!("  {label}{}{missing}", " ".repeat(padding)));
            }
        }
        let entries = failures
            .iter()
            .map(|(entry, failure, _)| {
                let pending = entry
                    .fiber
                    .as_ref()
                    .is_some_and(|f| f.state() == FiberState::Pending);
                let missing = loader
                    .audit(&[])
                    .inactive
                    .into_iter()
                    .find(|f| {
                        entry
                            .fiber
                            .as_ref()
                            .is_some_and(|fiber| fiber.uid() == f.uid)
                    })
                    .map(|f| f.missing.into_iter().map(|s| s.to_string()).collect())
                    .unwrap_or_default();
                StartupEntryDiagnostic {
                    id: entry.options.id.clone(),
                    module: entry.name.clone(),
                    required: entry.id == "include"
                        || REQUIRED.contains(&entry.options.id.as_str()),
                    fiber_state: entry.fiber.as_ref().map(|f| f.state()),
                    outcome: if pending {
                        StartupOutcome::Pending { missing }
                    } else {
                        StartupOutcome::Failed {
                            error: failure.clone(),
                        }
                    },
                }
            })
            .collect();
        let causes: Vec<_> = failures
            .iter()
            .filter(|(entry, _, _)| {
                entry
                    .fiber
                    .as_ref()
                    .is_none_or(|f| f.state() != FiberState::Pending)
            })
            .filter_map(|(_, _, cause)| cause.clone())
            .collect();
        return Err(LoadError::Startup(Box::new(StartupError {
            message: lines.join("\n"),
            configuration_path: None,
            composition: capture(loader).await,
            failures: failures
                .into_iter()
                .map(|(_, failure, _)| failure)
                .collect(),
            messages: vec![],
            entries,
            cause: (!causes.is_empty()).then_some(ActivationErrors { errors: causes }),
        })));
    }
    if !failures.is_empty() {
        warn(format!(
            "{bin_name}: warning: {} {} did not activate\n{}\n",
            failures.len(),
            if failures.len() == 1 {
                "entry"
            } else {
                "entries"
            },
            failures
                .iter()
                .map(|(_, f, _)| f.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(())
}
