//! The slot for an external agent harness (HOH-18): a CLI such as Claude Code or Codex that runs
//! its own agent loop, tools and continuation, and drives a HoH session instead of the agent loop.
//! No dsh counterpart; see `PORT_NOTES.md`.
//!
//! This crate knows no concrete harness. It holds:
//! - the contract ([`contract`]): discover, start or resume, prompt, interrupt, respond, and the
//!   stream of [`HarnessEvent`]s — plain JSON, versioned, the same messages in process
//!   ([`ExternalHarness`]) and over JSON-RPC ([`StdioHarness`]);
//! - the [`ExternalHarnesses`] service a harness package registers itself with, under the name
//!   that is also the session's provider route;
//! - the session executor behind that name: events go to the session log in the agent loop's
//!   shapes, permission requests through the destructive classifier and the approval service,
//!   questions through the user-questions service, the native id to the log for a later resume;
//! - process supervision shared by every harness that runs a program ([`Supervisor`]).

mod agent;
pub mod contract;
mod entries;
mod executor;
mod harness;
pub mod redact;
mod stdio;
pub mod supervisor;
mod window;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use hoh_agent::{Agents, SessionExecutor};
use hoh_destructive_classifier::DestructiveClassifier;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, Plugin, Service, Undo};
use hoh_llm::Llm;
use hoh_session::Sessions;
use hoh_user_approval::Approval;
use hoh_user_questions::UserQuestions;
use hoh_working_directory::WorkingDirectoryAccess;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use contract::*;
pub use executor::BINDING_RECORD;
pub use harness::{ExternalHarness, HarnessResult, HarnessSession, Started};
pub use stdio::{StdioHarness, StdioHarnessConfig};
pub use supervisor::{ExitInfo, Frame, ProcessSpec, StderrSink, Supervisor, SupervisorLimits};

/// Recheck the selected folder immediately before spawning a native executor.
pub fn validate_launch_directory(params: &StartParams) -> Result<(), HarnessFailure> {
    let Some(expected) = &params.cwd_identity else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&params.cwd).map_err(|_| {
            HarnessFailure::new(
                code::POLICY_UNAVAILABLE,
                "Рабочая папка разговора недоступна",
            )
        })?;
        if metadata.is_dir()
            && metadata.dev().to_string() == expected.device
            && metadata.ino().to_string() == expected.inode
        {
            return Ok(());
        }
    }
    Err(HarnessFailure::new(
        code::POLICY_UNAVAILABLE,
        "Рабочая папка разговора была заменена",
    ))
}
pub use window::{
    CHOICES_FILE, CODEX_SELECTION_FILE, ExternalHarnessWindowPlugin, SESSION_ACCESS_FILE,
};

use executor::{ExternalExecutor, Shared};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Configuration of the `ExternalHarness` plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// What a native session may do on its own. The default is the safe one. The sandbox and the
    /// network are capped by the access mode the user chose in the window: under `confirm` a
    /// native session never gets more than the folder and no network, whatever this says.
    pub policy: HarnessPolicy,
    /// How long the native interrupt may take before the process is terminated.
    pub interrupt_grace_ms: u64,
    /// How long SIGTERM may take before the process group is killed.
    pub terminate_grace_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            policy: HarnessPolicy::default(),
            interrupt_grace_ms: 5000,
            terminate_grace_ms: 3000,
        }
    }
}

/// The registry of external harnesses (`externalHarnesses`).
pub struct ExternalHarnessesService {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: Context,
    deps: Deps,
    config: Arc<Config>,
    choices: Arc<Choices>,
    harnesses: Mutex<Vec<Arc<Shared>>>,
}

/// What the user chose in the window for each executor. Changed only by the window's privileged
/// commands ([`window`]); read at every start of a native session.
#[derive(Default)]
pub(crate) struct Choices {
    access: Mutex<HashMap<String, AccessMode>>,
    /// Exact access selected for a conversation. Unlike the executor-wide legacy default this
    /// survives independently for each T3 thread.
    session_access: Mutex<HashMap<String, SessionAccessChoice>>,
    /// For each conversation an external agent serves (by HoH session id), the levels its
    /// prompts named, until their turns take them (HOH-40).
    prompts: Mutex<HashMap<String, ServedPrompts>>,
    /// Interaction modes are paired to the same message ids as prompt effort/access.
    interactions: Mutex<HashMap<String, PromptInteractions>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionAccessChoice {
    pub executor: String,
    pub mode: AccessMode,
}

/// Message-specific effort and access captured when the prompt was accepted.
type PromptSettings = VecDeque<(
    String,
    Option<String>,
    Option<AccessMode>,
    HashMap<String, AccessMode>,
)>;

#[derive(Default)]
struct ServedPrompts {
    executor: String,
    pending: PromptSettings,
}
type PromptInteractions = VecDeque<(String, InteractionMode)>;

impl Choices {
    pub(crate) fn access(&self, executor: &str) -> AccessMode {
        lock(&self.access)
            .get(executor)
            .copied()
            .unwrap_or(DEFAULT_ACCESS)
    }

    pub(crate) fn set_access(&self, executor: &str, mode: AccessMode) {
        lock(&self.access).insert(executor.to_owned(), mode);
    }

    pub(crate) fn all_access(&self) -> HashMap<String, AccessMode> {
        lock(&self.access).clone()
    }

    pub(crate) fn access_for(&self, executor: &str, session: &str) -> AccessMode {
        lock(&self.session_access)
            .get(session)
            .filter(|choice| choice.executor == executor)
            .map_or_else(|| self.access(executor), |choice| choice.mode)
    }

    pub(crate) fn set_session_access(&self, session: &str, executor: &str, mode: AccessMode) {
        lock(&self.session_access).insert(
            session.to_owned(),
            SessionAccessChoice {
                executor: executor.to_owned(),
                mode,
            },
        );
    }

    pub(crate) fn all_session_access(&self) -> HashMap<String, SessionAccessChoice> {
        lock(&self.session_access).clone()
    }

    /// An external agent serves `session`: the levels its prompts name are kept from now on.
    /// Conversations of the engine's own model keep nothing.
    pub(crate) fn serve_prompts(&self, session: &str, executor: &str) {
        lock(&self.interactions)
            .entry(session.to_owned())
            .or_default();
        let mut prompts = lock(&self.prompts);
        match prompts.entry(session.to_owned()) {
            std::collections::hash_map::Entry::Occupied(mut served) => {
                // `session/prompt` can be accepted before the external agent is constructed.
                // Keep those already snapshotted messages and bind later ones to the executor
                // the engine actually selected for the session.
                served.get_mut().executor = executor.to_owned();
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(ServedPrompts {
                    executor: executor.to_owned(),
                    pending: VecDeque::new(),
                });
            }
        }
    }

    /// The level and privileged per-session access in force when this prompt was accepted.
    pub(crate) fn note_prompt(&self, session: &str, message: &str, effort: Option<String>) {
        let mut prompts = lock(&self.prompts);
        if !prompts.contains_key(session) {
            // The prompt hook runs before agent construction. A session-specific choice already
            // names the executor and exact access; a legacy/global selection does not, but its
            // effort still has to survive until `serve_prompts` binds the eventual executor.
            let choice = lock(&self.session_access).get(session).cloned();
            prompts.insert(
                session.to_owned(),
                ServedPrompts {
                    executor: choice.map_or_else(String::new, |choice| choice.executor),
                    pending: VecDeque::new(),
                },
            );
        }
        let served = prompts.get_mut(session).expect("inserted above");
        let access =
            (!served.executor.is_empty()).then(|| self.access_for(&served.executor, session));
        let defaults = lock(&self.access).clone();
        served
            .pending
            .push_back((message.to_owned(), effort, access, defaults));
    }

    /// The prompt settings for `message`, with older abandoned prompts dropped.
    pub(crate) fn take_prompt(
        &self,
        session: &str,
        message: &str,
    ) -> Option<(Option<String>, Option<AccessMode>)> {
        let mut prompts = lock(&self.prompts);
        let served = prompts.get_mut(session)?;
        let executor = served.executor.clone();
        let pending = &mut served.pending;
        let at = pending.iter().position(|(id, _, _, _)| id == message)?;
        pending.remove(at).map(|(_, effort, access, defaults)| {
            (
                effort,
                access
                    .or_else(|| defaults.get(&executor).copied())
                    .or(Some(AccessMode::Confirm)),
            )
        })
    }

    pub(crate) fn note_interaction(&self, session: &str, message: &str, mode: InteractionMode) {
        lock(&self.interactions)
            .entry(session.to_owned())
            .or_default()
            .push_back((message.to_owned(), mode));
    }

    pub(crate) fn take_interaction(&self, session: &str, message: &str) -> Option<InteractionMode> {
        let mut interactions = lock(&self.interactions);
        let pending = interactions.get_mut(session)?;
        let at = pending.iter().position(|(id, _)| id == message)?;
        pending.remove(at).map(|(_, mode)| mode)
    }
}

#[cfg(test)]
mod access_snapshot_tests {
    use super::{AccessMode, Choices, InteractionMode};

    #[cfg(unix)]
    #[test]
    fn launch_rejects_a_missing_or_replaced_directory() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        std::fs::create_dir(&path).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let params = super::StartParams {
            session_id: "chat".into(),
            cwd: path.to_string_lossy().into_owned(),
            cwd_identity: Some(hoh_working_directory::WorkingDirectoryRootIdentity {
                device: metadata.dev().to_string(),
                inode: metadata.ino().to_string(),
            }),
            model: None,
            effort: None,
            interaction_mode: InteractionMode::Default,
            policy: Default::default(),
            resume: None,
        };
        assert!(super::validate_launch_directory(&params).is_ok());
        std::fs::rename(&path, temp.path().join("original")).unwrap();
        assert!(super::validate_launch_directory(&params).is_err());
        std::fs::create_dir(&path).unwrap();
        assert!(super::validate_launch_directory(&params).is_err());
    }

    #[test]
    fn out_of_order_preparation_keeps_each_messages_original_access() {
        let choices = Choices::default();
        choices.set_session_access("chat", "codex", AccessMode::Confirm);
        choices.note_prompt("chat", "a", None);
        choices.set_session_access("chat", "codex", AccessMode::Full);
        choices.note_prompt("chat", "b", None);
        choices.serve_prompts("chat", "codex");
        assert_eq!(
            choices.take_prompt("chat", "b"),
            Some((None, Some(AccessMode::Full)))
        );
        assert_eq!(
            choices.take_prompt("chat", "a"),
            Some((None, Some(AccessMode::Confirm)))
        );
    }

    #[test]
    fn interaction_modes_remain_bound_when_preparation_finishes_out_of_order() {
        let choices = Choices::default();
        choices.note_interaction("chat", "a", InteractionMode::Plan);
        choices.note_interaction("chat", "b", InteractionMode::Default);
        assert_eq!(
            choices.take_interaction("chat", "b"),
            Some(InteractionMode::Default)
        );
        assert_eq!(
            choices.take_interaction("chat", "a"),
            Some(InteractionMode::Plan)
        );
    }

    #[test]
    fn parallel_sessions_keep_the_mode_bound_to_each_accepted_message() {
        let choices = Choices::default();
        choices.set_session_access("chat-a", "codex", AccessMode::AutoAcceptEdits);
        choices.set_session_access("chat-b", "codex", AccessMode::Auto);
        // The window prompt hook wins the race against agent construction.
        choices.note_prompt("chat-a", "message-a1", Some("high".into()));
        choices.note_prompt("chat-b", "message-b1", Some("low".into()));
        choices.set_session_access("chat-a", "codex", AccessMode::Full);
        choices.serve_prompts("chat-a", "codex");
        choices.serve_prompts("chat-b", "codex");

        assert_eq!(
            choices.take_prompt("chat-b", "message-b1"),
            Some((Some("low".into()), Some(AccessMode::Auto)))
        );
        assert_eq!(
            choices.take_prompt("chat-a", "message-a1"),
            Some((Some("high".into()), Some(AccessMode::AutoAcceptEdits)))
        );

        choices.note_prompt("chat-a", "message-a2", None);
        assert_eq!(
            choices.take_prompt("chat-a", "message-a2"),
            Some((None, Some(AccessMode::Full)))
        );
    }

    #[test]
    fn a_prompt_before_legacy_agent_construction_keeps_its_effort() {
        let choices = Choices::default();
        choices.note_prompt("chat-a", "message-a1", Some("high".into()));
        choices.serve_prompts("chat-a", "claude");

        assert_eq!(
            choices.take_prompt("chat-a", "message-a1"),
            Some((Some("high".into()), Some(AccessMode::Confirm)))
        );
    }

    #[test]
    fn unbound_prompt_captures_the_provider_default_before_it_changes() {
        let choices = Choices::default();
        choices.set_access("claude", AccessMode::Full);
        choices.note_prompt("chat", "a", None);
        choices.set_access("claude", AccessMode::Confirm);
        choices.serve_prompts("chat", "claude");
        assert_eq!(
            choices.take_prompt("chat", "a"),
            Some((None, Some(AccessMode::Full)))
        );
    }
}

hoh_kernel::service!(pub ExternalHarnesses: ExternalHarnessesService = "externalHarnesses");

impl ExternalHarnessesService {
    /// Register `harness` under `name`, as an effect of `ctx` (the harness package's context).
    /// The name is also the provider route that selects the harness for a new session. Fails
    /// when the name is taken. Unregistering does not stop sessions it already drives.
    pub fn register(
        &self,
        ctx: &Context,
        name: &str,
        harness: Arc<dyn ExternalHarness>,
    ) -> Result<Disposer, BoxError> {
        if name.is_empty() {
            return Err("an external harness needs a name".into());
        }
        // The name is the provider route of a new session: one a model provider already has
        // would take that provider's sessions away from the agent loop.
        if let Some(llm) = self.inner.ctx.get::<Llm>()
            && llm
                .list_providers()
                .iter()
                .any(|provider| provider.id == name)
        {
            return Err(format!(
                "an external harness cannot be named \"{name}\": a model provider has that name"
            )
            .into());
        }
        let inner = self.inner.clone();
        let name = name.to_owned();
        let shared = Arc::new(Shared {
            ctx: inner.ctx.clone(),
            name: name.clone(),
            harness,
            agents: inner.deps.agents.clone(),
            sessions: inner.deps.sessions.clone(),
            approval: inner.deps.approval.clone(),
            questions: inner.deps.questions.clone(),
            classifier: inner.deps.classifier.clone(),
            config: inner.config.clone(),
            choices: inner.choices.clone(),
            launched: Mutex::new(Vec::new()),
            turn_access: Mutex::new(HashMap::new()),
        });
        let executor: Arc<dyn SessionExecutor> = Arc::new(ExternalExecutor(shared.clone()));
        ctx.try_effect(
            format!("externalHarnesses.register({name})"),
            |undos| -> Result<(), BoxError> {
                {
                    let mut harnesses = lock(&inner.harnesses);
                    if harnesses.iter().any(|taken| taken.name == name) {
                        return Err(format!(
                            "an external harness named \"{name}\" is already registered"
                        )
                        .into());
                    }
                    harnesses.push(shared.clone());
                }
                let registry = inner.clone();
                undos.push(Undo::sync(move || {
                    lock(&registry.harnesses).retain(|taken| !Arc::ptr_eq(taken, &shared));
                }))?;
                undos.push(inner.deps.agents.set_executor(ctx, &name, executor)?)?;
                Ok(())
            },
        )
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ExternalHarness>> {
        self.shared(name).map(|shared| shared.harness.clone())
    }

    pub(crate) fn shared(&self, name: &str) -> Option<Arc<Shared>> {
        lock(&self.inner.harnesses)
            .iter()
            .find(|shared| shared.name == name)
            .cloned()
    }

    pub(crate) fn choices(&self) -> &Arc<Choices> {
        &self.inner.choices
    }

    /// The access mode the user chose in the window for `name` ([`DEFAULT_ACCESS`] until then).
    /// Read-only: only the window's privileged command changes it.
    pub fn access(&self, name: &str) -> AccessMode {
        self.inner.choices.access(name)
    }

    /// Registered names, in registration order.
    pub fn names(&self) -> Vec<String> {
        lock(&self.inner.harnesses)
            .iter()
            .map(|shared| shared.name.clone())
            .collect()
    }

    /// Remote: externalHarness.discover
    pub async fn discover(&self, name: &str) -> Result<Discovery, BoxError> {
        let harness = self
            .get(name)
            .ok_or_else(|| format!("no external harness named \"{name}\""))?;
        Ok(harness.discover().await?)
    }
}

/// Dependencies of the plugin: the executor needs every one of them, so without the classifier
/// or the approval service no external session can start (fail closed).
#[derive(Inject)]
pub struct Deps {
    agents: Service<Agents>,
    sessions: Service<Sessions>,
    approval: Service<Approval>,
    questions: Service<UserQuestions>,
    classifier: Service<DestructiveClassifier>,
}

/// The code of the refusal to change the folder of a conversation of an external executor; the
/// wire carries only the message, so the message starts with it.
pub const EXECUTOR_FOLDER_FIXED: &str = "EXECUTOR_FOLDER_FIXED";

fn folder_fixed(executor: &str) -> BoxError {
    let name = agent::display_name(executor);
    Box::new(hoh_error::HarnessError::new(
        format!(
            "{EXECUTOR_FOLDER_FIXED}: Чат с {name} работает в одной папке. Чтобы работать в \
             другой, начните новый чат"
        ),
        EXECUTOR_FOLDER_FIXED,
    ))
}

pub struct ExternalHarnessPlugin;

impl Plugin for ExternalHarnessPlugin {
    const NAME: &'static str = "ExternalHarness";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        // The user's choices from the window, kept next to the profile (none: the defaults).
        let choices = Arc::new(Choices::default());
        window::restore_choices(&ctx, &choices, &deps.agents).await;
        let service = ExternalHarnessesService {
            inner: Arc::new(Inner {
                ctx: ctx.clone(),
                deps,
                config: config.get(),
                choices,
                harnesses: Mutex::new(Vec::new()),
            }),
        };
        let service = Arc::new(service);
        // A conversation of an external executor works in the folder it started in: the process
        // runs there, and Claude keeps its conversations by folder. Once it has worked there, the
        // window's folder change is refused with a word the window can tell from other failures;
        // a new chat (no turn yet) still takes another folder.
        let known = service.clone();
        ctx.listen()
            .prepend()
            .on_waterfall::<WorkingDirectoryAccess, String, _>(move |event, next| {
                let known = known.clone();
                async move {
                    // The event's agent is the folder service's adapter without a model: the
                    // conversation's own agent says whose it is.
                    if event.path.is_some()
                        && let Some(live) = known.inner.deps.agents.get(event.agent.id())
                        && let Some(chosen) = live.model().current()
                        && let Some(shared) = known.shared(&chosen.provider)
                        && shared.started_in_folder(live.session())
                    {
                        return Err(folder_fixed(&chosen.provider));
                    }
                    next.run().await
                }
            })?;
        ctx.provide::<ExternalHarnesses>(service).await?;
        Ok(())
    }
}
