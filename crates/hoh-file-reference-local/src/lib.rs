//! Local-filesystem implementation of `ctx.fileReferences`.
//!
//! Port of `@deepseek-ai/dsh-file-reference-local` (`packages/context/file-reference-local`).

pub mod search;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::future::{BoxFuture, join_all};
use hoh_agent::{Agent, AgentCreated, AgentDisposed, Agents};
use hoh_error::BoxError;
use hoh_file_reference::{
    FILE_REFERENCE_PROMPT, FileReferenceCandidate, FileReferenceService, FileReferences,
};
use hoh_kernel::{Cfg, Context, Fiber, Inject, Plugin, Service, Undo};
use hoh_session::{SessionEventEmitted, SessionId};
use hoh_system_prompt::{
    PromptSection, PromptSectionOrderName, PromptText, SystemPrompt, SystemPromptKey,
};
use hoh_timeout::AbortSignal;
use hoh_tools::Tools;
use hoh_working_directory::WorkingDirectoryKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use search::{
    DEFAULT_FILE_SEARCH_EXCLUDED_DIRECTORIES, DEFAULT_FILE_SEARCH_MAX_ENTRIES,
    DEFAULT_FILE_SEARCH_MAX_RESULTS, FileSearchConfig, WorkspaceFileSearch,
};

fn default_max_results() -> u64 {
    DEFAULT_FILE_SEARCH_MAX_RESULTS
}

fn default_max_entries() -> u64 {
    DEFAULT_FILE_SEARCH_MAX_ENTRIES
}

fn default_excluded_directories() -> Vec<String> {
    DEFAULT_FILE_SEARCH_EXCLUDED_DIRECTORIES
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

/// Local file-reference discovery configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Maximum ranked candidates returned for one query.
    #[serde(default = "default_max_results")]
    #[schemars(range(min = 1))]
    pub max_results: u64,
    /// Maximum indexed files and directories per agent workspace.
    #[serde(default = "default_max_entries")]
    #[schemars(range(min = 1))]
    pub max_entries: u64,
    /// Directory basenames never traversed or offered.
    #[serde(default = "default_excluded_directories")]
    pub excluded_directories: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_results: default_max_results(),
            max_entries: default_max_entries(),
            excluded_directories: default_excluded_directories(),
        }
    }
}

/// Services the plugin needs (`static inject = ['agents', 'workingDirectory']`).
#[derive(Inject)]
pub struct Deps {
    agents: Service<Agents>,
    working_directory: Service<WorkingDirectoryKey>,
}

/// Services of the per-agent guidance scope (`agent.ctx.inject(['systemPrompt', 'tools'], ...)`).
#[derive(Inject)]
struct PromptDeps {
    system_prompt: Service<SystemPromptKey>,
    tools: Service<Tools>,
}

struct CachedSearch {
    cwd: String,
    search: WorkspaceFileSearch,
}

#[derive(Default)]
struct State {
    searches: HashMap<SessionId, CachedSearch>,
    prompt_fibers: HashMap<SessionId, Fiber>,
    /// Prompt scopes whose disposal was requested and may still be running.
    prompt_disposals: Vec<Fiber>,
}

struct Inner {
    ctx: Context,
    agents: Service<Agents>,
    working_directory: Service<WorkingDirectoryKey>,
    config: FileSearchConfig,
    state: Mutex<State>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Local-filesystem owner of the file-reference discovery service.
#[derive(Clone)]
pub struct LocalFileReferenceService {
    inner: Arc<Inner>,
}

impl Plugin for LocalFileReferenceService {
    const NAME: &'static str = "LocalFileReferenceService";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        let config = FileSearchConfig {
            max_results: config.max_results,
            max_entries: config.max_entries,
            excluded_directories: config.excluded_directories.clone(),
        };
        validate_config(&config)?;
        let service = Self {
            inner: Arc::new(Inner {
                ctx: ctx.clone(),
                agents: deps.agents,
                working_directory: deps.working_directory,
                config,
                state: Mutex::new(State::default()),
            }),
        };
        service.install_listeners()?;
        let provided: Arc<dyn FileReferenceService> = Arc::new(service);
        ctx.provide::<FileReferences>(provided).await?;
        Ok(())
    }
}

impl LocalFileReferenceService {
    fn install_listeners(&self) -> Result<(), BoxError> {
        let ctx = &self.inner.ctx;
        for agent in self.inner.agents.list() {
            self.install_prompt(&agent)?;
        }
        let created = self.clone();
        ctx.on_serial::<AgentCreated, (), _>(move |event| {
            let service = created.clone();
            async move {
                let fiber = service.install_prompt(&event.agent)?;
                fiber.settled().await?;
                Ok(None)
            }
        })?;
        let disposed = self.clone();
        ctx.on_contained::<AgentDisposed>(move |event| {
            let id = event.agent.id();
            let search = lock(&disposed.inner.state).searches.remove(id);
            if let Some(cached) = search {
                cached.search.dispose();
            }
            disposed.dispose_prompt(id);
            Ok(())
        })?;
        let invalidated = self.clone();
        ctx.on_contained::<SessionEventEmitted>(move |event| {
            if event.event.event_type() != "tool/result" {
                return Ok(());
            }
            if let Some(agent) = invalidated.inner.agents.get(event.session.id()) {
                let search = lock(&invalidated.inner.state)
                    .searches
                    .get(agent.id())
                    .map(|cached| cached.search.clone());
                if let Some(search) = search {
                    search.invalidate();
                }
            }
            Ok(())
        })?;
        let teardown = self.clone();
        ctx.effect("file-reference-local: search cache", move || {
            Undo::future(move || async move {
                let (prompt_fibers, disposals) = {
                    let mut state = lock(&teardown.inner.state);
                    for cached in state.searches.values() {
                        cached.search.dispose();
                    }
                    state.searches.clear();
                    let prompt_fibers: Vec<Fiber> = state
                        .prompt_fibers
                        .drain()
                        .map(|(_, fiber)| fiber)
                        .collect();
                    (prompt_fibers, std::mem::take(&mut state.prompt_disposals))
                };
                join_all(
                    prompt_fibers
                        .iter()
                        .chain(disposals.iter())
                        .map(Fiber::dispose),
                )
                .await;
                Ok(())
            })
        })?;
        Ok(())
    }

    /// Register the read-tool guidance inside the agent's own scope, once per agent.
    fn install_prompt(&self, agent: &Arc<dyn Agent>) -> Result<Fiber, BoxError> {
        let mut state = lock(&self.inner.state);
        if let Some(existing) = state.prompt_fibers.get(agent.id()) {
            return Ok(existing.clone());
        }
        let scope = agent.scope_key();
        let fiber = agent
            .ctx()
            .inject::<PromptDeps, _>(move |scope_ctx, deps| {
                let scope = scope.clone();
                async move {
                    let tools = deps.tools.clone();
                    deps.system_prompt.section(
                        &scope_ctx,
                        PromptSection {
                            name: "context:file-reference".into(),
                            order: SystemPrompt::get_section_order(
                                PromptSectionOrderName::FileReference,
                            ),
                            text: PromptText::provider(move |_| {
                                Ok(if tools.get("read", Some(&scope), None).is_none() {
                                    String::new()
                                } else {
                                    FILE_REFERENCE_PROMPT.to_owned()
                                })
                            }),
                            ..PromptSection::default()
                        },
                    )?;
                    Ok(())
                }
            })?;
        state
            .prompt_fibers
            .insert(agent.id().clone(), fiber.clone());
        Ok(fiber)
    }

    /// Dispose the agent's guidance scope without waiting for it; the teardown of the service
    /// waits for the disposals that are still running.
    fn dispose_prompt(&self, id: &SessionId) {
        let mut state = lock(&self.inner.state);
        let Some(fiber) = state.prompt_fibers.remove(id) else {
            return;
        };
        state
            .prompt_disposals
            .retain(|pending| !pending.is_disposed());
        // The command is sent now; the returned future only awaits its outcome.
        drop(fiber.dispose());
        state.prompt_disposals.push(fiber);
    }
}

impl FileReferenceService for LocalFileReferenceService {
    fn list<'a>(
        &'a self,
        agent: &'a Arc<dyn Agent>,
        query: &'a str,
        signal: &'a AbortSignal,
    ) -> BoxFuture<'a, Result<Vec<FileReferenceCandidate>, BoxError>> {
        Box::pin(async move {
            let cwd = self
                .inner
                .working_directory
                .ensure(agent, Some(signal))
                .await?;
            let search = {
                let mut state = lock(&self.inner.state);
                match state.searches.get(agent.id()) {
                    Some(cached) if cached.cwd == cwd => cached.search.clone(),
                    previous => {
                        if let Some(previous) = previous {
                            previous.search.dispose();
                        }
                        let search =
                            WorkspaceFileSearch::new(cwd.clone(), self.inner.config.clone())?;
                        state.searches.insert(
                            agent.id().clone(),
                            CachedSearch {
                                cwd,
                                search: search.clone(),
                            },
                        );
                        search
                    }
                }
            };
            search.list(query, signal).await
        })
    }
}

fn validate_config(config: &FileSearchConfig) -> Result<(), BoxError> {
    if config.max_results == 0 || config.max_results > 9_007_199_254_740_991 {
        return Err("file-reference-local: maxResults must be a positive safe integer".into());
    }
    if config.max_entries == 0 || config.max_entries > 9_007_199_254_740_991 {
        return Err("file-reference-local: maxEntries must be a positive safe integer".into());
    }
    if config
        .excluded_directories
        .iter()
        .any(|name| name.is_empty() || name.contains('/') || name.contains('\\'))
    {
        return Err(
            "file-reference-local: excludedDirectories entries must be non-empty directory basenames"
                .into(),
        );
    }
    Ok(())
}
