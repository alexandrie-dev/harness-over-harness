//! Isolated real-provider fixtures for patch reports and observation state (`tests/patch/harness.ts`):
//! the real `LocalFileSystem` over a temporary workspace, wrapped so a test can inject failures at
//! the provider seam, plus the observation-policy stand-in.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use hoh_error::BoxError;
use hoh_fs::{
    FileSystem, Fs, FsByteRange, FsDeleteOutcome, FsDirEntry, FsEditIntent, FsEditOutcome,
    FsEditRequest, FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome, FsObservation, FsObserved,
    FsPathInfo, FsResolveOptions, FsTarget, FsTextStream, FsVersion, FsVersionGuard, FsWriteIntent,
    FsWriteOutcome,
};
use hoh_fs_local::{Config as LocalConfig, LocalFileSystem};
use hoh_kernel::{Context, Kernel};
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_fs::patch::{
    PatchExecutionReport, PatchExecutionRequest, PatchOperationKind, PatchOperationRecord,
};
use hoh_tool_fs::sandbox::FsSandboxController;
use hoh_tool_fs::{Env, ToolFs};
use hoh_tools::{ToolExecution, ToolExecutionInput, ToolExecutionPipeline, ToolRuntime};
use hoh_working_directory::WorkingDirectory;
use serde_json::json;

pub use super::lock;
use super::{TestAgent, mount_policy};

pub fn patch(operations: &[&str]) -> String {
    let mut lines = vec!["*** Begin Patch"];
    lines.extend(operations.iter().copied());
    lines.push("*** End Patch");
    lines.join("\n")
}
pub fn add(path: &str) -> String {
    add_with(path, "new")
}
pub fn add_with(path: &str, content: &str) -> String {
    format!("*** Add File: {path}\n+{content}")
}
pub fn update(path: &str) -> String {
    update_with(path, "old", "new")
}
pub fn update_with(path: &str, before: &str, after: &str) -> String {
    format!("*** Update File: {path}\n@@\n-{before}\n+{after}")
}
pub fn remove(path: &str) -> String {
    format!("*** Delete File: {path}")
}
pub fn mv(source: &str, destination: &str) -> String {
    move_with(source, destination, "old", "new")
}
pub fn move_with(source: &str, destination: &str, before: &str, after: &str) -> String {
    format!("*** Update File: {source}\n*** Move to: {destination}\n@@\n-{before}\n+{after}")
}

pub type MoveReplacement =
    Arc<dyn Fn(&FsTarget, &FsTarget) -> Option<Result<FsMoveOutcome, BoxError>> + Send + Sync>;

type Fault<A> = Arc<dyn Fn(A) -> Option<BoxError> + Send + Sync>;

/// Failure injection points over the real provider (the `vi.spyOn(t.fs, …)` of the source).
#[derive(Default)]
pub struct Hooks {
    /// Fail `resolve` of a raw path instead of calling the provider.
    pub resolve: Option<Fault<String>>,
    /// Report this `lstat` result.
    pub lstat: Option<FsPathInfo>,
    /// Fail `read_text` instead of reading.
    pub read_text: Option<Fault<()>>,
    /// Fail `patch_text` instead of patching.
    pub patch_text: Option<Fault<()>>,
    /// Run `patch_text`, then fail (a lost reply).
    pub patch_text_lost_reply: Option<Fault<()>>,
    /// Fail `stat` instead of statting.
    pub stat: Option<Fault<()>>,
    /// After `write_text` succeeds, run this (an external edit between operations).
    pub after_write_text: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Replace `move_file` (it may change disk before answering).
    pub move_file: Option<MoveReplacement>,
    /// Run `write_text`, then fail (a lost reply).
    pub write_text_lost_reply: Option<Fault<()>>,
    /// Run this before the real `move_file`.
    pub before_move_file: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// `LocalFileSystem` plus recorded calls and injected faults.
pub struct SpyFs {
    pub inner: Arc<LocalFileSystem>,
    pub hooks: Mutex<Hooks>,
    pub resolve_calls: AtomicUsize,
    pub read_text_calls: AtomicUsize,
    /// Display paths of every `stat`, in call order.
    pub stat_targets: Mutex<Vec<String>>,
    pub move_hunks: Mutex<Vec<usize>>,
    pub patch_text_guards: Mutex<Vec<Option<FsVersionGuard>>>,
}

impl SpyFs {
    /// Run the selected fault outside the hooks lock, so a fault may itself install other hooks.
    fn fault<A>(
        &self,
        pick: impl FnOnce(&Hooks) -> Option<Fault<A>>,
        argument: A,
    ) -> Option<BoxError> {
        let selected = pick(&lock(&self.hooks));
        selected.and_then(|fault| fault(argument))
    }
}

impl FileSystem for SpyFs {
    fn sandbox_mode(&self) -> Option<hoh_sandbox::SandboxMode> {
        self.inner.sandbox_mode()
    }
    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        Box::pin(async move {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.fault(|hooks| hooks.resolve.clone(), path.to_owned()) {
                return Err(error);
            }
            self.inner.resolve(path, options).await
        })
    }
    fn process_path(&self, target: &FsTarget) -> String {
        self.inner.process_path(target)
    }
    fn file_url(&self, target: &FsTarget) -> String {
        self.inner.file_url(target)
    }
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        self.inner.contains(parent, child)
    }
    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        Box::pin(async move {
            lock(&self.stat_targets).push(target.display_path.clone());
            if let Some(error) = self.fault(|hooks| hooks.stat.clone(), ()) {
                return Err(error);
            }
            self.inner.stat(target, signal).await
        })
    }
    fn lstat<'a>(
        &'a self,
        path: &'a str,
        cwd: Option<&'a str>,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        Box::pin(async move {
            if let Some(info) = lock(&self.hooks).lstat.clone() {
                return Ok(Some(info));
            }
            self.inner.lstat(path, cwd, signal).await
        })
    }
    fn read_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String> {
        Box::pin(async move {
            self.read_text_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.fault(|hooks| hooks.read_text.clone(), ()) {
                return Err(error);
            }
            self.inner.read_text(target, signal).await
        })
    }
    fn stream_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        self.inner.stream_text(target, signal)
    }
    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
        max: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        self.inner.read_bytes(target, signal, max)
    }
    fn read_byte_range<'a>(
        &'a self,
        target: &'a FsTarget,
        range: FsByteRange,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        self.inner.read_byte_range(target, range, signal)
    }
    fn list_dir<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        self.inner.list_dir(target, signal)
    }
    fn write_text<'a>(
        &'a self,
        target: &'a FsTarget,
        content: &'a str,
        expected: Option<FsWriteIntent>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsWriteOutcome> {
        Box::pin(async move {
            let outcome = self
                .inner
                .write_text(target, content, expected, signal, policy)
                .await?;
            if let Some(error) = self.fault(|hooks| hooks.write_text_lost_reply.clone(), ()) {
                return Err(error);
            }
            let after = lock(&self.hooks).after_write_text.clone();
            if let Some(after) = after {
                after();
            }
            Ok(outcome)
        })
    }
    fn edit_text<'a>(
        &'a self,
        target: &'a FsTarget,
        edit: FsEditRequest,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        self.inner.edit_text(target, edit, expected, signal, policy)
    }
    fn patch_text<'a>(
        &'a self,
        target: &'a FsTarget,
        hunks: &'a [hoh_fs_patch::PatchHunk],
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        Box::pin(async move {
            lock(&self.patch_text_guards).push(expected.clone());
            if let Some(error) = self.fault(|hooks| hooks.patch_text.clone(), ()) {
                return Err(error);
            }
            let outcome = self
                .inner
                .patch_text(target, hunks, expected, signal, policy)
                .await?;
            if let Some(error) = self.fault(|hooks| hooks.patch_text_lost_reply.clone(), ()) {
                return Err(error);
            }
            Ok(outcome)
        })
    }
    fn delete_file<'a>(
        &'a self,
        target: &'a FsTarget,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome> {
        self.inner.delete_file(target, expected, signal, policy)
    }
    fn move_file<'a>(
        &'a self,
        source: &'a FsTarget,
        destination: &'a FsTarget,
        options: FsMoveOptions,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome> {
        Box::pin(async move {
            lock(&self.move_hunks).push(options.hunks.as_ref().map_or(0, Vec::len));
            let replace = lock(&self.hooks).move_file.clone();
            if let Some(replace) = replace
                && let Some(answer) = replace(source, destination)
            {
                return answer;
            }
            let before = lock(&self.hooks).before_move_file.clone();
            if let Some(before) = before {
                before();
            }
            self.inner
                .move_file(source, destination, options, signal, policy)
                .await
        })
    }
}

/// What a test composes.
#[derive(Default)]
pub struct PatchOptions {
    /// Mount no observation policy: the tools use the bare provider's unconditional behavior.
    pub without_policy: bool,
    /// Leave `applyPatch` at its default (disabled).
    pub without_apply_patch: bool,
}

pub struct T {
    pub kernel: Kernel,
    pub ctx: Context,
    pub dir: tempfile::TempDir,
    pub workspace: PathBuf,
    pub spy: Arc<SpyFs>,
    pub local: Arc<LocalFileSystem>,
    pub controller: Arc<AbortController>,
    pub exec: Arc<ToolExecution>,
    pub env: Env,
    pub sandbox: FsSandboxController,
    pub observations: Arc<Mutex<Vec<(String, FsObservation)>>>,
    pub agent: Arc<dyn hoh_tools::Agent>,
    pub tool_fs: hoh_kernel::Fiber,
}

pub async fn setup(options: PatchOptions) -> T {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let workspace = root.join("workspace");
    let provider_root = root.join("provider");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&provider_root).unwrap();
    let workspace_text = workspace.to_string_lossy().into_owned();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let local = Arc::new(
        LocalFileSystem::new(LocalConfig {
            cwd: Some(provider_root.to_string_lossy().into_owned()),
            ..LocalConfig::default()
        })
        .unwrap(),
    );
    let spy = Arc::new(SpyFs {
        inner: local.clone(),
        hooks: Mutex::default(),
        resolve_calls: AtomicUsize::new(0),
        read_text_calls: AtomicUsize::new(0),
        stat_targets: Mutex::default(),
        move_hunks: Mutex::default(),
        patch_text_guards: Mutex::default(),
    });
    let provided = spy.clone();
    ctx.inject(move |ctx, (): ()| {
        let fs = provided.clone();
        async move {
            ctx.provide::<Fs>(fs).await?;
            Ok(())
        }
    })
    .unwrap()
    .settled()
    .await
    .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_agent::AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<NativeToolExecutorPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<WorkingDirectory>(json!({ "defaultDirectory": workspace_text }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    if !options.without_policy {
        mount_policy(&ctx);
    }
    let tool_fs = ctx
        .plugin::<ToolFs>(json!({ "applyPatch": !options.without_apply_patch }))
        .unwrap();
    tool_fs.settled().await.unwrap();
    let controller = Arc::new(AbortController::new());
    let mut input = ToolExecutionInput::new(
        "patch-test".into(),
        "apply_patch",
        json!({}),
        controller.signal(),
    );
    let agent: Arc<dyn hoh_tools::Agent> = TestAgent::new();
    super::ensure_live(&ctx, &agent);
    input.agent = Some(agent.clone());
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).unwrap();
    let exec = pipeline.prepare(input).await.exec().clone();
    let env = Env {
        ctx: ctx.clone(),
        fs: ctx.get::<Fs>().unwrap(),
        working_directory: ctx.get().unwrap(),
        tools: ctx.get().unwrap(),
        system_prompt: ctx.get().unwrap(),
    };
    let sandbox = FsSandboxController::new(&ctx, &*env.fs).unwrap();
    let observations: Arc<Mutex<Vec<(String, FsObservation)>>> = Arc::default();
    let seen = observations.clone();
    ctx.on::<FsObserved>(move |event| {
        lock(&seen).push((event.target.display_path.clone(), event.observation.clone()));
    })
    .unwrap();
    T {
        kernel,
        ctx,
        dir,
        workspace,
        spy,
        local,
        controller,
        exec,
        env,
        sandbox,
        observations,
        agent,
        tool_fs,
    }
}

impl T {
    /// Call the registered `apply_patch` tool as the executing agent.
    pub async fn call_patch(&self, text: &str) -> hoh_tools::ToolExecutionResult {
        super::call_with_signal(
            &self.ctx,
            "apply_patch",
            json!({ "input": text }),
            Some(self.agent.clone()),
            self.controller.signal(),
        )
        .await
    }

    /// Call a registered tool as the executing agent.
    pub async fn call(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> hoh_tools::ToolExecutionResult {
        super::call(&self.ctx, name, arguments, Some(self.agent.clone())).await
    }

    /// The `stat`s of files (not directories) so far; the working directory also stats its own.
    pub fn file_stats(&self) -> usize {
        let workspace = self.workspace.to_string_lossy().into_owned();
        lock(&self.spy.stat_targets)
            .iter()
            .filter(|path| **path != workspace)
            .count()
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.workspace.join(name)
    }

    /// `path.resolve(workspace, name)`: the joined path without `.` segments.
    pub fn path_text(&self, name: &str) -> String {
        self.path(name)
            .components()
            .collect::<PathBuf>()
            .to_string_lossy()
            .into_owned()
    }

    pub async fn target(&self, name: &str) -> FsTarget {
        self.local
            .resolve(
                name,
                FsResolveOptions {
                    cwd: Some(self.workspace.to_string_lossy().into_owned()),
                    signal: None,
                },
            )
            .await
            .unwrap()
    }

    /// Write the fixture and (unless `observed` is false) record a present observation for the
    /// executing agent; the recorded observation events are cleared afterwards.
    pub async fn file(&self, name: &str, text: &str, observed: bool) -> FsTarget {
        std::fs::write(self.path(name), text).unwrap();
        let resolved = self.target(name).await;
        if observed {
            let info = self
                .local
                .stat(&resolved, None)
                .await
                .unwrap()
                .expect("fixture file exists");
            self.ctx.emit(&FsObserved {
                target: resolved.clone(),
                observation: FsObservation::Present {
                    version: info.version,
                },
                actor: Some(self.exec.clone() as hoh_fs::FsActor),
            });
        }
        lock(&self.observations).clear();
        resolved
    }

    pub async fn absent(&self, name: &str) {
        self.ctx.emit(&FsObserved {
            target: self.target(name).await,
            observation: FsObservation::Absent,
            actor: Some(self.exec.clone() as hoh_fs::FsActor),
        });
        lock(&self.observations).clear();
    }

    /// The expected operation record for a path under the workspace.
    pub fn record(
        &self,
        kind: PatchOperationKind,
        name: &str,
        index: usize,
        patch_line: u64,
        destination: Option<&str>,
    ) -> PatchOperationRecord {
        PatchOperationRecord {
            kind,
            path: self.path_text(name),
            index,
            patch_line,
            destination: destination.map(|destination| self.path_text(destination)),
        }
    }

    pub async fn run(&self, text: &str) -> Result<PatchExecutionReport, BoxError> {
        self.run_with(text, None).await
    }

    pub async fn run_with(
        &self,
        text: &str,
        sandbox_policy: Option<SandboxExecutionPolicy>,
    ) -> Result<PatchExecutionReport, BoxError> {
        hoh_tool_fs::patch::execute_patch(
            &self.env,
            &self.sandbox,
            &self.exec,
            &PatchExecutionRequest {
                patch: text.to_owned(),
                sandbox_policy,
            },
        )
        .await
    }

    /// Run against an execution that has no agent (so no session directory is resolved, which the
    /// real working-directory service refuses for an aborted signal) but shares this signal.
    pub async fn run_without_agent(&self, text: &str) -> Result<PatchExecutionReport, BoxError> {
        let input = ToolExecutionInput::new(
            "patch-test".into(),
            "apply_patch",
            json!({}),
            self.controller.signal(),
        );
        let pipeline = ToolExecutionPipeline::new(self.ctx.clone()).unwrap();
        let exec = pipeline.prepare(input).await.exec().clone();
        hoh_tool_fs::patch::execute_patch(
            &self.env,
            &self.sandbox,
            &exec,
            &PatchExecutionRequest {
                patch: text.to_owned(),
                sandbox_policy: None,
            },
        )
        .await
    }

    pub fn bytes(&self, name: &str) -> std::io::Result<String> {
        std::fs::read_to_string(self.path(name))
    }

    pub async fn edit_intent(&self, name: &str) -> Result<Option<FsVersionGuard>, BoxError> {
        let target = self.target(name).await;
        self.ctx
            .waterfall(
                FsEditIntent {
                    target,
                    actor: Some(self.exec.clone() as hoh_fs::FsActor),
                },
                |_| async { Ok(None) },
            )
            .await
    }

    pub async fn version(&self, name: &str) -> FsVersion {
        let target = self.target(name).await;
        self.local
            .stat(&target, None)
            .await
            .unwrap()
            .map_or_else(|| FsVersion::from("missing-fixture"), |info| info.version)
    }

    /// The recorded observations as `(path, "present" | "absent")`.
    pub fn observation_kinds(&self) -> Vec<(String, &'static str)> {
        lock(&self.observations)
            .iter()
            .map(|(path, observation)| {
                (
                    path.clone(),
                    if matches!(observation, FsObservation::Present { .. }) {
                        "present"
                    } else {
                        "absent"
                    },
                )
            })
            .collect()
    }

    pub fn observed_paths(&self) -> Vec<String> {
        lock(&self.observations)
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn is_unobserved_empty(&self) -> bool {
        lock(&self.observations).is_empty()
    }
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub fn change(
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> hoh_tool_fs::patch::PatchFileChange {
    hoh_tool_fs::patch::PatchFileChange {
        path: path.to_owned(),
        before: before.map(str::to_owned),
        after: after.map(str::to_owned),
    }
}

pub fn failure(code: &str, message: &str) -> hoh_tool_fs::patch::PatchFailure {
    hoh_tool_fs::patch::PatchFailure::new(code, message)
}

pub fn refusal(
    operation: PatchOperationRecord,
    code: &str,
    message: &str,
) -> hoh_tool_fs::patch::PatchRefusal {
    hoh_tool_fs::patch::PatchRefusal {
        operation,
        failure: failure(code, message),
    }
}

pub fn preflight(refusals: Vec<hoh_tool_fs::patch::PatchRefusal>) -> PatchExecutionReport {
    PatchExecutionReport::Preflight { refusals }
}

pub fn commit(
    outcomes: Vec<hoh_tool_fs::patch::PatchOperationOutcome>,
    affected_paths: &[&str],
    cancelled: bool,
) -> PatchExecutionReport {
    PatchExecutionReport::Commit {
        outcomes,
        affected_paths: affected_paths
            .iter()
            .map(|path| (*path).to_owned())
            .collect(),
        cancelled,
    }
}

pub fn completed(
    operation: PatchOperationRecord,
    changes: Vec<hoh_tool_fs::patch::PatchFileChange>,
) -> hoh_tool_fs::patch::PatchOperationOutcome {
    hoh_tool_fs::patch::PatchOperationOutcome::Completed { operation, changes }
}

pub fn skipped(operation: PatchOperationRecord) -> hoh_tool_fs::patch::PatchOperationOutcome {
    hoh_tool_fs::patch::PatchOperationOutcome::Skipped { operation }
}
