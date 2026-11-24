//! The window's commands for external harnesses (HOH-18, CEO 20.11): what is installed
//! (`hoh/executor/discover`), the executor new chats go to (`hoh/executor/select`) and the access
//! mode of an executor (`hoh/executor/access/get`, `hoh/executor/access/set`). HOH-41 adds what
//! the window's `/`, `$` and `@` offer for a folder: the executor's own commands and skills
//! (`hoh/executor/commands`) and the folder's files (`hoh/workspace/entries`). Both answer only for
//! a folder the engine knows ([`Window::known_folder`]), never for any path the page names; a
//! project folder becomes known when the person adds it (`hoh/workspace/add`, privileged, kept in
//! [`FOLDERS_FILE`]).
//!
//! `select` and `access/set` are privileged (`hoh_protocol::PRIVILEGED_METHODS`): the SDK server
//! has the user allow the exact call before it reaches a handler here, and refuses it without a
//! host. They are the only way the access mode and the selection change; the choice is kept next
//! to the engine's profile ([`CHOICES_FILE`]), never in a working folder, and every engine run on
//! that profile starts with it ([`restore_choices`], from the `ExternalHarness` plugin).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hoh_agent::{AgentRegistry, AgentStatus, Agents, ExecutorSelection, stored_executor_binding};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Logger, NoConfig, Plugin, Service};
use hoh_loader::ProfileContextKey;
use hoh_protocol::{
    ExecutorAccessGetParams, ExecutorAccessGetResult, ExecutorAccessMode, ExecutorAccessSetParams,
    ExecutorAccessSetResult, ExecutorCommand, ExecutorCommandsParams, ExecutorCommandsResult,
    ExecutorEffectiveAccess, ExecutorParams, ExecutorRuntimeMode, ExecutorSelectParams,
    ExecutorSessionAccess, ExecutorTitleGenerateParams, SessionInteractionMode, WorkspaceAddParams,
    WorkspaceAddResult, WorkspaceEntriesParams, WorkspaceEntriesResult,
};
use hoh_sdk_jsonrpc_server::HohRpcMethods;
use hoh_session::SessionId;
use hoh_working_directory::WorkingDirectoryKey;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::access_text;
use crate::contract::{
    AccessMode, Discovery, HarnessCommand, HarnessCommands, InteractionMode, VersionStatus,
};
use crate::redact::{redact, redact_value};
use crate::{Choices, ExternalHarnesses, ExternalHarnessesService};

/// The file in the engine's profile folder that keeps the window's choices.
pub const CHOICES_FILE: &str = "external-harness.json";
/// Exact per-conversation modes. Kept separate so older HoH versions ignore it instead of
/// rejecting their legacy choices file when it contains a newer four-mode value.
pub const SESSION_ACCESS_FILE: &str = "external-harness-session-access.json";
/// Codex selection for new conversations. Kept in a versioned sidecar so an older HoH ignores
/// an executor it does not know instead of rejecting or restoring it from the legacy file.
pub const CODEX_SELECTION_FILE: &str = "external-harness-codex-selection.json";
/// The file in the engine's profile folder that keeps the project folders the person added.
pub const FOLDERS_FILE: &str = "workspace-folders.json";

const DISCOVER: &str = "hoh/executor/discover";
const SELECT: &str = "hoh/executor/select";
const ACCESS_GET: &str = "hoh/executor/access/get";
const ACCESS_SET: &str = "hoh/executor/access/set";
const COMMANDS: &str = "hoh/executor/commands";
const ENTRIES: &str = "hoh/workspace/entries";
const ADD: &str = "hoh/workspace/add";
const FOLDER_CACHE_TTL: Duration = Duration::from_secs(30);
const TITLE_GENERATE: &str = "hoh/executor/title/generate";
const METHODS: [&str; 8] = [
    DISCOVER,
    SELECT,
    ACCESS_GET,
    ACCESS_SET,
    COMMANDS,
    ENTRIES,
    ADD,
    TITLE_GENERATE,
];

/// The saved choices.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    #[serde(default)]
    access: BTreeMap<String, ExecutorAccessMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<SavedSelection>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedSelection {
    executor: String,
    model: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedSessionAccess {
    version: u32,
    sessions: BTreeMap<String, crate::SessionAccessChoice>,
}

fn build_15_mode(mode: AccessMode) -> AccessMode {
    if mode == AccessMode::AcceptEdits {
        AccessMode::AutoAcceptEdits
    } else {
        mode
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedCodexSelection {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<SavedSelection>,
}

#[derive(Inject)]
pub struct WindowDeps {
    harnesses: Service<ExternalHarnesses>,
    agents: Service<Agents>,
    methods: Service<HohRpcMethods>,
    profile: Service<ProfileContextKey>,
}

/// Serves the `hoh/executor/*` methods of the window and its `hoh/workspace/entries`.
pub struct ExternalHarnessWindowPlugin;

impl Plugin for ExternalHarnessWindowPlugin {
    const NAME: &'static str = "ExternalHarnessWindow";
    type Config = NoConfig;
    type Deps = WindowDeps;

    async fn apply(ctx: Context, deps: WindowDeps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let folders_path = deps.profile.dir.join(FOLDERS_FILE);
        let folders = match tokio::fs::read(&folders_path).await {
            Ok(bytes) => serde_json::from_slice::<Vec<PathBuf>>(&bytes).unwrap_or_else(|error| {
                ctx.logger().warn(format_args!(
                    "project folders {} are unusable: {error}",
                    folders_path.display()
                ));
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        // The default spelling may itself be a symlink. Remember the target accepted at plugin
        // start; resolving the spelling again on every request would authorize a later swap.
        let default_folder = match ctx.get::<WorkingDirectoryKey>() {
            Some(directories) => tokio::fs::canonicalize(directories.default_directory())
                .await
                .ok(),
            None => None,
        };
        let window = Arc::new(Window {
            ctx: ctx.clone(),
            harnesses: deps.harnesses,
            agents: deps.agents,
            path: deps.profile.dir.join(CHOICES_FILE),
            session_access_path: deps.profile.dir.join(SESSION_ACCESS_FILE),
            codex_selection_path: deps.profile.dir.join(CODEX_SELECTION_FILE),
            folders_path,
            folders: std::sync::Mutex::new(folders.into_iter().collect()),
            engine_home: deps.profile.home.clone(),
            default_folder,
            saving: tokio::sync::Mutex::new(()),
            folder_calls: std::sync::Mutex::new(BTreeMap::new()),
        });
        for method in METHODS {
            let window = window.clone();
            deps.methods.register(&ctx, method, move |params| {
                let window = window.clone();
                async move { window.call(method, params).await }
            })?;
        }
        // Bind effort, interaction and access to this message before it can be dequeued.
        let choices = window.service().choices().clone();
        let log = ctx.logger();
        deps.methods
            .on_prompt(&ctx, move |session, message, params| {
                let effort = match params.get("effort") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(effort)) => Some(effort.clone()),
                    Some(_) => {
                        log.warn(format_args!(
                            "session/prompt effort is not a string: the turn runs without one"
                        ));
                        None
                    }
                };
                choices.note_prompt(session, message, effort);
                let interaction = match params.get("interactionMode") {
                    None | Some(Value::Null) => Some(InteractionMode::Default),
                    Some(value) => {
                        match serde_json::from_value::<SessionInteractionMode>(value.clone()) {
                            Ok(SessionInteractionMode::Default) => Some(InteractionMode::Default),
                            Ok(SessionInteractionMode::Plan) => Some(InteractionMode::Plan),
                            Err(error) => {
                                // RPC admission normally rejects this before creating the message.
                                // Keep the listener fail-closed too: an unknown restricted mode must
                                // never become an ordinary (possibly full-access) prompt choice.
                                log.error(format_args!(
                                    "rejected invalid session/prompt interactionMode: {error}"
                                ));
                                None
                            }
                        }
                    }
                };
                if let Some(interaction) = interaction {
                    choices.note_interaction(session, message, interaction);
                }
            })?;
        Ok(())
    }
}

/// One choice of the window, to be written and then applied.
enum Change {
    Select(Option<ExecutorSelection>),
    Access(String, AccessMode),
    SessionAccess {
        session: String,
        executor: String,
        mode: AccessMode,
    },
}

struct Window {
    ctx: Context,
    harnesses: Service<ExternalHarnesses>,
    agents: Service<Agents>,
    path: PathBuf,
    session_access_path: PathBuf,
    codex_selection_path: PathBuf,
    folders_path: PathBuf,
    /// The project folders the person added, resolved (HOH-41).
    folders: std::sync::Mutex<std::collections::BTreeSet<PathBuf>>,
    /// The engine's own data (keys, sessions): never a project folder.
    engine_home: PathBuf,
    /// Canonical identity of the deployment default accepted when this plugin started.
    default_folder: Option<PathBuf>,
    /// One save at a time, so the file holds the last choice.
    saving: tokio::sync::Mutex<()>,
    /// Expensive composer calls, serialized and briefly cached by canonical folder.
    folder_calls: std::sync::Mutex<BTreeMap<PathBuf, Arc<tokio::sync::Mutex<FolderCalls>>>>,
}

#[derive(Default)]
struct FolderCalls {
    commands: BTreeMap<String, Cached<HarnessCommands>>,
    entries: BTreeMap<EntriesKey, Cached<WorkspaceEntriesResult>>,
}

struct Cached<T> {
    at: Instant,
    revision: u64,
    value: T,
}

impl<T: Clone> Cached<T> {
    fn is_fresh(&self) -> bool {
        self.at.elapsed() < FOLDER_CACHE_TTL
    }

    fn fresh(&self) -> Option<T> {
        self.is_fresh().then(|| self.value.clone())
    }

    fn fresh_at(&self, revision: u64) -> Option<T> {
        (self.revision == revision).then(|| self.fresh()).flatten()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EntriesKey {
    query: String,
    limit: u32,
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
            COMMANDS => Ok(serde_json::to_value(
                self.commands(parse(method, params)?).await?,
            )?),
            ENTRIES => Ok(serde_json::to_value(
                self.entries(parse(method, params)?).await?,
            )?),
            ADD => Ok(serde_json::to_value(
                self.add_folder(parse(method, params)?).await?,
            )?),
            TITLE_GENERATE => {
                let params: ExecutorTitleGenerateParams = parse(method, params)?;
                let harness = self
                    .service()
                    .get(&params.executor)
                    .ok_or_else(|| format!("no external harness named {:?}", params.executor))?;
                Ok(json!({ "title": harness.generate_title(params.prompt).await? }))
            }
            _ => Err(format!("external harness window does not serve {method}").into()),
        }
    }

    /// The folder `cwd` names, when the engine knows it (HOH-41) and it is not the engine's own
    /// data: the default folder of new chats,
    /// a project folder the person added ([`Window::add_folder`]), or the folder of a conversation
    /// open in the engine — a folder the person chose in the main window
    /// (`session/working-directory/set`); both are confirmed and journalled there. Both sides are
    /// resolved (links, `..`) before they are compared, so another spelling of a known folder
    /// passes and nothing else does. The answer is the resolved folder.
    async fn known_folder(&self, cwd: &str) -> Result<PathBuf, BoxError> {
        let unknown = || -> BoxError {
            format!("{cwd:?} is not the folder of a conversation of this window").into()
        };
        if !Path::new(cwd).is_absolute() {
            return Err(unknown());
        }
        let wanted = tokio::fs::canonicalize(cwd).await.map_err(|_| unknown())?;
        // This is repeated on every use, not only when a project is registered: a saved path may
        // have been replaced with a link since then.
        if let Some(reason) = self.folder_refusal(&wanted).await {
            return Err(format!("{cwd:?} {reason}").into());
        }
        // A composition without working directories knows no folder.
        let directories = self.ctx.get::<WorkingDirectoryKey>().ok_or_else(unknown)?;
        // Check registrations before active conversations. Otherwise a conversation that still
        // names the registered spelling could make the new target of a replaced symlink pass as
        // an active folder before the stale registration is examined.
        let saved: Vec<PathBuf> = lock_folders(&self.folders).iter().cloned().collect();
        for folder in saved {
            match saved_folder_match(&folder, &wanted).await {
                Some(true) => return Ok(wanted),
                Some(false) => return Err(unknown()),
                None => {}
            }
        }
        // Unlike the request spelling, accepted identities never follow a newly introduced link.
        // The default target was frozen at startup; WorkingDirectoryService stores canonical paths
        // for session changes, so the same saved-identity check covers active-only conversations.
        if let Some(default) = &self.default_folder {
            match saved_folder_match(default, &wanted).await {
                Some(true) => return Ok(wanted),
                Some(false) => return Err(unknown()),
                None => {}
            }
        }
        for folder in self
            .agents
            .list()
            .iter()
            .map(|agent| directories.get(agent.session()))
        {
            match saved_folder_match(Path::new(&folder), &wanted).await {
                Some(true) => return Ok(wanted),
                Some(false) => return Err(unknown()),
                None => {}
            }
        }
        Err(unknown())
    }

    /// `/` and `$` of the window: what the executor itself declares for a known folder, asked
    /// without a turn (masked like everything else the harness says).
    async fn commands(
        &self,
        params: ExecutorCommandsParams,
    ) -> Result<ExecutorCommandsResult, BoxError> {
        let harness = self
            .service()
            .get(&params.executor)
            .ok_or_else(|| format!("no external harness named \"{}\"", params.executor))?;
        // Without a folder: the engine's default one ("No project" of the window).
        let cwd = match params.cwd {
            Some(cwd) => cwd,
            None => self
                .ctx
                .get::<WorkingDirectoryKey>()
                .ok_or("the engine has no default folder")?
                .default_directory()
                .to_owned(),
        };
        let folder = self.known_folder(&cwd).await?;
        let calls = self.folder_calls(&folder);
        let mut calls = calls.lock().await;
        let revision = harness.commands_revision(&folder);
        let found = match calls
            .commands
            .get(&params.executor)
            .and_then(|cached| cached.fresh_at(revision))
        {
            Some(found) => found,
            None => {
                // A real session may finish initialization while the safe probe is in flight.
                // Retry against its in-memory snapshot until the revision around the answer is
                // stable, so a probe can never be cached under the real snapshot's revision.
                let mut answer_revision = revision;
                let found = loop {
                    let found = harness.commands(folder.clone()).await?;
                    let current = harness.commands_revision(&folder);
                    if current == answer_revision {
                        break found;
                    }
                    answer_revision = current;
                };
                calls.commands.retain(|_, cached| cached.is_fresh());
                calls.commands.insert(
                    params.executor.clone(),
                    Cached {
                        at: Instant::now(),
                        revision: answer_revision,
                        value: found.clone(),
                    },
                );
                found
            }
        };
        let wire = |list: Vec<HarnessCommand>| {
            list.into_iter()
                .map(|command| ExecutorCommand {
                    name: redact(&command.name),
                    description: command.description.as_deref().map(redact),
                    argument_hint: command.argument_hint.as_deref().map(redact),
                })
                .collect()
        };
        Ok(ExecutorCommandsResult {
            executor: params.executor,
            cwd,
            commands: wire(found.commands),
            skills: wire(found.skills),
        })
    }

    /// `@` of the window: entries of a known folder by relative path ([`crate::entries`]).
    async fn entries(
        &self,
        params: WorkspaceEntriesParams,
    ) -> Result<WorkspaceEntriesResult, BoxError> {
        if params.limit == 0 || params.limit > crate::entries::MAX_LIMIT {
            return Err(format!("limit must be from 1 to {}", crate::entries::MAX_LIMIT).into());
        }
        if params.query.chars().count() > crate::entries::MAX_QUERY_CHARS {
            return Err(format!(
                "query is longer than {} characters",
                crate::entries::MAX_QUERY_CHARS
            )
            .into());
        }
        let folder = self.known_folder(&params.cwd).await?;
        let key = EntriesKey {
            query: params.query.clone(),
            limit: params.limit,
        };
        let calls = self.folder_calls(&folder);
        let mut calls = calls.lock().await;
        if let Some(found) = calls.entries.get(&key).and_then(Cached::fresh) {
            return Ok(found);
        }
        let (entries, truncated) = tokio::task::spawn_blocking(move || {
            crate::entries::search(&folder, &params.query, params.limit as usize)
        })
        .await?;
        let found = WorkspaceEntriesResult { entries, truncated };
        calls.entries.retain(|_, cached| cached.is_fresh());
        calls.entries.insert(
            key,
            Cached {
                at: Instant::now(),
                revision: 0,
                value: found.clone(),
            },
        );
        Ok(found)
    }

    /// A folder the person added as a project in the main window (`hoh/workspace/add`, privileged:
    /// the SDK server has the host confirm the exact call first). It must be an existing folder;
    /// `~` is the user's home folder. Sensitive system and home folders and the engine's own data
    /// are refused after links have been resolved. Kept on disk before it counts, so a refused
    /// write changes nothing.
    async fn add_folder(&self, params: WorkspaceAddParams) -> Result<WorkspaceAddResult, BoxError> {
        let typed = params.cwd.as_str();
        let path = match typed.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                let home = std::env::var_os("HOME").ok_or("the home folder is unknown")?;
                PathBuf::from(home).join(rest.trim_start_matches('/'))
            }
            _ => PathBuf::from(typed),
        };
        if !path.is_absolute() {
            return Err(format!("{typed:?} is not a full path of a folder").into());
        }
        let folder = tokio::fs::canonicalize(&path)
            .await
            .map_err(|error| format!("the folder {typed:?} cannot be opened: {error}"))?;
        if !tokio::fs::metadata(&folder).await?.is_dir() {
            return Err(format!("{typed:?} is not a folder").into());
        }
        if let Some(reason) = self.folder_refusal(&folder).await {
            return Err(format!("{typed:?} {reason} and cannot be a project").into());
        }
        let _one = self.saving.lock().await;
        let mut folders = lock_folders(&self.folders).clone();
        if folders.insert(folder.clone()) {
            let list: Vec<&PathBuf> = folders.iter().collect();
            write(&self.folders_path, &serde_json::to_vec_pretty(&list)?)
                .await
                .map_err(|error| -> BoxError {
                    format!(
                        "the project folder could not be saved to {}: {error}",
                        self.folders_path.display()
                    )
                    .into()
                })?;
            *lock_folders(&self.folders) = folders;
        }
        Ok(WorkspaceAddResult {
            cwd: folder.to_string_lossy().into_owned(),
        })
    }

    /// `folder` (resolved) is the engine's home, inside it or around it.
    async fn holds_engine_data(&self, folder: &Path) -> bool {
        let home = tokio::fs::canonicalize(&self.engine_home)
            .await
            .unwrap_or_else(|_| self.engine_home.clone());
        folder.starts_with(&home) || home.starts_with(folder)
    }

    /// Why a canonical folder cannot be used by the composer's `@`, `/` or `$`.
    async fn folder_refusal(&self, folder: &Path) -> Option<&'static str> {
        if self.holds_engine_data(folder).await {
            return Some("holds the data of HoH");
        }
        let user_home = std::env::var_os("HOME").map(PathBuf::from);
        // An unusable HOME is not a trusted path. Treat it like a missing HOME so the policy
        // fails closed for the conventional /home tree instead of protecting a fictitious path.
        let user_home = match user_home {
            Some(home) => tokio::fs::canonicalize(home).await.ok(),
            None => None,
        };
        protected_folder(folder, user_home.as_deref())
            .then_some("is a protected system or home folder")
    }

    fn service(&self) -> &ExternalHarnessesService {
        &self.harnesses
    }

    fn folder_calls(&self, folder: &Path) -> Arc<tokio::sync::Mutex<FolderCalls>> {
        lock_folder_calls(&self.folder_calls)
            .entry(folder.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(FolderCalls::default())))
            .clone()
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
        Ok(Self::discovery_value(executor, found, status))
    }

    fn discovery_value(executor: &str, found: Discovery, status: VersionStatus) -> Value {
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
        let mut value = json!({
            "executor": executor,
            "installed": found.installed,
            "version": found.version.as_deref().map(redact),
            "versionStatus": status,
            "auth": found.auth,
            "models": redact_value(&json!(found.models)),
            "detail": detail.as_deref().map(redact),
        });
        if let Some(failure) = found.failure {
            value["failure"] = json!(failure);
        }
        value
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
        let requested = session_id.map_or(mode, |id| shared.access_for(id));
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
            let next = crate::agent::launch_policy(&policy, requested);
            (requested, shared.harness.launch_flags(&next, requested))
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
                runtime_mode: None,
                running_runtime_mode: None,
                pending: false,
            });
        };
        let shared = self
            .service()
            .shared(&name)
            .ok_or_else(|| format!("no external harness named \"{name}\""))?;
        let mode = shared.access_for(id.as_ref());
        let running = shared.running_launch_of(id.as_ref()).map(|(mode, _)| mode);
        Ok(ExecutorSessionAccess {
            executor: Some(name),
            model: model.map(|model| redact(&model)),
            // The conversation's words go to the harness's service on the internet.
            privacy: Some("remote".to_owned()),
            mode: Some(wire(mode)),
            running: running.map(wire),
            runtime_mode: Some(runtime_wire(mode)),
            running_runtime_mode: running.map(runtime_wire),
            pending: running.is_some_and(|running| running != mode),
        })
    }

    /// The user's choice of mode; a running native session takes it from its next turn.
    async fn access_set(
        &self,
        params: ExecutorAccessSetParams,
    ) -> Result<ExecutorAccessSetResult, BoxError> {
        self.known(&params.executor)?;
        let mode = match params.runtime_mode.as_ref() {
            Some(ExecutorRuntimeMode::Confirm) => AccessMode::Confirm,
            Some(ExecutorRuntimeMode::AutoAcceptEdits) if params.executor == "claude" => {
                AccessMode::AcceptEdits
            }
            Some(ExecutorRuntimeMode::AutoAcceptEdits) => AccessMode::AutoAcceptEdits,
            Some(ExecutorRuntimeMode::Auto) => AccessMode::Auto,
            Some(ExecutorRuntimeMode::Full) => AccessMode::Full,
            None => match &params.mode {
                ExecutorAccessMode::Confirm => AccessMode::Confirm,
                ExecutorAccessMode::Full => AccessMode::Full,
            },
        };
        if wire(mode) != params.mode {
            return Err("runtimeMode does not match its downgrade-safe mode projection".into());
        }
        if params.session_id.is_none()
            && matches!(
                mode,
                AccessMode::AutoAcceptEdits | AccessMode::AcceptEdits | AccessMode::Auto
            )
        {
            return Err("auto access modes require a concrete sessionId".into());
        }
        if let Some(session) = params.session_id.as_deref() {
            let binding = self.session_access(session).await?;
            if binding
                .executor
                .as_deref()
                .is_some_and(|executor| executor != params.executor)
            {
                return Err("Conversation is bound to another executor".into());
            }
            self.commit(Change::SessionAccess {
                session: session.to_owned(),
                executor: params.executor.clone(),
                mode,
            })
            .await?;
            self.retire_idle_session(&params.executor, session, mode)
                .await;
        } else {
            self.commit(Change::Access(params.executor.clone(), mode))
                .await?;
            self.retire_idle(&params.executor, mode).await;
        }
        Ok(ExecutorAccessSetResult {
            executor: params.executor,
            mode: params.mode,
            runtime_mode: params.runtime_mode,
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

    async fn retire_idle_session(&self, executor: &str, session: &str, mode: AccessMode) {
        let Some(shared) = self.service().shared(executor) else {
            return;
        };
        let idle = self
            .agents
            .get(&SessionId::from(session))
            .is_none_or(|agent| agent.status() == AgentStatus::Idle);
        for control in shared.retire_idle_session(session, mode, idle) {
            control.terminate(shared.terminate_grace()).await;
        }
    }

    /// The window's choice, applied only once it is on disk: the write comes first, under one lock
    /// for every choice, so that two decisions in a row take effect in order and a refused write
    /// leaves the earlier state exactly as it was (and tells the human).
    async fn commit(&self, change: Change) -> Result<(), BoxError> {
        let _one = self.saving.lock().await;
        if let Change::SessionAccess {
            session,
            executor,
            mode,
        } = &change
        {
            let mut sessions: BTreeMap<String, crate::SessionAccessChoice> = self
                .service()
                .choices()
                .all_session_access()
                .into_iter()
                .collect();
            sessions.insert(
                session.clone(),
                crate::SessionAccessChoice {
                    executor: executor.clone(),
                    // Version 1 must remain readable by build 15, including every other entry.
                    mode: build_15_mode(*mode),
                },
            );
            for choice in sessions.values_mut() {
                choice.mode = build_15_mode(choice.mode);
            }
            let saved = SavedSessionAccess {
                version: 1,
                sessions,
            };
            write(
                &self.session_access_path,
                &serde_json::to_vec_pretty(&saved)?,
            )
            .await
            .map_err(|error| -> BoxError {
                format!(
                    "the conversation access could not be saved to {}: {error}",
                    self.session_access_path.display()
                )
                .into()
            })?;
            self.service()
                .choices()
                .set_session_access(session, executor, *mode);
            return Ok(());
        }
        let mut access: BTreeMap<String, ExecutorAccessMode> = self
            .service()
            .choices()
            .all_access()
            .into_iter()
            .map(|(executor, mode)| (executor, wire(mode)))
            .collect();
        let mut selected = self.agents.selected_executor();
        match &change {
            Change::Select(selection) => selected.clone_from(selection),
            Change::Access(executor, mode) => {
                access.insert(executor.clone(), wire(*mode));
            }
            Change::SessionAccess { .. } => unreachable!("handled above"),
        }
        let saved = Saved {
            access,
            // Codex did not exist in the legacy choices format. Keeping its global selection in
            // that file makes an older HoH restore an executor it cannot register; the versioned
            // sidecar below is ignored by old versions and restored by this one.
            selected: selected
                .as_ref()
                .filter(|chosen| chosen.executor != "codex")
                .map(|chosen| SavedSelection {
                    executor: chosen.executor.clone(),
                    model: chosen.model.clone(),
                }),
        };
        let codex = SavedCodexSelection {
            version: 1,
            selected: selected
                .as_ref()
                .filter(|chosen| chosen.executor == "codex")
                .map(|chosen| SavedSelection {
                    executor: chosen.executor.clone(),
                    model: chosen.model.clone(),
                }),
        };
        let legacy_bytes = serde_json::to_vec_pretty(&saved)?;
        let codex_bytes = serde_json::to_vec_pretty(&codex)?;
        // Selecting Codex clears it from the downgrade-safe legacy file before publishing the
        // sidecar. Leaving Codex clears the sidecar before publishing the legacy selection. A
        // crash between the two writes therefore falls back safely instead of restoring Codex
        // with an older engine or claiming a newer choice that was not fully committed.
        if codex.selected.is_some() {
            write(&self.path, &legacy_bytes)
                .await
                .map_err(|error| -> BoxError {
                    format!(
                        "the choice could not be saved to {}: {error}",
                        self.path.display()
                    )
                    .into()
                })?;
            write(&self.codex_selection_path, &codex_bytes)
                .await
                .map_err(|error| -> BoxError {
                    format!(
                        "the Codex choice could not be saved to {}: {error}",
                        self.codex_selection_path.display()
                    )
                    .into()
                })?;
        } else {
            write(&self.codex_selection_path, &codex_bytes)
                .await
                .map_err(|error| -> BoxError {
                    format!(
                        "the Codex choice could not be saved to {}: {error}",
                        self.codex_selection_path.display()
                    )
                    .into()
                })?;
            write(&self.path, &legacy_bytes)
                .await
                .map_err(|error| -> BoxError {
                    format!(
                        "the choice could not be saved to {}: {error}",
                        self.path.display()
                    )
                    .into()
                })?;
        }
        match change {
            Change::Select(selection) => self.agents.select_executor(selection),
            Change::Access(executor, mode) => self.service().choices().set_access(&executor, mode),
            Change::SessionAccess { .. } => unreachable!("handled above"),
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
    let legacy_modified = tokio::fs::metadata(&path)
        .await
        .and_then(|metadata| metadata.modified())
        .ok();
    match tokio::fs::read(&path).await {
        Ok(bytes) => match serde_json::from_slice::<Saved>(&bytes) {
            Ok(saved) => {
                for (executor, mode) in saved.access {
                    choices.set_access(
                        &executor,
                        match mode {
                            ExecutorAccessMode::Confirm => AccessMode::Confirm,
                            ExecutorAccessMode::Full => AccessMode::Full,
                        },
                    );
                }
                agents.select_executor(saved.selected.map(|chosen| ExecutorSelection {
                    executor: chosen.executor,
                    model: chosen.model,
                }));
            }
            Err(error) => logger.warn(format_args!(
                "saved external harness choices {} are unusable: {error}",
                path.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => logger.warn(format_args!(
            "saved external harness choices {} could not be read: {error}",
            path.display()
        )),
    }

    let codex_path = profile.dir.join(CODEX_SELECTION_FILE);
    let codex_modified = tokio::fs::metadata(&codex_path)
        .await
        .and_then(|metadata| metadata.modified())
        .ok();
    let codex_is_current = match (codex_modified, legacy_modified) {
        (Some(codex), Some(legacy)) => codex >= legacy,
        (Some(_), None) => true,
        _ => false,
    };
    match tokio::fs::read(&codex_path).await {
        Ok(bytes) => match serde_json::from_slice::<SavedCodexSelection>(&bytes) {
            Ok(saved) if saved.version == 1 => {
                if let Some(chosen) = saved.selected.as_ref()
                    && codex_is_current
                {
                    if chosen.executor == "codex" {
                        agents.select_executor(Some(ExecutorSelection {
                            executor: chosen.executor.clone(),
                            model: chosen.model.clone(),
                        }));
                    } else {
                        logger.warn(format_args!(
                            "saved Codex choice {} names an unexpected executor",
                            codex_path.display()
                        ));
                    }
                } else if saved.selected.is_some() && !codex_is_current {
                    logger.warn(format_args!(
                        "saved Codex choice {} is older than the legacy choice and was ignored",
                        codex_path.display()
                    ));
                }
            }
            Ok(saved) => logger.warn(format_args!(
                "saved Codex choice {} has unsupported version {}",
                codex_path.display(),
                saved.version
            )),
            Err(error) => logger.warn(format_args!(
                "saved Codex choice {} is unusable: {error}",
                codex_path.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => logger.warn(format_args!(
            "saved Codex choice {} could not be read: {error}",
            codex_path.display()
        )),
    }

    let session_path = profile.dir.join(SESSION_ACCESS_FILE);
    match tokio::fs::read(&session_path).await {
        Ok(bytes) => match serde_json::from_slice::<SavedSessionAccess>(&bytes) {
            Ok(saved) if saved.version == 1 => {
                for (session, choice) in saved.sessions {
                    choices.set_session_access(&session, &choice.executor, choice.mode);
                }
            }
            Ok(saved) => logger.warn(format_args!(
                "saved conversation access {} has unsupported version {}",
                session_path.display(),
                saved.version
            )),
            Err(error) => logger.warn(format_args!(
                "saved conversation access {} is unusable: {error}",
                session_path.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => logger.warn(format_args!(
            "saved conversation access {} could not be read: {error}",
            session_path.display()
        )),
    }
}

fn lock_folders(
    folders: &std::sync::Mutex<std::collections::BTreeSet<PathBuf>>,
) -> std::sync::MutexGuard<'_, std::collections::BTreeSet<PathBuf>> {
    folders
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_folder_calls(
    calls: &std::sync::Mutex<BTreeMap<PathBuf, Arc<tokio::sync::Mutex<FolderCalls>>>>,
) -> std::sync::MutexGuard<'_, BTreeMap<PathBuf, Arc<tokio::sync::Mutex<FolderCalls>>>> {
    calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `Some(true)`: this registration still names `wanted`; `Some(false)`: it resolves to `wanted`
/// only because the saved canonical path was replaced (for example by a symlink); `None`: this
/// registration names another folder or no longer resolves.
async fn saved_folder_match(saved: &Path, wanted: &Path) -> Option<bool> {
    tokio::fs::canonicalize(saved)
        .await
        .ok()
        .filter(|current| current == wanted)
        .map(|current| current == saved)
}

/// Folders whose names alone may disclose credentials or host configuration. Both arguments are
/// canonical: callers resolve links before this policy is evaluated.
fn protected_folder(folder: &Path, user_home: Option<&Path>) -> bool {
    if folder == Path::new("/") {
        return true;
    }
    if let Some(home) = user_home {
        // The home itself and its ancestors (for example `/home`) disclose every protected
        // child at once and are never projects.
        if home.starts_with(folder) {
            return true;
        }
        if folder
            .strip_prefix(home)
            .ok()
            .and_then(|relative| relative.components().next())
            .is_some_and(|component| match component {
                Component::Normal(name) => name.as_encoded_bytes().first() == Some(&b'.'),
                _ => false,
            })
        {
            return true;
        }
    } else if folder.starts_with("/home") {
        // Tauri can still have a passwd-derived home when HOME is absent. Fail closed for all
        // conventional homes instead of losing the home-specific rules entirely.
        return true;
    }
    const SYSTEM: &[&str] = &[
        "/etc", "/proc", "/sys", "/dev", "/run", "/boot", "/root", "/var", "/usr", "/bin", "/sbin",
    ];
    if SYSTEM.iter().any(|protected| folder.starts_with(protected)) {
        return true;
    }
    // `/lib*`: `/lib`, `/lib64`, and any other top-level name beginning with `lib`.
    folder
        .strip_prefix("/")
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| match component {
            Component::Normal(name) => name.as_encoded_bytes().starts_with(b"lib"),
            _ => false,
        })
}

fn wire(mode: AccessMode) -> ExecutorAccessMode {
    match mode {
        AccessMode::Confirm => ExecutorAccessMode::Confirm,
        AccessMode::AutoAcceptEdits | AccessMode::AcceptEdits | AccessMode::Auto => {
            ExecutorAccessMode::Confirm
        }
        AccessMode::Full => ExecutorAccessMode::Full,
    }
}

fn runtime_wire(mode: AccessMode) -> ExecutorRuntimeMode {
    match mode {
        AccessMode::Confirm => ExecutorRuntimeMode::Confirm,
        AccessMode::AutoAcceptEdits | AccessMode::AcceptEdits => {
            ExecutorRuntimeMode::AutoAcceptEdits
        }
        AccessMode::Auto => ExecutorRuntimeMode::Auto,
        AccessMode::Full => ExecutorRuntimeMode::Full,
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

#[cfg(test)]
mod tests {
    use super::{Window, build_15_mode, protected_folder, saved_folder_match};
    use crate::contract::{
        AuthState, Discovery, DiscoveryFailure, DiscoveryOperation, VersionStatus,
    };
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn saved_session_modes_are_readable_by_build_15_without_losing_confirm() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "kebab-case")]
        enum OldMode {
            Confirm,
            AutoAcceptEdits,
            Auto,
            Full,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OldChoice {
            executor: String,
            mode: OldMode,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OldFile {
            version: u32,
            sessions: std::collections::BTreeMap<String, OldChoice>,
        }
        let saved = super::SavedSessionAccess {
            version: 1,
            sessions: [
                (
                    "a".into(),
                    crate::SessionAccessChoice {
                        executor: "claude".into(),
                        mode: build_15_mode(crate::AccessMode::AcceptEdits),
                    },
                ),
                (
                    "b".into(),
                    crate::SessionAccessChoice {
                        executor: "claude".into(),
                        mode: build_15_mode(crate::AccessMode::Confirm),
                    },
                ),
            ]
            .into(),
        };
        let old: OldFile = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(old.version, 1);
        assert_eq!(old.sessions["b"].executor, "claude");
        assert_eq!(old.sessions["a"].mode, OldMode::AutoAcceptEdits);
        assert_eq!(old.sessions["b"].mode, OldMode::Confirm);
    }

    #[test]
    fn discovery_json_keeps_the_structured_probe_failure_for_the_window() {
        let value = Window::discovery_value(
            "claude",
            Discovery {
                installed: true,
                version: None,
                auth: AuthState::Unknown,
                models: Vec::new(),
                detail: Some("localized detail".into()),
                failure: Some(DiscoveryFailure::Timeout {
                    operation: DiscoveryOperation::Version,
                    timeout_seconds: 20,
                }),
            },
            VersionStatus::Unknown,
        );
        assert_eq!(
            value["failure"],
            json!({"kind":"timeout", "operation":"version", "timeoutSeconds":20})
        );
    }

    #[test]
    fn discovery_json_keeps_initialize_timeout_for_the_window() {
        let value = Window::discovery_value(
            "claude",
            Discovery {
                installed: true,
                version: Some("2.1.329".into()),
                auth: AuthState::Unknown,
                models: Vec::new(),
                detail: Some("localized detail".into()),
                failure: Some(DiscoveryFailure::Timeout {
                    operation: DiscoveryOperation::Initialize,
                    timeout_seconds: 45,
                }),
            },
            VersionStatus::Supported,
        );
        assert_eq!(
            value["failure"],
            json!({"kind":"timeout", "operation":"initialize", "timeoutSeconds":45})
        );
    }

    #[test]
    fn protected_folders_cover_root_home_hidden_home_and_system_classes() {
        let home = Path::new("/home/person");
        for path in [
            "/",
            "/home",
            "/home/person",
            "/home/person/.ssh",
            "/home/person/.ssh/keys",
            "/home/person/.config/app",
            "/etc",
            "/etc/ssh",
            "/proc",
            "/proc/1",
            "/sys",
            "/sys/kernel",
            "/dev",
            "/dev/zero",
            "/run",
            "/run/secrets",
            "/boot",
            "/boot/loader",
            "/root",
            "/root/work",
            "/var",
            "/var/lib",
            "/usr",
            "/usr/local",
            "/bin",
            "/bin/sh",
            "/sbin",
            "/sbin/init",
            "/lib",
            "/lib64",
            "/libexec/tool",
        ] {
            assert!(
                protected_folder(Path::new(path), Some(home)),
                "{path} must be protected"
            );
        }
        for path in [
            "/home/person/work",
            "/home/person/project/.hidden",
            "/home/other/.ssh",
            "/opt/project",
            "/etcetera/project",
            "/usrland/project",
        ] {
            assert!(
                !protected_folder(Path::new(path), Some(home)),
                "{path} is not in a protected class"
            );
        }
    }

    #[test]
    fn missing_home_protects_conventional_home_tree_and_root_home() {
        for path in [
            "/home",
            "/home/person",
            "/home/person/project",
            "/root/work",
        ] {
            assert!(
                protected_folder(Path::new(path), None),
                "{path} must be protected without HOME"
            );
        }
        for path in ["/homeward/project", "/mnt/project", "/opt/project"] {
            assert!(
                !protected_folder(Path::new(path), None),
                "{path} is not a conventional home"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_saved_canonical_folder_replaced_by_a_link_no_longer_matches() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("project");
        let other = dir.path().join("other");
        std::fs::create_dir(&saved).unwrap();
        std::fs::create_dir(&other).unwrap();
        let saved = saved.canonicalize().unwrap();
        assert_eq!(saved_folder_match(&saved, &saved).await, Some(true));

        let moved = dir.path().join("project-moved");
        std::fs::rename(&saved, &moved).unwrap();
        symlink(&other, &saved).unwrap();
        let wanted = other.canonicalize().unwrap();
        assert_eq!(saved_folder_match(&saved, &wanted).await, Some(false));
    }
}
