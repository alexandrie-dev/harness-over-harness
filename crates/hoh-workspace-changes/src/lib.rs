//! Per-turn workspace changes from private Git snapshots and file-tool captures.

mod capture;
mod compare;
mod git;
mod numstat;
mod paths;
mod recorder;
mod types;

#[cfg(test)]
mod tests;

pub use compare::{Comparison, compare_text};
pub use numstat::{NumstatEntry, parse_numstat};
pub use paths::{
    canonical_path, compare_display, display_path_of, durable_path_of, is_inside,
    is_temporary_path, temporary_roots, to_posix,
};
pub use types::*;

use crate::{
    git::{GitLimits, GitRunner},
    recorder::{GitResolver, RecorderEnvironment, TurnRecorder, create_recorder},
};
use futures_util::future::BoxFuture;
use hoh_agent::AgentTurnStopping;
use hoh_error::BoxError;
use hoh_fs::{
    Fs, FsActor, FsDeleteIntent, FsEditIntent, FsTarget, FsVersionGuard, FsWriteIntent,
    FsWriteIntentEvent,
};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_session::{Session, SessionDisposed, SessionEventEmitted, SessionId};
use hoh_subprocess::{
    Subprocess, SubprocessCollect, SubprocessOutputMode, SubprocessSpawnSpec, SubprocessStdinMode,
    SubprocessStdio,
};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tools::{PreToolDecision, ToolExecution, ToolExecutionToken, ToolsPreExecute, ToolsResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, Weak},
};
use tokio::sync::OnceCell;

pub const NAME: &str = "workspace-changes";
pub const INJECT: [&str; 2] = ["subprocess", "fs"];
const JS_MAX_SAFE_INTEGER: u128 = 9_007_199_254_740_991;

fn default_timeout_ms() -> u64 {
    30_000
}
fn default_output_max_bytes() -> u64 {
    8 * 1_024 * 1_024
}
fn default_max_files() -> usize {
    500
}
fn default_max_file_bytes() -> usize {
    2 * 1_024 * 1_024
}
fn default_diff_timeout_ms() -> u64 {
    100
}

/// Snapshot, capture, and comparison bounds.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub timeout_ms: u64,
    pub output_max_bytes: u64,
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub diff_timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout_ms(),
            output_max_bytes: default_output_max_bytes(),
            max_files: default_max_files(),
            max_file_bytes: default_max_file_bytes(),
            diff_timeout_ms: default_diff_timeout_ms(),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    subprocess: Service<Subprocess>,
    fs: Service<Fs>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct State {
    config: Config,
    subprocess: Service<Subprocess>,
    fs: Service<Fs>,
    logger: hoh_kernel::Logger,
    recorders: Mutex<HashMap<SessionId, Arc<TurnRecorder>>>,
    mutations: Arc<Mutex<HashMap<ToolExecutionToken, MutationEntry>>>,
    git: OnceCell<Option<GitRunner>>,
    lifetime: AbortController,
}

struct MutationEntry {
    execution: Weak<ToolExecution>,
    recorder: Weak<TurnRecorder>,
}

impl State {
    async fn resolve_git(self: &Arc<Self>) -> Option<GitRunner> {
        self.git
            .get_or_init(|| async {
                let signal = self.lifetime.signal();
                let executable = match self
                    .subprocess
                    .resolve_executable("git", None, Some(signal.clone()))
                    .await
                {
                    Ok(executable) => executable,
                    Err(_) => {
                        self.logger.info(
                            "workspace-changes: git is unavailable; only file-tool edits are summarized",
                        );
                        return None;
                    }
                };
                if cfg!(target_os = "macos")
                    && executable == "/usr/bin/git"
                    && !self.xcode_selected(signal).await
                {
                    self.logger.info(
                        "workspace-changes: git is unavailable; only file-tool edits are summarized",
                    );
                    return None;
                }
                Some(GitRunner::new(
                    self.subprocess.clone(),
                    executable,
                    GitLimits {
                        timeout_ms: self.config.timeout_ms,
                        output_max_bytes: self.config.output_max_bytes,
                    },
                ))
            })
            .await
            .clone()
    }

    async fn xcode_selected(&self, signal: AbortSignal) -> bool {
        let handle = match self.subprocess.spawn(SubprocessSpawnSpec {
            argv: vec!["/usr/bin/xcode-select".into(), "-p".into()],
            cwd: std::env::var("HOME").unwrap_or_else(|_| "/".into()),
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Ignore,
                stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 4_096,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 4_096,
                    spill: None,
                }),
                control: None,
            },
            grace_ms: 1_000.0,
            signal: Some(signal),
            env: None,
        }) {
            Ok(handle) => handle,
            Err(_) => return false,
        };
        handle
            .done
            .await
            .is_ok_and(|outcome| outcome.exit_code == Some(0))
    }

    fn eligible(session: &Session) -> Option<String> {
        let header = session.header();
        let origin = header.get("origin").and_then(serde_json::Value::as_str);
        let depth = header
            .get("delegationDepth")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if origin == Some("subagent") || depth > 0 {
            None
        } else {
            header
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        }
    }

    fn recorder_for(self: &Arc<Self>, session: &Session, cwd: String) -> Arc<TurnRecorder> {
        if let Some(recorder) = lock(&self.recorders).get(session.id()).cloned() {
            return recorder;
        }
        let weak = Arc::downgrade(self);
        let git: GitResolver = Arc::new(move || {
            let weak = weak.clone();
            Box::pin(async move {
                Ok(match weak.upgrade() {
                    Some(state) => state.resolve_git().await,
                    None => None,
                })
            })
        });
        let logger = self.logger.clone();
        let recorder = create_recorder(
            session.clone(),
            cwd,
            RecorderEnvironment {
                git,
                temp_root: std::env::temp_dir(),
                max_files: self.config.max_files,
                max_file_bytes: self.config.max_file_bytes,
                diff_timeout_ms: self.config.diff_timeout_ms,
                warn: Arc::new(move |message| logger.warn(message)),
            },
        );
        lock(&self.recorders).insert(session.id().clone(), recorder.clone());
        recorder
    }

    fn recorder(&self, session_id: &SessionId) -> Option<Arc<TurnRecorder>> {
        lock(&self.recorders).get(session_id).cloned()
    }

    fn forget(&self, session_id: &SessionId) -> Option<Arc<TurnRecorder>> {
        lock(&self.recorders).remove(session_id)
    }

    async fn capture(&self, target: &FsTarget, actor: &Option<FsActor>) {
        let Some(exec) = actor
            .as_ref()
            .and_then(|actor| actor.as_ref().downcast_ref::<ToolExecution>())
        else {
            return;
        };
        let recorder = {
            let mut mutations = lock(&self.mutations);
            mutations.retain(|_, entry| {
                entry.execution.strong_count() > 0 && entry.recorder.strong_count() > 0
            });
            mutations
                .get(&exec.token)
                .and_then(|entry| entry.recorder.upgrade())
        };
        let Some(recorder) = recorder else {
            return;
        };
        recorder.capture(PathBuf::from(self.fs.process_path(target)));
        recorder.settled().await;
    }

    async fn dispose_all(&self) {
        self.lifetime.abort();
        let recorders: Vec<_> = lock(&self.recorders)
            .drain()
            .map(|(_, value)| value)
            .collect();
        lock(&self.mutations).clear();
        for recorder in recorders {
            recorder.dispose().await;
        }
    }
}

impl WorkspaceChangesApi for State {
    fn summary(&self, session_id: &SessionId, seq: u64) -> Option<WorkspaceChangesSummary> {
        self.recorder(session_id)?.summary(seq)
    }

    fn diff<'a>(
        &'a self,
        session_id: &'a SessionId,
        seq: u64,
        index: usize,
        signal: AbortSignal,
    ) -> BoxFuture<'a, Result<Option<WorkspaceFileDiff>, BoxError>> {
        Box::pin(async move {
            match self.recorder(session_id) {
                Some(recorder) => recorder.diff(seq, index, signal).await,
                None => Ok(None),
            }
        })
    }
}

pub struct WorkspaceChangesPlugin;

impl Plugin for WorkspaceChangesPlugin {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get().as_ref().clone();
        for (field, value) in [
            ("timeoutMs", config.timeout_ms as u128),
            ("outputMaxBytes", config.output_max_bytes as u128),
            ("maxFiles", config.max_files as u128),
            ("maxFileBytes", config.max_file_bytes as u128),
            ("diffTimeoutMs", config.diff_timeout_ms as u128),
        ] {
            if value == 0 || value > JS_MAX_SAFE_INTEGER {
                return Err(
                    format!("workspace-changes requires a positive integer {field}").into(),
                );
            }
        }
        let state = Arc::new(State {
            config,
            subprocess: deps.subprocess,
            fs: deps.fs,
            logger: ctx.logger().named(NAME),
            recorders: Mutex::new(HashMap::new()),
            mutations: Arc::new(Mutex::new(HashMap::new())),
            git: OnceCell::new(),
            lifetime: AbortController::new(),
        });

        let events = state.clone();
        ctx.on_contained::<SessionEventEmitted>(move |emitted| {
            match emitted.event.event_type() {
                "turn/start" => {
                    if let Some(turn) = emitted
                        .event
                        .data()
                        .get("turn")
                        .and_then(serde_json::Value::as_u64)
                        && let Some(cwd) = State::eligible(&emitted.session)
                    {
                        events.recorder_for(&emitted.session, cwd).start(turn);
                    }
                }
                "tool/result" => {
                    if let Some(recorder) = events.recorder(emitted.session.id()) {
                        recorder.observe(&emitted.event);
                    }
                }
                "turn/end" => {
                    if let Some(turn) = emitted
                        .event
                        .data()
                        .get("turn")
                        .and_then(serde_json::Value::as_u64)
                        && let Some(recorder) = events.recorder(emitted.session.id())
                    {
                        recorder.end(turn);
                    }
                }
                _ => {}
            }
            Ok(())
        })?;

        let disposed = state.clone();
        ctx.on_contained::<SessionDisposed>(move |event| {
            if let Some(recorder) = disposed.forget(event.session.id()) {
                tokio::spawn(async move { recorder.dispose().await });
            }
            Ok(())
        })?;

        let stopping = state.clone();
        ctx.on_serial::<AgentTurnStopping, (), _>(move |event| {
            let recorder = stopping.recorder(event.agent.session().id());
            let turn = event.turn;
            async move {
                if let Some(recorder) = recorder {
                    recorder.stopping(turn).await;
                }
                Ok(None)
            }
        })?;

        let pre_execute = state.clone();
        ctx.on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
            let pre_execute = pre_execute.clone();
            let exec = event.exec.clone();
            async move {
                if let Some(agent) = &exec.agent
                    && let Some(recorder) = pre_execute.recorder(agent.session().id())
                {
                    {
                        let mut mutations = lock(&pre_execute.mutations);
                        mutations.retain(|_, entry| {
                            entry.execution.strong_count() > 0 && entry.recorder.strong_count() > 0
                        });
                        mutations.insert(
                            exec.token,
                            MutationEntry {
                                execution: Arc::downgrade(&exec),
                                recorder: Arc::downgrade(&recorder),
                            },
                        );
                    }
                    recorder.settled().await;
                }
                next.run().await
            }
        })?;

        let tool_results = state.clone();
        ctx.on_contained::<ToolsResult>(move |event| {
            lock(&tool_results.mutations).remove(&event.exec.token);
            Ok(())
        })?;

        let writes = state.clone();
        ctx.listen()
            .global()
            .prepend()
            .on_waterfall::<FsWriteIntentEvent, Option<FsWriteIntent>, _>(move |event, next| {
                let writes = writes.clone();
                async move {
                    let result = next.run().await?;
                    writes.capture(&event.target, &event.actor).await;
                    Ok(result)
                }
            })?;
        let edits = state.clone();
        ctx.listen()
            .global()
            .prepend()
            .on_waterfall::<FsEditIntent, Option<FsVersionGuard>, _>(move |event, next| {
                let edits = edits.clone();
                async move {
                    let result = next.run().await?;
                    edits.capture(&event.target, &event.actor).await;
                    Ok(result)
                }
            })?;
        let deletes = state.clone();
        ctx.listen()
            .global()
            .prepend()
            .on_waterfall::<FsDeleteIntent, Option<FsVersionGuard>, _>(move |event, next| {
                let deletes = deletes.clone();
                async move {
                    let result = next.run().await?;
                    deletes.capture(&event.target, &event.actor).await;
                    Ok(result)
                }
            })?;

        ctx.provide::<WorkspaceChanges>(state.clone()).await?;
        ctx.effect("workspace-changes teardown", move || {
            Undo::future(move || async move {
                state.dispose_all().await;
                Ok(())
            })
        })?;
        Ok(())
    }
}
