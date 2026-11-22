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

use hoh_agent::{AgentRegistry, AgentStatus, Agents, ExecutorSelection, stored_executor_binding};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Logger, NoConfig, Plugin, Service};
use hoh_loader::ProfileContextKey;
use hoh_protocol::{
    ExecutorAccessGetParams, ExecutorAccessGetResult, ExecutorAccessMode, ExecutorAccessSetParams,
    ExecutorAccessSetResult, ExecutorEffectiveAccess, ExecutorParams, ExecutorSelectParams,
    ExecutorSessionAccess,
};
use hoh_sdk_jsonrpc_server::HohRpcMethods;
use hoh_session::SessionId;
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
            ctx: ctx.clone(),
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

/// One choice of the window, to be written and then applied.
enum Change {
    Select(Option<ExecutorSelection>),
    Access(String, AccessMode),
}

struct Window {
    ctx: Context,
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
                let params: ExecutorAccessGetParams = parse(method, params)?;
                Ok(serde_json::to_value(
                    self.access_get(&params.executor, params.session_id.as_deref())
                        .await?,
                )?)
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
                "Версия {version} не поддерживается: чат с ней не начнётся"
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
        self.commit(Change::Select(selection.clone())).await?;
        Ok(json!({
            "executor": selection.as_ref().map(|chosen| chosen.executor.clone()),
            "model": params.model,
        }))
    }

    async fn access_get(
        &self,
        executor: &str,
        session_id: Option<&str>,
    ) -> Result<ExecutorAccessGetResult, BoxError> {
        let shared = self
            .service()
            .shared(executor)
            .ok_or_else(|| format!("no external harness named \"{executor}\""))?;
        let mode = shared.access();
        let session = match session_id {
            Some(id) => Some(self.session_access(id).await?),
            None => None,
        };
        // The process's own launch: that of the conversation asked about when it runs on this
        // executor, else the one started last; without one, what the next start would use.
        let own = session
            .as_ref()
            .filter(|found| found.executor.as_deref() == Some(executor))
            .and_then(|_| session_id.and_then(|id| shared.running_launch_of(id)));
        let launch = match session_id {
            Some(_) => own,
            None => shared.running_launch(),
        };
        let (running, flags) = launch.unwrap_or_else(|| {
            let policy = shared.config.policy;
            let next = crate::agent::launch_policy(&policy, mode);
            (mode, shared.harness.launch_flags(&next, mode))
        });
        let selected = self.agents.selected_executor();
        Ok(ExecutorAccessGetResult {
            session,
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

    /// What conversation `id` runs with: its own executor, model and mode, from its binding in the
    /// engine, never from the choice for new chats. A live agent says it (the provider route of an
    /// external one is the harness's name); a stored one that is not open is known by the binding
    /// record of its log, without a model.
    async fn session_access(&self, id: &str) -> Result<ExecutorSessionAccess, BoxError> {
        let id = SessionId::from(id);
        let live = self
            .agents
            .get(&id)
            .and_then(|agent| agent.model().current());
        let (executor, model) = match live {
            Some(chosen) if self.service().get(&chosen.provider).is_some() => {
                (Some(chosen.provider), Some(chosen.model))
            }
            Some(_) => (None, None),
            None => (
                stored_executor_binding(&self.ctx, &id)
                    .await?
                    .filter(|name| self.service().get(name).is_some()),
                None,
            ),
        };
        let Some(name) = executor else {
            return Ok(ExecutorSessionAccess {
                executor: None,
                model: None,
                privacy: None,
                mode: None,
                running: None,
                pending: false,
            });
        };
        let shared = self
            .service()
            .shared(&name)
            .ok_or_else(|| format!("no external harness named \"{name}\""))?;
        let mode = shared.access();
        let running = shared.running_launch_of(id.as_ref()).map(|(mode, _)| mode);
        Ok(ExecutorSessionAccess {
            executor: Some(name),
            model: model.map(|model| redact(&model)),
            // The conversation's words go to the harness's service on the internet.
            privacy: Some("remote".to_owned()),
            mode: Some(wire(mode)),
            running: running.map(wire),
            pending: running.is_some_and(|running| running != mode),
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
        self.commit(Change::Access(params.executor.clone(), mode))
            .await?;
        self.retire_idle(&params.executor, mode).await;
        Ok(ExecutorAccessSetResult {
            executor: params.executor,
            mode: params.mode,
        })
    }

    /// A native process of `executor` that idles between turns in another mode than the one just
    /// chosen ends now, so that nothing it starts on its own (a background turn) runs in the
    /// old mode; the conversation's next message starts it anew in the chosen one. A process in
    /// the middle of a turn is left alone: the turn ends in the mode it began in.
    async fn retire_idle(&self, executor: &str, mode: AccessMode) {
        let Some(shared) = self.service().shared(executor) else {
            return;
        };
        let agents = self.agents.clone();
        let idle = |id: &str| {
            agents
                .get(&SessionId::from(id))
                .is_none_or(|agent| agent.status() == AgentStatus::Idle)
        };
        for session in shared.retire_idle(mode, idle) {
            session.terminate(shared.terminate_grace()).await;
        }
    }

    /// The window's choice, applied only once it is on disk: the write comes first, under one lock
    /// for every choice, so that two decisions in a row take effect in order and a refused write
    /// leaves the earlier state exactly as it was (and tells the human).
    async fn commit(&self, change: Change) -> Result<(), BoxError> {
        let _one = self.saving.lock().await;
        let mut access: BTreeMap<String, AccessMode> =
            self.service().choices().all_access().into_iter().collect();
        let mut selected = self.agents.selected_executor();
        match &change {
            Change::Select(selection) => selected.clone_from(selection),
            Change::Access(executor, mode) => {
                access.insert(executor.clone(), *mode);
            }
        }
        let saved = Saved {
            access,
            selected: selected.map(|chosen| SavedSelection {
                executor: chosen.executor,
                model: chosen.model,
            }),
        };
        write(&self.path, &serde_json::to_vec_pretty(&saved)?)
            .await
            .map_err(|error| -> BoxError {
                format!(
                    "the choice could not be saved to {}: {error}",
                    self.path.display()
                )
                .into()
            })?;
        match change {
            Change::Select(selection) => self.agents.select_executor(selection),
            Change::Access(executor, mode) => self.service().choices().set_access(&executor, mode),
        }
        Ok(())
    }
}

/// Apply the choices saved next to the active profile, when the launcher named one; a missing or
/// unusable file leaves the defaults. The engine's launcher always names the profile before the
/// plugins start (the window's path: `t8_…` in `apps/hoh/tests/window_executor.rs`); without one
/// the defaults stay, and the log says so.
pub(crate) async fn restore_choices(ctx: &Context, choices: &Choices, agents: &AgentRegistry) {
    let logger: Logger = ctx.logger();
    let Some(profile) = ctx.get::<ProfileContextKey>() else {
        logger.warn(format_args!(
            "no active profile: saved external harness choices are not read, the defaults apply"
        ));
        return;
    };
    let path = profile.dir.join(CHOICES_FILE);
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
