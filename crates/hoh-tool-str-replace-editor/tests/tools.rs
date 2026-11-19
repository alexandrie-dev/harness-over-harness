use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_fs::{
    FileSystem, Fs, FsByteRange, FsChanged, FsDeleteOutcome, FsDirEntry, FsEditOutcome,
    FsEditRequest, FsError, FsErrorCode, FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome,
    FsPathInfo, FsResolveOptions, FsTarget, FsTextStream, FsType, FsVersion, FsVersionGuard,
    FsWatcher, FsWriteIntent, FsWriteOutcome,
};
use hoh_fs_local::{Config as LocalConfig, LocalFileSystem};
use hoh_kernel::{Context, Fiber, Kernel, Plugin};
use hoh_llm::{ContentBlock, ToolCallId, UserMessage};
use hoh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, SESSION_FORMAT_VERSION, Session, SessionId};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_str_replace_editor::ToolStrReplaceEditor;
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{
    ToolCallKind, ToolCallView, ToolExecutionInput, ToolExecutionResult, ToolRuntime, Tools,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct FsControls {
    first_stat_absent: AtomicBool,
    force_other: AtomicBool,
    fail_write: AtomicBool,
    reverse_root_listing: Mutex<Option<(String, FsTarget)>>,
}

struct TestFileSystem {
    local: LocalFileSystem,
    sandbox_mode: Option<SandboxMode>,
    controls: Arc<FsControls>,
}

impl TestFileSystem {
    fn new(root: &str, sandbox_mode: Option<SandboxMode>) -> Self {
        Self {
            local: LocalFileSystem::new(LocalConfig {
                cwd: Some(root.to_owned()),
                ..LocalConfig::default()
            })
            .unwrap(),
            sandbox_mode,
            controls: Arc::default(),
        }
    }
}

impl FileSystem for TestFileSystem {
    fn watch(
        &self,
        target: &FsTarget,
        changed: FsChanged,
        signal: AbortSignal,
    ) -> FsFuture<'_, Arc<dyn FsWatcher>> {
        self.local.watch(target, changed, signal)
    }

    fn sandbox_mode(&self) -> Option<SandboxMode> {
        self.sandbox_mode
    }

    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        self.local.resolve(path, options)
    }

    fn process_path(&self, target: &FsTarget) -> String {
        self.local.process_path(target)
    }

    fn process_path_from_host_path(&self, host_path: &str) -> Option<String> {
        self.local.process_path_from_host_path(host_path)
    }

    fn file_url(&self, target: &FsTarget) -> String {
        self.local.file_url(target)
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        self.local.contains(parent, child)
    }

    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        Box::pin(async move {
            if self
                .controls
                .first_stat_absent
                .swap(false, Ordering::SeqCst)
            {
                return Ok(None);
            }
            if self.controls.force_other.load(Ordering::SeqCst) {
                return Ok(Some(FsInfo {
                    version: FsVersion::from("special"),
                    r#type: FsType::Other,
                    size: None,
                }));
            }
            self.local.stat(target, signal).await
        })
    }

    fn lstat<'a>(
        &'a self,
        path: &'a str,
        cwd: Option<&'a str>,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        self.local.lstat(path, cwd, signal)
    }

    fn read_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String> {
        self.local.read_text(target, signal)
    }

    fn stream_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        self.local.stream_text(target, signal)
    }

    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
        max_bytes: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        self.local.read_bytes(target, signal, max_bytes)
    }

    fn read_byte_range<'a>(
        &'a self,
        target: &'a FsTarget,
        range: FsByteRange,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        self.local.read_byte_range(target, range, signal)
    }

    fn list_dir<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        Box::pin(async move {
            let mut entries = self.local.list_dir(target, signal).await?;
            let injected = lock(&self.controls.reverse_root_listing).clone();
            if let Some((root, other)) = injected
                && target.display_path == root
            {
                entries.reverse();
                entries.insert(
                    0,
                    FsDirEntry {
                        name: "other".into(),
                        r#type: FsType::Other,
                        target: other.clone(),
                        version: None,
                        size: None,
                    },
                );
                entries.insert(
                    0,
                    FsDirEntry {
                        name: "same-target".into(),
                        r#type: FsType::Other,
                        target: other,
                        version: None,
                        size: None,
                    },
                );
            }
            Ok(entries)
        })
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
            if self.controls.fail_write.load(Ordering::SeqCst) {
                return Err("backend write failed".into());
            }
            if policy
                .as_ref()
                .is_some_and(|policy| policy.mode == SandboxMode::ReadOnly)
            {
                return Err(FsError::new(
                    format!(
                        "cannot write \"{}\": file access denied under read-only mode",
                        target.display_path
                    ),
                    FsErrorCode::SandboxDenied,
                )
                .into());
            }
            self.local
                .write_text(target, content, expected, signal, None)
                .await
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
        self.local.edit_text(target, edit, expected, signal, policy)
    }

    fn patch_text<'a>(
        &'a self,
        target: &'a FsTarget,
        hunks: &'a [hoh_fs_patch::PatchHunk],
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        self.local
            .patch_text(target, hunks, expected, signal, policy)
    }

    fn delete_file<'a>(
        &'a self,
        target: &'a FsTarget,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome> {
        self.local.delete_file(target, expected, signal, policy)
    }

    fn move_file<'a>(
        &'a self,
        source: &'a FsTarget,
        destination: &'a FsTarget,
        options: FsMoveOptions,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome> {
        self.local
            .move_file(source, destination, options, signal, policy)
    }
}

struct TestAgent {
    session: Session,
    ctx: Context,
    key: ScopeKey,
    options: AgentOptions,
}

impl Agent for TestAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }

    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn model(&self) -> &dyn Model {
        panic!("model is not used by editor tests")
    }

    fn inbox(&self) -> &dyn Inbox {
        panic!("inbox is not used by editor tests")
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }

    fn when_idle(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        Box::pin(async {})
    }

    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(AbortController::new().signal()))
    }

    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }

    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
}

static CALL_NUMBER: AtomicU64 = AtomicU64::new(0);

struct Harness {
    _kernel: Kernel,
    ctx: Context,
    root: TempDir,
    fs: Arc<TestFileSystem>,
    editor: Fiber,
    owner: Arc<dyn Agent>,
}

async fn mount<P: Plugin>(ctx: &Context, config: Value) -> Fiber {
    let fiber = ctx.plugin::<P>(config).unwrap();
    fiber.settled().await.unwrap();
    fiber
}

async fn setup(config: Value, fs_policy: bool, sandbox_mode: Option<SandboxMode>) -> Harness {
    let root = TempDir::new().unwrap();
    let root_text = root.path().to_string_lossy().into_owned();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount::<hoh_session_projection::SessionProjectionRegistry>(&ctx, json!({})).await;
    mount::<hoh_system_prompt::SystemPrompt>(&ctx, json!({})).await;
    mount::<ToolRuntime>(&ctx, json!({})).await;
    mount::<NativeToolExecutorPlugin>(&ctx, json!({})).await;
    if let Some(mode) = sandbox_mode {
        mount::<hoh_sandbox_policy::SandboxPolicyService>(
            &ctx,
            json!({ "mode": mode, "workspaceRoot": root_text }),
        )
        .await;
    }
    let fs = Arc::new(TestFileSystem::new(&root_text, sandbox_mode));
    ctx.provide::<Fs>(fs.clone()).await.unwrap();
    if fs_policy {
        mount::<hoh_fs_observation_policy::FsObservationPolicy>(&ctx, json!({})).await;
    }
    let editor = mount::<ToolStrReplaceEditor>(&ctx, config).await;
    let id = SessionId::from(format!(
        "str-replace-editor-owner-{}",
        CALL_NUMBER.fetch_add(1, Ordering::SeqCst)
    ));
    let session = Session::create(
        id.clone(),
        None,
        Some(json!({
            "version": SESSION_FORMAT_VERSION,
            "id": id,
            "createdAt": 0,
            "cwd": root_text,
            "isSeeded": false,
        })),
        None,
        None,
    )
    .unwrap();
    let owner = Arc::new(TestAgent {
        session,
        ctx: ctx.clone(),
        key: ScopeKey::new(),
        options: AgentOptions::default(),
    });
    Harness {
        _kernel: kernel,
        ctx,
        root,
        fs,
        editor,
        owner,
    }
}

async fn call(harness: &Harness, owner: bool, arguments: Value) -> ToolExecutionResult {
    let number = CALL_NUMBER.fetch_add(1, Ordering::SeqCst);
    let mut input = ToolExecutionInput::new(
        ToolCallId::from(format!("str-replace-editor-{number}")),
        "str_replace_editor",
        arguments,
        AbortController::new().signal(),
    );
    if owner {
        input.agent = Some(harness.owner.clone());
    }
    execute_tool_fixture(&harness.ctx, input).await.unwrap()
}

fn text(result: &ToolExecutionResult) -> String {
    result
        .content()
        .iter()
        .filter_map(ContentBlock::text_value)
        .collect()
}

fn path(harness: &Harness, name: &str) -> String {
    harness
        .root
        .path()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn assert_error(result: &ToolExecutionResult, name: &str, code: &str) {
    let info = result
        .error()
        .and_then(|error| error.info.as_ref())
        .unwrap();
    assert_eq!(info.name, name);
    assert_eq!(info.code, code);
}

#[tokio::test]
async fn registers_the_standalone_schema_and_configurable_description() {
    let harness = setup(
        json!({ "description": "custom editor description" }),
        false,
        None,
    )
    .await;
    let tools = harness.ctx.get::<Tools>().unwrap();
    let schemas = tools.schemas(None, None).unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "str_replace_editor");
    assert_eq!(schemas[0].description, "custom editor description");
    let properties = &schemas[0].parameters["properties"];
    assert!(properties.get("replace_all").is_none());
    for (field, types) in [
        ("file_text", ["string", "null"]),
        ("insert_line", ["integer", "null"]),
        ("new_str", ["string", "null"]),
        ("old_str", ["string", "null"]),
        ("view_range", ["array", "null"]),
    ] {
        let actual: Vec<_> = properties[field]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["type"].as_str().unwrap())
            .collect();
        assert_eq!(actual, types, "{field}");
    }
    assert_eq!(
        properties["view_range"]["oneOf"][0]["items"]["type"],
        "integer"
    );

    let tool = tools.get("str_replace_editor", None, None).unwrap();
    let present = tool.present_call.as_ref().unwrap();
    let view = present(&json!({
        "command": "view", "path": "/workspace/a.txt", "file_text": null,
        "insert_line": null, "new_str": null, "old_str": null, "view_range": null,
    }))
    .unwrap();
    assert_eq!(
        view,
        ToolCallView::Generic(hoh_tools::GenericCallView {
            title: "view /workspace/a.txt".into(),
            kind: Some(ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: Some(vec![hoh_tools::FileLocation {
                path: "/workspace/a.txt".into(),
                line: None,
            }]),
        })
    );
    let create = serde_json::to_value(
        present(&json!({
            "command": "create", "path": "/workspace/a.txt", "file_text": "hello",
            "insert_line": null, "new_str": null, "old_str": null, "view_range": null,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(create["diffs"][0]["oldText"], Value::Null);
    assert_eq!(create["diffs"][0]["newText"], "hello");
    let replace = serde_json::to_value(
        present(&json!({
            "command": "str_replace", "path": "/workspace/a.txt",
            "old_str": "old", "new_str": "new", "file_text": null,
            "insert_line": null, "view_range": null,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(replace["diffs"][0]["oldText"], "old");
    assert_eq!(replace["diffs"][0]["newText"], "new");
    let insert_null = serde_json::to_value(
        present(&json!({
            "command": "insert", "path": "/workspace/a.txt",
            "insert_line": null, "new_str": "x",
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(insert_null["locations"][0].get("line").is_none());
    let insert_zero = serde_json::to_value(
        present(&json!({
            "command": "insert", "path": "/workspace/a.txt",
            "insert_line": 0, "new_str": "x", "file_text": null,
            "old_str": null, "view_range": null,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(insert_zero["locations"][0]["line"], 1);
    let insert_unrepresentable = serde_json::to_value(
        present(&json!({
            "command": "insert", "path": "/workspace/a.txt",
            "insert_line": 1e20, "new_str": "x",
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        insert_unrepresentable["locations"][0].get("line").is_none(),
        "an unrepresentable JS line must not be reported as u64::MAX"
    );
    for (command, field) in [("create", "newText"), ("str_replace", "newText")] {
        let value = serde_json::to_value(
            present(&json!({ "command": command, "path": "/workspace/empty.txt" })).unwrap(),
        )
        .unwrap();
        assert_eq!(value["diffs"][0][field], "");
    }
    let insert_missing = serde_json::to_value(
        present(&json!({ "command": "insert", "path": "/workspace/a.txt" })).unwrap(),
    )
    .unwrap();
    assert!(insert_missing["locations"][0].get("line").is_none());

    harness.editor.dispose().await;
    assert!(tools.schemas(None, None).unwrap().is_empty());
    assert!(tools.get("str_replace_editor", None, None).is_none());
}

#[tokio::test]
async fn creates_views_replaces_and_inserts_with_the_canonical_model_facing_output() {
    let harness = setup(json!({}), false, None).await;
    let sample = path(&harness, "sample.txt");
    let created = call(
        &harness,
        true,
        json!({
            "command": "create", "path": sample, "file_text": "one\ntwo\nthree\n",
            "insert_line": null, "new_str": null, "old_str": null, "view_range": null,
        }),
    )
    .await;
    assert_eq!(
        text(&created),
        format!("New file created successfully at: {sample}")
    );
    let full = call(
        &harness,
        true,
        json!({
            "command": "view", "path": sample, "file_text": null,
            "insert_line": null, "new_str": null, "old_str": null, "view_range": null,
        }),
    )
    .await;
    assert!(text(&full).contains("     2  two"));
    let ranged = call(
        &harness,
        true,
        json!({ "command": "view", "path": sample, "view_range": [2, -1] }),
    )
    .await;
    assert_eq!(
        text(&ranged),
        [
            format!("Here's the content of {sample} with line numbers (which has a total of 4 lines) with view_range=[2, -1]:"),
            "     2  two".into(),
            "     3  three".into(),
            "     4  ".into(),
            String::new(),
        ]
        .join("\n")
    );
    assert_eq!(
        text(
            &call(
                &harness,
                true,
                json!({
                    "command": "str_replace", "path": sample, "old_str": "two",
                    "new_str": "TWO", "file_text": null, "insert_line": null,
                    "view_range": null,
                }),
            )
            .await
        ),
        format!("The file {sample} has been edited successfully.")
    );
    assert_eq!(
        text(
            &call(
                &harness,
                true,
                json!({ "command": "str_replace", "path": sample, "old_str": "TWO" }),
            )
            .await
        ),
        format!("The file {sample} has been edited successfully.")
    );
    assert_eq!(
        text(
            &call(
                &harness,
                true,
                json!({
                    "command": "insert", "path": sample, "insert_line": 1,
                    "new_str": "between", "file_text": null, "old_str": null,
                    "view_range": null,
                }),
            )
            .await
        ),
        format!("The file {sample} has been edited successfully.")
    );
    assert_eq!(
        tokio::fs::read_to_string(sample).await.unwrap(),
        "one\nbetween\n\nthree\n"
    );
}

#[tokio::test]
async fn a_failed_view_records_absence_so_create_can_recover_after_external_deletion() {
    let harness = setup(json!({}), true, None).await;
    let sample = path(&harness, "deleted.txt");
    tokio::fs::write(&sample, "original").await.unwrap();
    assert!(
        !call(&harness, true, json!({ "command": "view", "path": sample }))
            .await
            .is_error()
    );
    tokio::fs::remove_file(&sample).await.unwrap();
    let missing = call(&harness, true, json!({ "command": "view", "path": sample })).await;
    assert_error(&missing, "FsError", "FS_NOT_FOUND");
    let edit = call(
        &harness,
        true,
        json!({ "command": "str_replace", "path": sample, "old_str": "original", "new_str": "edited" }),
    )
    .await;
    assert_error(&edit, "FsError", "FS_NOT_FOUND");
    let created = call(
        &harness,
        true,
        json!({ "command": "create", "path": sample, "file_text": "fresh" }),
    )
    .await;
    assert!(!created.is_error());
    assert_eq!(tokio::fs::read_to_string(sample).await.unwrap(), "fresh");
}

#[tokio::test]
async fn writes_replacement_text_literally() {
    let harness = setup(json!({}), false, None).await;
    let sample = path(&harness, "literal.txt");
    let replacement = "$&|$`|$'|$$";
    tokio::fs::write(&sample, "before OLD after").await.unwrap();
    let result = call(
        &harness,
        true,
        json!({ "command": "str_replace", "path": sample, "old_str": "OLD", "new_str": replacement }),
    )
    .await;
    assert!(!result.is_error());
    assert_eq!(
        tokio::fs::read_to_string(sample).await.unwrap(),
        format!("before {replacement} after")
    );
}

#[tokio::test]
async fn lists_visible_entries_to_depth_two_and_clips_at_the_configured_view_limit() {
    let harness = setup(json!({ "maxOutputChars": 10_000 }), false, None).await;
    let directory = path(&harness, "dir");
    for name in [
        "nested/third",
        "node_modules/pkg",
        "node_modules_old",
        "__pycache__",
        "__pycache__backup",
    ] {
        tokio::fs::create_dir_all(format!("{directory}/{name}"))
            .await
            .unwrap();
    }
    for (name, content) in [
        ("visible.txt", "ok"),
        (".hidden", "hidden"),
        ("nested/child.txt", "child"),
        ("nested/third/too-deep.txt", "deep"),
        ("node_modules/pkg/index.js", "hidden dependency"),
        ("node_modules_old/kept.js", "visible source"),
        ("__pycache__/module.pyc", "cache"),
        ("__pycache__backup/kept.py", "visible source"),
    ] {
        tokio::fs::write(format!("{directory}/{name}"), content)
            .await
            .unwrap();
    }
    let other = harness
        .fs
        .resolve(&format!("{directory}/other"), FsResolveOptions::default())
        .await
        .unwrap();
    *lock(&harness.fs.controls.reverse_root_listing) = Some((directory.clone(), other));
    let listing = text(
        &call(
            &harness,
            true,
            json!({ "command": "view", "path": directory }),
        )
        .await,
    );
    for hidden in [".hidden", "too-deep.txt", "index.js", "module.pyc"] {
        assert!(!listing.contains(hidden), "{hidden}");
    }
    assert!(listing.contains("node_modules_old/kept.js"));
    assert!(listing.contains("__pycache__backup/kept.py"));

    let clipped = setup(json!({ "maxOutputChars": 10 }), false, None).await;
    let large = path(&clipped, "large.txt");
    tokio::fs::write(&large, "x".repeat(100)).await.unwrap();
    let output = text(&call(&clipped, true, json!({ "command": "view", "path": large })).await);
    assert!(output.contains("<response clipped>"));
}

#[tokio::test]
async fn clips_a_view_at_a_code_unit_boundary_without_splitting_a_surrogate_pair() {
    let wide = setup(json!({ "maxOutputChars": 10_000 }), false, None).await;
    let file = path(&wide, "emoji.txt");
    tokio::fs::write(&file, format!("{}tail", "😀".repeat(4)))
        .await
        .unwrap();
    let untruncated = text(&call(&wide, true, json!({ "command": "view", "path": file })).await);
    let byte = untruncated.find('😀').unwrap();
    let split_at = untruncated[..byte].encode_utf16().count() + 1;

    wide.editor.dispose().await;
    let editor = wide
        .ctx
        .plugin::<ToolStrReplaceEditor>(json!({ "maxOutputChars": split_at }))
        .unwrap();
    editor.settled().await.unwrap();
    let rendered = text(&call(&wide, true, json!({ "command": "view", "path": file })).await);
    let expected_prefix = &untruncated[..byte];
    assert!(rendered.starts_with(&format!("{expected_prefix}<response clipped>")));
    assert!(!rendered.contains('😀'));
}

#[tokio::test]
async fn matches_canonical_empty_line_range_and_end_insert_behavior() {
    let harness = setup(json!({}), false, None).await;
    let empty = path(&harness, "empty.txt");
    let newline = path(&harness, "newline.txt");
    let plain = path(&harness, "plain.txt");
    tokio::fs::write(&empty, "").await.unwrap();
    tokio::fs::write(&newline, "\n").await.unwrap();
    tokio::fs::write(&plain, "one\ntwo").await.unwrap();
    assert!(
        text(&call(&harness, true, json!({ "command": "view", "path": empty })).await)
            .contains("(which has a total of 1 lines):\n     1  \n")
    );
    assert!(
        text(
            &call(
                &harness,
                true,
                json!({ "command": "view", "path": newline })
            )
            .await
        )
        .contains("(which has a total of 2 lines):\n     1  \n     2  \n")
    );
    assert!(
        text(
            &call(
                &harness,
                true,
                json!({ "command": "view", "path": plain, "view_range": [1, 2] })
            )
            .await
        )
        .contains("     2  two")
    );
    assert!(
        text(&call(&harness, false, json!({ "command": "view", "path": plain })).await)
            .contains("     1  one")
    );
    let ownerless = path(&harness, "ownerless.txt");
    assert!(
        !call(
            &harness,
            false,
            json!({ "command": "create", "path": ownerless, "file_text": "ownerless" })
        )
        .await
        .is_error()
    );
    assert!(
        !call(
            &harness,
            true,
            json!({ "command": "insert", "path": plain, "insert_line": 2, "new_str": "three" })
        )
        .await
        .is_error()
    );
    assert_eq!(
        tokio::fs::read_to_string(&plain).await.unwrap(),
        "one\ntwo\nthree"
    );
    tokio::fs::write(&newline, "one\n").await.unwrap();
    assert!(
        !call(
            &harness,
            true,
            json!({ "command": "insert", "path": newline, "insert_line": 2, "new_str": "three" })
        )
        .await
        .is_error()
    );
    assert_eq!(
        tokio::fs::read_to_string(newline).await.unwrap(),
        "one\n\nthree"
    );
}

#[tokio::test]
async fn uses_old_str_only_replacement_failures_and_rejects_relative_paths() {
    let harness = setup(json!({}), false, None).await;
    let ambiguous = path(&harness, "ambiguous.txt");
    tokio::fs::write(&ambiguous, "same\nother\nsame")
        .await
        .unwrap();
    let missing = call(
        &harness,
        true,
        json!({ "command": "str_replace", "path": ambiguous, "old_str": "absent", "new_str": "x" }),
    )
    .await;
    assert!(text(&missing).contains(&format!(
        "old_str `absent` did not appear verbatim in {ambiguous}"
    )));
    assert!(!text(&missing).contains("old_string"));
    let repeated = call(
        &harness,
        true,
        json!({ "command": "str_replace", "path": ambiguous, "old_str": "same", "new_str": "x" }),
    )
    .await;
    assert!(text(&repeated).contains("Multiple occurrences of old_str `same` in lines [1, 3]"));
    assert!(!text(&repeated).contains("replace_all"));
    tokio::fs::write(&ambiguous, "alpha\nbeta\nmiddle\nalpha\nbeta")
        .await
        .unwrap();
    let multiline = call(&harness, true, json!({ "command": "str_replace", "path": ambiguous, "old_str": "alpha\nbeta", "new_str": "x" })).await;
    assert!(
        text(&multiline).contains("Multiple occurrences of old_str `alpha\nbeta` in lines [1, 4]")
    );
    let mixed = path(&harness, "mixed-eol.txt");
    tokio::fs::write(&mixed, "alpha\r\nbeta\nmiddle\nalpha\nbeta")
        .await
        .unwrap();
    assert!(!call(&harness, true, json!({ "command": "str_replace", "path": mixed, "old_str": "alpha\r\nbeta", "new_str": "replaced" })).await.is_error());
    assert_eq!(
        tokio::fs::read_to_string(mixed).await.unwrap(),
        "replaced\nmiddle\nalpha\nbeta"
    );
    let relative = call(
        &harness,
        true,
        json!({ "command": "view", "path": "ambiguous.txt" }),
    )
    .await;
    assert!(text(&relative).contains("is not an absolute path"));
    assert_error(&relative, "ToolArgsError", "INVALID_ARGS");
    assert_eq!(
        tokio::fs::read_to_string(ambiguous).await.unwrap(),
        "alpha\nbeta\nmiddle\nalpha\nbeta"
    );
}

#[tokio::test]
async fn reports_invalid_commands_or_arguments_without_mutating_files() {
    let harness = setup(json!({}), false, None).await;
    let ambiguous = path(&harness, "ambiguous.txt");
    let empty = path(&harness, "empty.txt");
    let three_lines = path(&harness, "three-lines.txt");
    let directory = path(&harness, "directory");
    tokio::fs::write(&ambiguous, "same same").await.unwrap();
    tokio::fs::write(&empty, "").await.unwrap();
    tokio::fs::write(&three_lines, "one\ntwo\nthree")
        .await
        .unwrap();
    tokio::fs::create_dir(&directory).await.unwrap();
    let missing = path(&harness, "missing.txt");
    let new_file = path(&harness, "new.txt");
    let cases = vec![
        (
            json!({ "command": null, "path": ambiguous }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": null }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": "" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": missing }),
            "FsError",
            "FS_NOT_FOUND",
        ),
        (
            json!({ "command": "view", "path": ambiguous, "view_range": [1] }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": ambiguous, "view_range": [0, 1] }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": ambiguous, "view_range": [1.5, 2] }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": three_lines, "view_range": [1, 99] }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": three_lines, "view_range": [2, 1] }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "view", "path": directory, "view_range": [1, 1] }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "create", "path": new_file }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "create", "path": new_file, "file_text": null }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "create", "path": ambiguous, "file_text": "overwrite" }),
            "FsError",
            "FS_ALREADY_EXISTS",
        ),
        (
            json!({ "command": "str_replace", "path": ambiguous, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "str_replace", "path": ambiguous, "old_str": null, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "str_replace", "path": ambiguous, "old_str": "same same", "new_str": null }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "str_replace", "path": ambiguous, "old_str": "", "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": ambiguous, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": ambiguous, "insert_line": null, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": ambiguous, "insert_line": 0, "new_str": null }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": ambiguous, "insert_line": -1, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": ambiguous, "insert_line": 1.5, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": ambiguous, "insert_line": 99, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": empty, "insert_line": 2, "new_str": "x" }),
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "command": "insert", "path": directory, "insert_line": 0, "new_str": "x" }),
            "FsError",
            "FS_NOT_REGULAR_FILE",
        ),
    ];
    for (arguments, name, code) in cases {
        let result = call(&harness, true, arguments.clone()).await;
        assert!(result.is_error(), "{arguments}");
        assert_error(&result, name, code);
    }
    assert_eq!(
        tokio::fs::read_to_string(&ambiguous).await.unwrap(),
        "same same"
    );

    harness
        .fs
        .controls
        .force_other
        .store(true, Ordering::SeqCst);
    let special_path = path(&harness, "special");
    let special = call(
        &harness,
        true,
        json!({ "command": "view", "path": special_path }),
    )
    .await;
    assert_error(&special, "FsError", "FS_NOT_REGULAR_FILE");
    let replace = call(
        &harness,
        true,
        json!({ "command": "str_replace", "path": special_path, "old_str": "x", "new_str": "y" }),
    )
    .await;
    assert_error(&replace, "FsError", "FS_NOT_REGULAR_FILE");
    let insert = call(
        &harness,
        true,
        json!({ "command": "insert", "path": special_path, "insert_line": 0, "new_str": "x" }),
    )
    .await;
    assert_error(&insert, "FsError", "FS_NOT_REGULAR_FILE");
}

#[tokio::test]
async fn delegates_read_before_edit_decisions_to_fs_observation_policy() {
    let harness = setup(json!({}), true, None).await;
    let existing = path(&harness, "existing.txt");
    let created = path(&harness, "created.txt");
    tokio::fs::write(&existing, "before").await.unwrap();
    let blind = call(&harness, true, json!({ "command": "str_replace", "path": existing, "old_str": "before", "new_str": "after" })).await;
    assert_error(&blind, "FsError", "FS_NOT_OBSERVED");
    assert_eq!(
        tokio::fs::read_to_string(&existing).await.unwrap(),
        "before"
    );
    assert!(
        !call(
            &harness,
            true,
            json!({ "command": "view", "path": existing })
        )
        .await
        .is_error()
    );
    assert!(!call(&harness, true, json!({ "command": "str_replace", "path": existing, "old_str": "before", "new_str": "after" })).await.is_error());
    assert_eq!(tokio::fs::read_to_string(&existing).await.unwrap(), "after");
    assert!(
        !call(
            &harness,
            true,
            json!({ "command": "insert", "path": existing, "insert_line": 1, "new_str": "tail" })
        )
        .await
        .is_error()
    );
    assert_eq!(
        tokio::fs::read_to_string(existing).await.unwrap(),
        "after\ntail"
    );
    assert!(
        !call(
            &harness,
            true,
            json!({ "command": "create", "path": created, "file_text": "new" })
        )
        .await
        .is_error()
    );
    assert_eq!(tokio::fs::read_to_string(created).await.unwrap(), "new");
}

#[tokio::test]
async fn passes_the_session_sandbox_policy_to_every_mutation() {
    let harness = setup(json!({}), false, Some(SandboxMode::ReadOnly)).await;
    let blocked = path(&harness, "blocked.txt");
    let result = call(
        &harness,
        true,
        json!({ "command": "create", "path": blocked, "file_text": "blocked" }),
    )
    .await;
    assert_error(&result, "FsError", "FS_SANDBOX_DENIED");
    assert!(text(&result).contains("[sandbox: file access denied under read-only mode]"));
    let ownerless = path(&harness, "ownerless-blocked.txt");
    let result = call(
        &harness,
        false,
        json!({ "command": "create", "path": ownerless, "file_text": "blocked" }),
    )
    .await;
    assert_error(&result, "FsError", "FS_SANDBOX_DENIED");
}

#[tokio::test]
async fn preserves_tabs_outside_the_edited_region() {
    let harness = setup(json!({}), false, None).await;
    let makefile = path(&harness, "Makefile");
    tokio::fs::write(&makefile, "target:\n\told\nremove\n")
        .await
        .unwrap();
    let view = call(
        &harness,
        true,
        json!({ "command": "view", "path": makefile }),
    )
    .await;
    assert!(text(&view).contains("     2  \told"));
    assert!(!call(&harness, true, json!({ "command": "str_replace", "path": makefile, "old_str": "\told", "new_str": "\tnew" })).await.is_error());
    assert!(
        !call(
            &harness,
            true,
            json!({ "command": "str_replace", "path": makefile, "old_str": "remove\n" })
        )
        .await
        .is_error()
    );
    assert!(
        !call(
            &harness,
            true,
            json!({ "command": "insert", "path": makefile, "insert_line": 1, "new_str": "\tkept" })
        )
        .await
        .is_error()
    );
    assert_eq!(
        tokio::fs::read_to_string(makefile).await.unwrap(),
        "target:\n\tkept\n\tnew\n"
    );
}

#[tokio::test]
async fn reports_missing_sandbox_policy_composition_during_plugin_startup() {
    let root = TempDir::new().unwrap();
    let root_text = root.path().to_string_lossy().into_owned();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount::<hoh_system_prompt::SystemPrompt>(&ctx, json!({})).await;
    mount::<ToolRuntime>(&ctx, json!({})).await;
    let fs = Arc::new(TestFileSystem::new(&root_text, Some(SandboxMode::ReadOnly)));
    ctx.provide::<Fs>(fs).await.unwrap();
    let fiber = ctx.plugin::<ToolStrReplaceEditor>(json!({})).unwrap();
    let error = fiber.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the mounted filesystem confines but ctx.sandboxPolicy is missing")
    );
}

#[tokio::test]
async fn reports_a_file_created_after_the_absence_check_as_fs_already_exists() {
    let harness = setup(json!({}), false, None).await;
    let raced = path(&harness, "raced.txt");
    tokio::fs::write(&raced, "concurrent").await.unwrap();
    harness
        .fs
        .controls
        .first_stat_absent
        .store(true, Ordering::SeqCst);
    let result = call(
        &harness,
        true,
        json!({ "command": "create", "path": raced, "file_text": "mine" }),
    )
    .await;
    assert_error(&result, "FsError", "FS_ALREADY_EXISTS");
    assert!(text(&result).contains(&format!("File already exists at: {raced}")));
    assert_eq!(
        tokio::fs::read_to_string(raced).await.unwrap(),
        "concurrent"
    );
}

#[tokio::test]
async fn maps_unexpected_backend_write_failures_for_replace_and_insert() {
    let harness = setup(json!({}), false, None).await;
    let backend = path(&harness, "backend-error.txt");
    tokio::fs::write(&backend, "old\n").await.unwrap();
    harness.fs.controls.fail_write.store(true, Ordering::SeqCst);
    let replace = call(
        &harness,
        true,
        json!({ "command": "str_replace", "path": backend, "old_str": "old", "new_str": "new" }),
    )
    .await;
    assert!(replace.is_error());
    assert!(text(&replace).contains("backend write failed"));
    let insert = call(
        &harness,
        true,
        json!({ "command": "insert", "path": backend, "insert_line": 1, "new_str": "new" }),
    )
    .await;
    assert!(insert.is_error());
    assert!(text(&insert).contains("backend write failed"));
}

#[tokio::test]
async fn rejects_invalid_plugin_config() {
    async fn rejection(config: Value) -> String {
        let root = TempDir::new().unwrap();
        let root_text = root.path().to_string_lossy().into_owned();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount::<hoh_system_prompt::SystemPrompt>(&ctx, json!({})).await;
        mount::<ToolRuntime>(&ctx, json!({})).await;
        ctx.provide::<Fs>(Arc::new(TestFileSystem::new(&root_text, None)))
            .await
            .unwrap();
        let fiber = ctx.plugin::<ToolStrReplaceEditor>(config).unwrap();
        let error = fiber.settled().await.unwrap_err();
        error.to_string()
    }
    assert!(
        rejection(json!({ "maxOutputChars": 0 }))
            .await
            .contains("maxOutputChars must be a positive safe integer")
    );
    assert!(
        rejection(json!({ "description": " " }))
            .await
            .contains("description must be non-empty")
    );
}
