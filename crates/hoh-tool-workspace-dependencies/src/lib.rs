//! Bundled workspace runtime discovery and installation.

mod error;
mod runtime;
mod shim;

pub use error::{
    WORKSPACE_DEPENDENCY_ERROR_CODES, WorkspaceDependenciesError, WorkspaceDependencyError,
    WorkspaceDependencyErrorCode,
};
pub use runtime::{
    PrimaryRuntimeManifest, WorkspaceDependencies, copy_primary_runtime, install_primary_runtime,
    parse_primary_runtime, primary_runtime_python_aliases, read_primary_runtime,
    resolve_primary_runtime, workspace_dependency_paths,
};

use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_llm::ContentBlock;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, GenericCallView, ToolCallKind, ToolCallView,
    ToolRegistrationOptions, Tools, define_tool,
};
use hoh_util_values::js_json_stringify_pretty;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[schemars(length(min = 1))]
    pub source: String,
    #[schemars(length(min = 1))]
    #[schemars(with = "String")]
    #[serde(
        default,
        deserialize_with = "optional_non_null_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub root: Option<String>,
}

fn optional_non_null_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
}

#[derive(Default)]
struct Attempt {
    result: Mutex<Option<std::result::Result<WorkspaceDependencies, SharedError>>>,
    changed: tokio::sync::Notify,
}

impl Attempt {
    fn result(&self) -> Option<std::result::Result<WorkspaceDependencies, SharedError>> {
        lock(&self.result).clone()
    }

    fn finish(&self, result: std::result::Result<WorkspaceDependencies, SharedError>) {
        *lock(&self.result) = Some(result);
        self.changed.notify_waiters();
    }

    async fn wait(&self) -> std::result::Result<WorkspaceDependencies, SharedError> {
        loop {
            let changed = self.changed.notified();
            if let Some(result) = self.result() {
                return result;
            }
            changed.await;
        }
    }
}

#[derive(Clone)]
struct Preparation {
    requests: tokio::sync::mpsc::UnboundedSender<Arc<Attempt>>,
    current: Arc<Mutex<Option<Arc<Attempt>>>>,
}

impl Preparation {
    fn start(&self) -> Arc<Attempt> {
        let mut current = lock(&self.current);
        if let Some(attempt) = current.as_ref()
            && !matches!(attempt.result(), Some(Err(_)))
        {
            return attempt.clone();
        }
        let attempt = Arc::new(Attempt::default());
        *current = Some(attempt.clone());
        if let Err(stopped) = self.requests.send(attempt.clone()) {
            stopped.0.finish(Err(SharedError::new(std::io::Error::other(
                "workspace dependency preparation worker stopped",
            ))));
        }
        attempt
    }

    fn current(&self) -> Option<Arc<Attempt>> {
        lock(&self.current).clone()
    }
}

async fn prepare(config: &Config) -> runtime::Result<WorkspaceDependencies> {
    let result = match &config.root {
        None => resolve_primary_runtime(&config.source).await,
        Some(root) => install_primary_runtime(&config.source, root).await,
    };
    match result {
        Err(cause)
            if HarnessError::of(cause.as_ref())
                .is_some_and(|error| error.is_named(WorkspaceDependencyError::NAME)) =>
        {
            Err(cause)
        }
        Err(cause) => {
            let message = format!("primary runtime: preparation failed: {cause}");
            Err(Box::new(WorkspaceDependencyError::with_cause(
                message,
                WorkspaceDependencyErrorCode::PreparationFailed,
                cause,
            )))
        }
        Ok(paths) => Ok(paths),
    }
}

pub struct ToolWorkspaceDependencies;

impl Plugin for ToolWorkspaceDependencies {
    const NAME: &'static str = "tool-workspace-dependencies";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        if !runtime::is_absolute(&config.source)
            || config
                .root
                .as_deref()
                .is_some_and(|root| !runtime::is_absolute(root))
        {
            return Err(Box::new(WorkspaceDependenciesError::RelativeConfig));
        }
        let (requests, mut requested) = tokio::sync::mpsc::unbounded_channel::<Arc<Attempt>>();
        let worker_config = config.clone();
        ctx.spawn(
            "toolWorkspaceDependencies.worker",
            move |cancel| async move {
                loop {
                    let attempt = tokio::select! {
                        _ = cancel.cancelled() => break,
                        attempt = requested.recv() => match attempt {
                            Some(attempt) => attempt,
                            None => break,
                        },
                    };
                    // A source Promise already running is not cancelled by plugin disposal.
                    let result = prepare(&worker_config).await.map_err(SharedError::new);
                    attempt.finish(result);
                }
                Ok(())
            },
        )?;
        let preparation = Preparation {
            requests,
            current: Arc::new(Mutex::new(None)),
        };
        let on_dispose = preparation.clone();
        ctx.effect("toolWorkspaceDependencies.preparation", move || {
            Undo::future(move || async move {
                if let Some(attempt) = on_dispose.current() {
                    let _ = attempt.wait().await;
                }
                Ok(())
            })
        })?;

        let output = DefineToolOutput {
            schema: json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "python": { "type": "string", "required": true },
                    "node": { "type": "string", "description": "Absent when the payload ships no Node.js." },
                    "pnpm": { "type": "string", "description": "Absent when the payload ships no pnpm." },
                    "pythonPackages": { "type": "string", "required": true },
                    "nodePackages": { "type": "string", "description": "Absent when the payload ships no Node.js." },
                    "pythonDistributions": { "type": "object", "additionalProperties": true, "required": true, "description": "Bundled distribution names and versions recorded in runtime.json; excludes user-installed additions." }
                }
            }),
            render: Arc::new(|_, value| {
                vec![ContentBlock::text(js_json_stringify_pretty(value, 2))]
            }),
            presentation_meta: None,
        };
        let execute_preparation = preparation.clone();
        let mut options = DefineToolOptions::new(
            "load_workspace_dependencies",
            "Get absolute paths to bundled Python and library directories, plus bundled Python distribution versions. Node.js and pnpm paths are included when the payload provides them. Python includes numpy, pandas, python-docx, python-pptx, openpyxl, Pillow, lxml, and XlsxWriter. Use these libraries for Office files unless the user or workspace instructions select another environment. When Node.js and pnpm paths are returned, run pnpm with that Node executable and pnpm script path. This does not change PATH or package-manager settings.",
            json!({}),
            output,
            move |_, _| {
                // Promise creation in the source is synchronous: make the attempt visible to
                // teardown before returning the Rust future too.
                let attempt = execute_preparation.start();
                Box::pin(async move {
                    let paths = attempt
                        .wait()
                        .await
                        .map_err(|cause| Box::new(cause) as BoxError)?;
                    Ok(serde_json::to_value(paths)?)
                })
            },
        );
        options.present_call = Some(Arc::new(|_| {
            Some(ToolCallView::Generic(GenericCallView {
                title: "Load workspace dependencies".into(),
                kind: Some(ToolCallKind::Read),
                raw_input: None,
                content: None,
                locations: None,
            }))
        }));
        deps.tools.register(
            &ctx,
            define_tool(options)?,
            ToolRegistrationOptions::default(),
        )?;
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
