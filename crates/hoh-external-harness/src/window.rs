//! The window's commands for external harnesses (HOH-18, CEO 20.11): what is installed
//! (`hoh/executor/discover`), the executor new chats go to (`hoh/executor/select`) and the access
//! mode of an executor (`hoh/executor/access/get`, `hoh/executor/access/set`).
//!
//! `select` and `access/set` are privileged (`hoh_protocol::PRIVILEGED_METHODS`): the SDK server
//! has the user allow the exact call before it reaches a handler here, and refuses it without a
//! host. They are the only way the access mode and the selection change; the choice is kept next
//! to the engine's profile ([`CHOICES_FILE`]), never in a working folder, and every engine run on
//! that profile starts with it ([`restore_choices`], from the `ExternalHarness` plugin).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hoh_agent::{AgentRegistry, Agents, ExecutorSelection};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Logger, NoConfig, Plugin, Service};
use hoh_loader::ProfileContextKey;
use hoh_protocol::{
    ExecutorAccessGetResult, ExecutorAccessMode, ExecutorAccessSetParams, ExecutorAccessSetResult,
    ExecutorEffectiveAccess, ExecutorParams, ExecutorSelectParams,
};
use hoh_sdk_jsonrpc_server::HohRpcMethods;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::access_text;
use crate::contract::{AccessMode, VersionStatus};
use crate::redact::{redact, redact_value};
use crate::{Choices, ExternalHarnesses, ExternalHarnessesService};

/// The file in the engine's profile folder that keeps the window's choices.
pub const CHOICES_FILE: &str = "external-harness.json";

const DISCOVER: &str = "hoh/executor/discover";
const SELECT: &str = "hoh/executor/select";
const ACCESS_GET: &str = "hoh/executor/access/get";
const ACCESS_SET: &str = "hoh/executor/access/set";
const METHODS: [&str; 4] = [DISCOVER, SELECT, ACCESS_GET, ACCESS_SET];

/// The saved choices.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    #[serde(default)]
    access: BTreeMap<String, AccessMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<SavedSelection>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedSelection {
    executor: String,
    model: String,
}

#[derive(Inject)]
pub struct WindowDeps {
    harnesses: Service<ExternalHarnesses>,
    agents: Service<Agents>,
    methods: Service<HohRpcMethods>,
    profile: Service<ProfileContextKey>,
}

/// Serves the four `hoh/executor/*` methods of the window.
pub struct ExternalHarnessWindowPlugin;

impl Plugin for ExternalHarnessWindowPlugin {
    const NAME: &'static str = "ExternalHarnessWindow";
    type Config = NoConfig;
    type Deps = WindowDeps;

    async fn apply(ctx: Context, deps: WindowDeps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let window = Arc::new(Window {
            harnesses: deps.harnesses,
            agents: deps.agents,
            path: deps.profile.dir.join(CHOICES_FILE),
            saving: tokio::sync::Mutex::new(()),
        });
        for method in METHODS {
            let window = window.clone();
            deps.methods.register(&ctx, method, move |params| {
                let window = window.clone();
                async move { window.call(method, params).await }
            })?;
        }
        Ok(())
    }
}

struct Window {
    harnesses: Service<ExternalHarnesses>,
    agents: Service<Agents>,
    path: PathBuf,
    /// One save at a time, so the file holds the last choice.
    saving: tokio::sync::Mutex<()>,
}

impl Window {
    async fn call(&self, method: &str, params: Value) -> Result<Value, BoxError> {
        match method {
            DISCOVER => {
                let params: ExecutorParams = parse(method, params)?;
                self.discover(&params.executor).await
            }
            SELECT => self.select(parse(method, params)?).await,
            ACCESS_GET => {
                let params: ExecutorParams = parse(method, params)?;
                Ok(serde_json::to_value(self.access_get(&params.executor)?)?)
            }
            ACCESS_SET => Ok(serde_json::to_value(
                self.access_set(parse(method, params)?).await?,
            )?),
            _ => Err(format!("external harness window does not serve {method}").into()),
        }
    }

    fn service(&self) -> &ExternalHarnessesService {
        &self.harnesses
    }

    fn known(&self, executor: &str) -> Result<(), BoxError> {
        if self.service().get(executor).is_none() {
            return Err(format!("no external harness named \"{executor}\"").into());
        }
        Ok(())
    }

    /// A fresh discovery every time (the window's "check again"), masked like everything else
    /// the harness says. Nothing here reads the harness's sign-in files.
    async fn discover(&self, executor: &str) -> Result<Value, BoxError> {
        let harness = self
            .service()
            .get(executor)
            .ok_or_else(|| format!("no external harness named \"{executor}\""))?;
        let (found, status) = harness.discover_with_status().await?;
        let status = match &found.version {
            Some(_) if found.installed => status,
            _ => VersionStatus::Unknown,
        };
        let warning = match (&found.version, status) {
            (Some(version), VersionStatus::Unsupported) if found.installed => Some(format!(
                "Версия {version} не входит в проверенный диапазон: работа возможна, но не \
                 гарантирована"
            )),
            (Some(version), VersionStatus::Unknown) if found.installed => Some(format!(
                "Версию {version} не удалось сверить с проверенным диапазоном"
            )),
            _ => None,
        };
        let detail = match (found.detail, warning) {
            (Some(detail), Some(warning)) => Some(format!("{detail}\n{warning}")),
            (detail, warning) => detail.or(warning),
        };
        Ok(json!({
            "executor": executor,
            "installed": found.installed,
            "version": found.version.as_deref().map(redact),
            "versionStatus": status,
            "auth": found.auth,
            "models": redact_value(&json!(found.models)),
            "detail": detail.as_deref().map(redact),
        }))
    }

    async fn select(&self, params: ExecutorSelectParams) -> Result<Value, BoxError> {
        let selection = match params.executor {
            Some(executor) => {
                self.known(&executor)?;
                Some(ExecutorSelection {
                    executor,
                    model: params.model.clone(),
                })
            }
            None => None,
        };
        let previous = self.agents.selected_executor();
        self.agents.select_executor(selection.clone());
        if let Err(error) = self.save().await {
            self.agents.select_executor(previous);
            return Err(error);
        }
        Ok(json!({
            "executor": selection.as_ref().map(|chosen| chosen.executor.clone()),
            "model": params.model,
        }))
    }

    fn access_get(&self, executor: &str) -> Result<ExecutorAccessGetResult, BoxError> {
        let shared = self
            .service()
            .shared(executor)
            .ok_or_else(|| format!("no external harness named \"{executor}\""))?;
        let mode = shared.access();
        // The running process's own launch; without one, what the next start would use.
        let (running, flags) = shared.running_launch().unwrap_or_else(|| {
            let policy = shared.config.policy;
            let next = crate::agent::launch_policy(&policy, mode);
            (mode, shared.harness.launch_flags(&next, mode))
        });
        let selected = self.agents.selected_executor();
        Ok(ExecutorAccessGetResult {
            executor: executor.to_owned(),
            mode: wire(mode),
            effective: ExecutorEffectiveAccess {
                mode: wire(running),
                flags,
                text: access_text(executor, running),
            },
            selected: hoh_protocol::ExecutorSelection {
                executor: selected.as_ref().map(|chosen| chosen.executor.clone()),
                model: selected.map(|chosen| chosen.model),
            },
        })
    }

    /// The user's choice of mode; a running native session takes it from its next turn.
    async fn access_set(
        &self,
        params: ExecutorAccessSetParams,
    ) -> Result<ExecutorAccessSetResult, BoxError> {
        self.known(&params.executor)?;
        let mode = match params.mode {
            ExecutorAccessMode::Confirm => AccessMode::Confirm,
            ExecutorAccessMode::Full => AccessMode::Full,
        };
        let choices = self.service().choices();
        let previous = choices.access(&params.executor);
        choices.set_access(&params.executor, mode);
        if let Err(error) = self.save().await {
            choices.set_access(&params.executor, previous);
            return Err(error);
        }
        Ok(ExecutorAccessSetResult {
            executor: params.executor,
            mode: params.mode,
        })
    }

    async fn save(&self) -> Result<(), BoxError> {
        let _one = self.saving.lock().await;
        let saved = Saved {
            access: self.service().choices().all_access().into_iter().collect(),
            selected: self
                .agents
                .selected_executor()
                .map(|chosen| SavedSelection {
                    executor: chosen.executor,
                    model: chosen.model,
                }),
        };
        write(&self.path, &serde_json::to_vec_pretty(&saved)?)
            .await
            .map_err(|error| {
                format!(
                    "the choice could not be saved to {}: {error}",
                    self.path.display()
                )
                .into()
            })
    }
}

/// Apply the choices saved next to the active profile, when the launcher named one; a missing or
/// unusable file leaves the defaults.
pub(crate) async fn restore_choices(ctx: &Context, choices: &Choices, agents: &AgentRegistry) {
    let Some(profile) = ctx.get::<ProfileContextKey>() else {
        return;
    };
    let path = profile.dir.join(CHOICES_FILE);
    let logger: Logger = ctx.logger();
    let saved: Saved = match tokio::fs::read(&path).await {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(saved) => saved,
            Err(error) => {
                logger.warn(format_args!(
                    "saved external harness choices {} are unusable: {error}",
                    path.display()
                ));
                return;
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            logger.warn(format_args!(
                "saved external harness choices {} could not be read: {error}",
                path.display()
            ));
            return;
        }
    };
    for (executor, mode) in saved.access {
        choices.set_access(&executor, mode);
    }
    agents.select_executor(saved.selected.map(|chosen| ExecutorSelection {
        executor: chosen.executor,
        model: chosen.model,
    }));
}

fn wire(mode: AccessMode) -> ExecutorAccessMode {
    match mode {
        AccessMode::Confirm => ExecutorAccessMode::Confirm,
        AccessMode::Full => ExecutorAccessMode::Full,
    }
}

fn parse<T: DeserializeOwned>(method: &str, params: Value) -> Result<T, BoxError> {
    serde_json::from_value(params).map_err(|_| format!("invalid {method} params").into())
}

async fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    hoh_atomic_write::write_file_atomic(
        path,
        bytes.to_vec(),
        hoh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: None,
        },
    )
    .await
}
