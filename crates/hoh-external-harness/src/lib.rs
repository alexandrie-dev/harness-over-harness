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
mod executor;
mod harness;
pub mod redact;
mod stdio;
pub mod supervisor;
mod window;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use hoh_agent::{Agents, SessionExecutor};
use hoh_destructive_classifier::DestructiveClassifier;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, Plugin, Service, Undo};
use hoh_llm::Llm;
use hoh_session::Sessions;
use hoh_user_approval::Approval;
use hoh_user_questions::UserQuestions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use contract::*;
pub use executor::BINDING_RECORD;
pub use harness::{ExternalHarness, HarnessResult, HarnessSession, Started};
pub use stdio::{StdioHarness, StdioHarnessConfig};
pub use supervisor::{ExitInfo, Frame, ProcessSpec, StderrSink, Supervisor, SupervisorLimits};
pub use window::{CHOICES_FILE, ExternalHarnessWindowPlugin};

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
}

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
            launched: Mutex::new(None),
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
        ctx.provide::<ExternalHarnesses>(Arc::new(service)).await?;
        Ok(())
    }
}
