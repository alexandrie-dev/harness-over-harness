use hoh_credentials::{
    CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
    CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef, Credentials,
    ResolvedCredential, credential_ref,
};
use hoh_kernel::{Context, Kernel, Service};
use hoh_mcp_client::{McpClient, McpProbeOptions, McpProbeOutcome, ReconnectConfig};
use hoh_mcp_manager::{
    McpManagedServerEntry, McpManagedServerStatus, McpManager, McpManagerError, McpManagerStatus,
};
use hoh_system_prompt::SystemPrompt;
use hoh_tools::{ToolRuntime, Tools};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const HELPER_ENV: &str = "HOH_MCP_MANAGER_STDIO_HELPER";
const HELPER_TOOL_ENV: &str = "HOH_MCP_MANAGER_STDIO_TOOL";
const HELPER_START_LOG_ENV: &str = "HOH_MCP_MANAGER_STDIO_START_LOG";
const HELPER_DELAY_MS_ENV: &str = "HOH_MCP_MANAGER_STDIO_DELAY_MS";
const HELPER_HANG_AFTER_FIRST_ENV: &str = "HOH_MCP_MANAGER_STDIO_HANG_AFTER_FIRST";

static TEMP_IDS: AtomicU64 = AtomicU64::new(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

type ResolveGate = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

struct TestCredentials {
    value: Mutex<Option<String>>,
    next_gate: Mutex<Option<ResolveGate>>,
    fail_next: AtomicBool,
    resolves: AtomicU64,
}

impl TestCredentials {
    fn new(value: Option<&str>) -> Self {
        Self {
            value: Mutex::new(value.map(str::to_owned)),
            next_gate: Mutex::new(None),
            fail_next: AtomicBool::new(false),
            resolves: AtomicU64::new(0),
        }
    }

    fn replace(&self, value: Option<&str>) {
        *lock(&self.value) = value.map(str::to_owned);
    }

    fn gate_next(&self) -> ResolveGate {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *lock(&self.next_gate) = Some((entered.clone(), release.clone()));
        (entered, release)
    }

    fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn resolves(&self) -> u64 {
        self.resolves.load(Ordering::SeqCst)
    }
}

impl CredentialProvider for TestCredentials {
    fn resolve<'a>(
        &'a self,
        _reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        let gate = lock(&self.next_gate).take();
        let fail = self.fail_next.swap(false, Ordering::SeqCst);
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            if fail {
                return Err("credential provider unavailable".into());
            }
            let value = lock(&self.value).clone();
            Ok(value.map(|value| ResolvedCredential {
                value,
                source: "test".into(),
            }))
        })
    }

    fn describe<'a>(
        &'a self,
        _reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, CredentialInfo> {
        let configured = lock(&self.value).is_some();
        Box::pin(async move {
            Ok(CredentialInfo {
                configured,
                source: configured.then(|| "test".into()),
                writable: true,
                masked: None,
            })
        })
    }

    fn set<'a>(
        &'a self,
        _reference: &'a CredentialRef,
        value: &'a str,
    ) -> CredentialFuture<'a, ()> {
        self.replace(Some(value));
        Box::pin(async { Ok(()) })
    }

    fn unset<'a>(&'a self, _reference: &'a CredentialRef) -> CredentialFuture<'a, ()> {
        self.replace(None);
        Box::pin(async { Ok(()) })
    }

    fn read_record<'a>(
        &'a self,
        _key: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async { Ok(None) })
    }

    fn describe_record<'a>(
        &'a self,
        _key: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo> {
        Box::pin(async {
            Ok(CredentialRecordInfo {
                configured: false,
                kind: None,
                writable: false,
            })
        })
    }

    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn modify_record<'a>(
        &'a self,
        _key: &'a CredentialKey,
        _mutate: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async { Ok(None) })
    }

    fn delete_record<'a>(&'a self, _key: &'a CredentialKey) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct StartLog(PathBuf);

impl StartLog {
    fn new(label: &str) -> Self {
        let id = TEMP_IDS.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "hoh-mcp-manager-{label}-{}-{id}.log",
            std::process::id()
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn starts(&self) -> usize {
        std::fs::read_to_string(&self.0)
            .map(|text| text.lines().count())
            .unwrap_or_default()
    }
}

impl Drop for StartLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn fixture_executable() -> PathBuf {
    let current = std::env::current_exe().expect("current integration-test executable");
    let deps = current.parent().expect("target debug/deps directory");
    std::fs::read_dir(deps)
        .expect("read target debug/deps")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mcp_stdio_fixture-"))
                && if std::env::consts::EXE_EXTENSION.is_empty() {
                    path.extension().is_none()
                } else {
                    path.extension().and_then(|value| value.to_str())
                        == Some(std::env::consts::EXE_EXTENSION)
                }
        })
        .max_by_key(|path| {
            path.metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .expect("cargo must build the mcp_stdio_fixture test target")
}

fn live_entry(tool_name: &str, starts: &StartLog) -> McpManagedServerEntry {
    let mut entry = McpManagedServerEntry::stdio(fixture_executable().to_string_lossy());
    entry.env.insert(HELPER_ENV.into(), "1".into());
    entry
        .env
        .insert(HELPER_TOOL_ENV.into(), tool_name.to_owned());
    entry.env.insert(
        HELPER_START_LOG_ENV.into(),
        starts.path().to_string_lossy().into_owned(),
    );
    entry.startup_timeout_ms = 10_000;
    entry.close_timeout_ms = Some(2_000);
    entry.reconnect = Some(ReconnectConfig {
        enabled: Some(false),
        initial_delay_ms: None,
        max_delay_ms: None,
        max_attempts: None,
    });
    entry
}

fn direct_client_config(server_name: &str, entry: &McpManagedServerEntry) -> serde_json::Value {
    json!({
        "transport": "stdio",
        "serverName": server_name,
        "command": entry.command.clone(),
        "args": entry.args.clone(),
        "env": entry.env.clone(),
        "cwd": entry.cwd.clone(),
        "toolCallTimeoutMs": entry.tool_call_timeout_ms.unwrap_or(60_000),
        "startupTimeoutMs": entry.startup_timeout_ms,
        "closeTimeoutMs": entry.close_timeout_ms.unwrap_or(5_000),
        "expandTemplates": true,
        "failOnStartupError": false,
        "reconnect": entry.reconnect,
    })
}

fn live_tests() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("live MCP condition timed out");
}

fn has_tool(ctx: &Context, name: &str) -> bool {
    ctx.get::<Tools>()
        .unwrap()
        .known_names(None)
        .iter()
        .any(|candidate| candidate == name)
}

async fn wait_status(manager: &Service<McpManager>, name: &str, state: &str) {
    struct Report<'a>(&'a Service<McpManager>);
    impl Drop for Report<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                for view in self.0.list() {
                    eprintln!("DBG {} {:?}", view.name, view.status);
                }
            }
        }
    }
    let _report = Report(manager);
    wait_until(|| {
        manager
            .list()
            .iter()
            .find(|view| view.name == name)
            .is_some_and(|view| view.status.state() == state)
    })
    .await;
}

async fn world(config: serde_json::Value) -> (Kernel, hoh_kernel::Fiber, Service<McpManager>) {
    world_with_credentials(config, None).await
}

/// A world for live-transport cases whose entries carry no credential templates. The client
/// binding of this port needs a credentials service to resolve any entry with template
/// expansion on (see PORT_NOTES, "Вопросы ведущему"), so an empty provider is mounted.
async fn live_world(config: serde_json::Value) -> (Kernel, hoh_kernel::Fiber, Service<McpManager>) {
    world_with_credentials(config, Some(Arc::new(TestCredentials::new(None)))).await
}

async fn world_with_credentials(
    config: serde_json::Value,
    credentials: Option<Arc<dyn CredentialProvider>>,
) -> (Kernel, hoh_kernel::Fiber, Service<McpManager>) {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    if let Some(credentials) = credentials {
        root.provide::<Credentials>(credentials).await.unwrap();
    }
    let fiber = root.plugin::<McpManager>(config).unwrap();
    fiber.settled().await.unwrap();
    let manager = root.get::<McpManager>().unwrap();
    (kernel, fiber, manager)
}

#[tokio::test]
async fn disabled_config_is_listed_without_mounting_a_client() {
    let _guard = live_tests().lock().await;
    let (kernel, _, manager) = world(json!({
        "servers": { "off": { "transport": "stdio", "command": "echo", "enabled": false } }
    }))
    .await;
    assert_eq!(manager.list().len(), 1);
    assert_eq!(manager.list()[0].name, "off");
    assert_eq!(manager.list()[0].status, McpManagedServerStatus::Disabled);
    kernel.shutdown().await;
}

#[tokio::test]
async fn watch_starts_with_a_complete_snapshot() {
    let _guard = live_tests().lock().await;
    let (kernel, _, manager) = world(json!({
        "servers": { "off": { "transport": "stdio", "command": "echo", "enabled": false } }
    }))
    .await;
    let receiver = manager.watch();
    assert_eq!(receiver.borrow().as_slice(), manager.list());
    kernel.shutdown().await;
}

#[tokio::test]
async fn volatile_add_change_and_delete_keep_the_fiber() {
    let _guard = live_tests().lock().await;
    let (kernel, fiber, manager) = world(json!({ "servers": {} })).await;
    let uid = fiber.uid();
    fiber
        .commit_volatile(json!({ "servers": {
            "off": { "transport": "stdio", "command": "echo", "enabled": false }
        }}))
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(manager.list()[0].status, McpManagedServerStatus::Disabled);
    fiber
        .commit_volatile(json!({ "servers": {} }))
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert!(manager.list().is_empty());
    assert_eq!(fiber.uid(), uid);
    kernel.shutdown().await;
}

#[tokio::test]
async fn restart_rejects_unknown_and_reaffirms_disabled() {
    let _guard = live_tests().lock().await;
    let (kernel, _, manager) = world(json!({
        "servers": { "off": { "transport": "stdio", "command": "echo", "enabled": false } }
    }))
    .await;
    manager.restart("off").await.unwrap();
    assert_eq!(manager.list()[0].status, McpManagedServerStatus::Disabled);
    assert_eq!(
        manager.restart("ghost").await,
        Err(McpManagerError::UnknownRestartServer("ghost".into()))
    );
    assert_eq!(
        manager.restart("ghost").await.unwrap_err().to_string(),
        "mcp-manager: unknown server \"ghost\" — it is neither configured nor registered"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn probe_rejects_an_unknown_name_without_io() {
    let _guard = live_tests().lock().await;
    let (kernel, _, manager) = world(json!({ "servers": {} })).await;
    assert_eq!(
        manager.probe("ghost", McpProbeOptions::default()).await,
        Err(McpManagerError::UnknownServer("ghost".into()))
    );
    assert_eq!(
        manager
            .probe("ghost", McpProbeOptions::default())
            .await
            .unwrap_err()
            .to_string(),
        "mcp-manager: unknown server \"ghost\""
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn registration_is_owned_by_the_callers_context() {
    let _guard = live_tests().lock().await;
    let (kernel, _, manager) = world(json!({ "servers": {} })).await;
    let root = kernel.root();
    let mut entry = McpManagedServerEntry::stdio("echo");
    entry.enabled = false;
    let disposer = manager.register(&root, "app", entry).unwrap();
    let registered = manager.list();
    assert_eq!(registered[0].name, "app");
    assert_ne!(registered[0].status, McpManagedServerStatus::Disabled);
    disposer.dispose().await;
    tokio::task::yield_now().await;
    assert!(manager.list().is_empty());
    kernel.shutdown().await;
}

#[test]
fn public_error_texts_match_the_source() {
    assert_eq!(
        McpManagerError::Changing("srv".into()).to_string(),
        "mcp-manager: configuration for \"srv\" is changing"
    );
    assert_eq!(
        McpManagerError::Cleanup("srv".into()).to_string(),
        "mcp-manager: cleanup for \"srv\" is incomplete; restart the Host"
    );
    assert_eq!(
        McpManagerError::Duplicate("srv".into()).to_string(),
        "mcp-manager: server \"srv\" is already configured or registered"
    );
    assert_eq!(
        McpManagerError::RegistrationConflict("srv".into()).to_string(),
        "servers.srv: a plugin already registers this server name — rename the entry key"
    );
}

#[tokio::test]
async fn registration_rejects_configured_and_registered_names() {
    let _guard = live_tests().lock().await;
    let (kernel, _, manager) = world(json!({
        "servers": { "base": { "transport": "stdio", "command": "echo", "enabled": false } }
    }))
    .await;
    let root = kernel.root();
    let mut entry = McpManagedServerEntry::stdio("echo");
    entry.enabled = false;
    assert!(matches!(
        manager.register(&root, "base", entry.clone()),
        Err(McpManagerError::Duplicate(_))
    ));
    let _registration = manager.register(&root, "app", entry.clone()).unwrap();
    assert!(matches!(
        manager.register(&root, "app", entry),
        Err(McpManagerError::Duplicate(_))
    ));
    kernel.shutdown().await;
}

#[tokio::test]
async fn disposed_manager_refuses_operations() {
    let _guard = live_tests().lock().await;
    let (kernel, fiber, manager) = world(json!({
        "servers": { "off": { "transport": "stdio", "command": "echo", "enabled": false } }
    }))
    .await;
    fiber.dispose().await;
    assert_eq!(manager.restart("off").await, Err(McpManagerError::Disposed));
    kernel.shutdown().await;
}

#[tokio::test]
async fn disposal_closes_existing_watchers() {
    let _guard = live_tests().lock().await;
    let (kernel, fiber, manager) = world(json!({ "servers": {} })).await;
    let mut receiver = manager.watch();
    fiber.dispose().await;
    assert!(receiver.changed().await.is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn watcher_created_after_disposal_is_already_closed() {
    let _guard = live_tests().lock().await;
    let (kernel, fiber, manager) = world(json!({ "servers": {} })).await;
    fiber.dispose().await;
    let mut receiver = manager.watch();
    assert!(receiver.changed().await.is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn live_stdio_server_mounts_probes_restarts_reconfigures_and_unmounts() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("lifecycle");
    let alpha = live_entry("alpha", &starts);
    let alpha_value = serde_json::to_value(&alpha).unwrap();
    let (kernel, fiber, manager) = live_world(json!({ "servers": { "live": alpha_value } })).await;
    let root = kernel.root();
    let alpha_name = hoh_mcp_client::public_tool_name("live", "alpha");
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &alpha_name)).await;
    assert_eq!(starts.starts(), 1);

    let probe = manager
        .probe("live", McpProbeOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        probe.outcome,
        McpProbeOutcome::Success { ref tool_names, .. } if tool_names == &["alpha"]
    ));
    wait_until(|| starts.starts() >= 2).await;

    manager.restart("live").await.unwrap();
    wait_until(|| starts.starts() >= 3).await;
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &alpha_name)).await;

    let beta = live_entry("beta", &starts);
    fiber
        .commit_volatile(json!({ "servers": { "live": serde_json::to_value(&beta).unwrap() } }))
        .await
        .unwrap();
    let beta_name = hoh_mcp_client::public_tool_name("live", "beta");
    wait_until(|| starts.starts() >= 4).await;
    wait_status(&manager, "live", "connected").await;
    wait_until(|| !has_tool(&root, &alpha_name) && has_tool(&root, &beta_name)).await;

    let mut disabled = beta.clone();
    disabled.enabled = false;
    fiber
        .commit_volatile(json!({ "servers": { "live": serde_json::to_value(&disabled).unwrap() } }))
        .await
        .unwrap();
    wait_status(&manager, "live", "disabled").await;
    wait_until(|| !has_tool(&root, &beta_name)).await;

    fiber
        .commit_volatile(json!({ "servers": { "live": serde_json::to_value(&beta).unwrap() } }))
        .await
        .unwrap();
    wait_until(|| starts.starts() >= 5).await;
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &beta_name)).await;

    fiber
        .commit_volatile(json!({ "servers": {} }))
        .await
        .unwrap();
    wait_until(|| manager.list().is_empty() && !has_tool(&root, &beta_name)).await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn live_reconfiguration_rebuilds_only_the_changed_server() {
    let _guard = live_tests().lock().await;
    let first_starts = StartLog::new("changed");
    let sibling_starts = StartLog::new("sibling");
    let first = live_entry("first", &first_starts);
    let sibling = live_entry("sibling", &sibling_starts);
    let (kernel, fiber, manager) = live_world(json!({
        "servers": {
            "first": serde_json::to_value(&first).unwrap(),
            "sibling": serde_json::to_value(&sibling).unwrap()
        }
    }))
    .await;
    let root = kernel.root();
    wait_status(&manager, "first", "connected").await;
    wait_status(&manager, "sibling", "connected").await;
    let sibling_name = hoh_mcp_client::public_tool_name("sibling", "sibling");
    wait_until(|| has_tool(&root, &sibling_name)).await;
    assert_eq!(first_starts.starts(), 1);
    assert_eq!(sibling_starts.starts(), 1);

    let changed = live_entry("changed", &first_starts);
    fiber
        .commit_volatile(json!({
            "servers": {
                "first": serde_json::to_value(&changed).unwrap(),
                "sibling": serde_json::to_value(&sibling).unwrap()
            }
        }))
        .await
        .unwrap();
    wait_until(|| first_starts.starts() >= 2).await;
    wait_status(&manager, "first", "connected").await;
    assert_eq!(sibling_starts.starts(), 1);
    assert!(has_tool(&root, &sibling_name));
    kernel.shutdown().await;
}

#[tokio::test]
async fn live_registered_server_follows_registration_and_manager_lifetimes() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("registered");
    let (kernel, fiber, manager) = live_world(json!({ "servers": {} })).await;
    let root = kernel.root();
    let tool_name = hoh_mcp_client::public_tool_name("registered", "registered");
    let registration = manager
        .register(&root, "registered", live_entry("registered", &starts))
        .unwrap();
    wait_status(&manager, "registered", "connected").await;
    wait_until(|| has_tool(&root, &tool_name)).await;

    manager.restart("registered").await.unwrap();
    wait_until(|| starts.starts() >= 2).await;
    wait_status(&manager, "registered", "connected").await;
    let probe = manager
        .probe("registered", McpProbeOptions::default())
        .await
        .unwrap();
    assert!(matches!(probe.outcome, McpProbeOutcome::Success { .. }));

    registration.dispose().await;
    wait_until(|| manager.list().is_empty() && !has_tool(&root, &tool_name)).await;

    let late_registration = manager
        .register(&root, "registered", live_entry("registered", &starts))
        .unwrap();
    wait_status(&manager, "registered", "connected").await;
    fiber.dispose().await;
    wait_until(|| !has_tool(&root, &tool_name)).await;
    tokio::time::timeout(Duration::from_secs(5), late_registration.dispose())
        .await
        .expect("registration disposal after manager stop timed out");
    assert_eq!(
        manager.restart("registered").await,
        Err(McpManagerError::Disposed)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn live_credentials_rotate_without_expanding_manager_config_and_recover_after_unset() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("credentials");
    let credentials = Arc::new(TestCredentials::new(Some("alpha")));
    let mut entry = live_entry("unused", &starts);
    entry
        .env
        .insert(HELPER_TOOL_ENV.into(), "${credential:TOOL}".into());
    let raw_entry = serde_json::to_value(&entry).unwrap();
    let provider: Arc<dyn CredentialProvider> = credentials.clone();
    let (kernel, fiber, manager) = world_with_credentials(
        json!({ "servers": { "live": raw_entry.clone() } }),
        Some(provider),
    )
    .await;
    let root = kernel.root();
    let reference = credential_ref("TOOL").unwrap();
    let alpha = hoh_mcp_client::public_tool_name("live", "alpha");
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &alpha)).await;
    assert_eq!(
        fiber.raw_config().as_ref(),
        &json!({ "servers": { "live": raw_entry } })
    );
    let initial_starts = starts.starts();
    let initial_resolves = credentials.resolves();

    credentials.notify_updated(&root, &reference);
    wait_until(|| credentials.resolves() > initial_resolves).await;
    assert_eq!(starts.starts(), initial_starts);
    assert!(has_tool(&root, &alpha));

    credentials.replace(Some("beta"));
    credentials.notify_updated(&root, &reference);
    let beta = hoh_mcp_client::public_tool_name("live", "beta");
    wait_until(|| starts.starts() > initial_starts).await;
    wait_status(&manager, "live", "connected").await;
    wait_until(|| !has_tool(&root, &alpha) && has_tool(&root, &beta)).await;

    credentials.replace(None);
    credentials.notify_updated(&root, &reference);
    wait_status(&manager, "live", "waiting-credentials").await;
    wait_until(|| !has_tool(&root, &beta)).await;

    credentials.replace(Some("gamma"));
    credentials.notify_updated(&root, &reference);
    let gamma = hoh_mcp_client::public_tool_name("live", "gamma");
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &gamma)).await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn credential_template_without_provider_reports_the_client_diagnostic() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("missing-credentials");
    let mut entry = live_entry("unused", &starts);
    entry
        .env
        .insert(HELPER_TOOL_ENV.into(), "${credential:TOOL}".into());
    let (kernel, _, manager) = world(json!({
        "servers": { "live": serde_json::to_value(&entry).unwrap() }
    }))
    .await;
    wait_status(&manager, "live", "failed").await;
    let view = manager
        .list()
        .into_iter()
        .find(|view| view.name == "live")
        .unwrap();
    let McpManagedServerStatus::LoadFailed { last_error } = view.status else {
        panic!("expected load failure")
    };
    assert!(
        last_error.contains("MCP credential templates require a credentials provider"),
        "{last_error}"
    );
    assert_eq!(starts.starts(), 0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn aborting_a_real_probe_does_not_close_the_managed_connection() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("probe-abort");
    let mut entry = live_entry("managed", &starts);
    entry
        .env
        .insert(HELPER_HANG_AFTER_FIRST_ENV.into(), "1".into());
    let (kernel, _, manager) = live_world(json!({
        "servers": { "live": serde_json::to_value(&entry).unwrap() }
    }))
    .await;
    let root = kernel.root();
    let tool = hoh_mcp_client::public_tool_name("live", "managed");
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &tool)).await;

    let controller = hoh_timeout::AbortController::new();
    let options = McpProbeOptions {
        signal: Some(controller.signal()),
        ..McpProbeOptions::default()
    };
    let probing_manager = manager.clone();
    let probing = tokio::spawn(async move { probing_manager.probe("live", options).await });
    wait_until(|| starts.starts() >= 2).await;
    controller.abort_with("stop probe");
    let probe = probing.await.unwrap().unwrap();
    assert!(matches!(
        probe.outcome,
        McpProbeOutcome::Failure { ref message, .. } if message == "probe aborted"
    ));
    wait_status(&manager, "live", "connected").await;
    assert!(has_tool(&root, &tool));
    kernel.shutdown().await;
}

#[tokio::test]
async fn real_delayed_start_cannot_publish_after_reconfiguration_or_disposal() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("stale-start");
    let mut delayed = live_entry("stale", &starts);
    delayed.env.insert(HELPER_DELAY_MS_ENV.into(), "500".into());
    let (kernel, fiber, manager) = live_world(json!({
        "servers": { "live": serde_json::to_value(&delayed).unwrap() }
    }))
    .await;
    let root = kernel.root();
    wait_until(|| starts.starts() >= 1).await;
    let current = live_entry("current", &starts);
    fiber
        .commit_volatile(json!({
            "servers": { "live": serde_json::to_value(&current).unwrap() }
        }))
        .await
        .unwrap();
    let stale_tool = hoh_mcp_client::public_tool_name("live", "stale");
    let current_tool = hoh_mcp_client::public_tool_name("live", "current");
    wait_status(&manager, "live", "connected").await;
    wait_until(|| !has_tool(&root, &stale_tool) && has_tool(&root, &current_tool)).await;
    kernel.shutdown().await;

    let dispose_starts = StartLog::new("stale-dispose");
    let mut delayed = live_entry("late", &dispose_starts);
    delayed.env.insert(HELPER_DELAY_MS_ENV.into(), "500".into());
    let (kernel, fiber, manager) = live_world(json!({
        "servers": { "late": serde_json::to_value(&delayed).unwrap() }
    }))
    .await;
    let root = kernel.root();
    wait_until(|| dispose_starts.starts() >= 1).await;
    fiber.dispose().await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!has_tool(
        &root,
        &hoh_mcp_client::public_tool_name("late", "late")
    ));
    assert_eq!(
        manager.restart("late").await,
        Err(McpManagerError::Disposed)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn status_listener_failure_does_not_skip_later_listeners_or_lifecycle() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("listener");
    let entry = live_entry("listener", &starts);
    let (kernel, _, manager) = live_world(json!({
        "servers": { "live": serde_json::to_value(&entry).unwrap() }
    }))
    .await;
    wait_status(&manager, "live", "connected").await;
    let root = kernel.root();
    root.on_parallel::<McpManagerStatus, _>(|_| async {
        Err(Box::<dyn std::error::Error + Send + Sync>::from(
            "listener failed",
        ))
    })
    .unwrap();
    let later = Arc::new(AtomicU64::new(0));
    let observed = later.clone();
    root.on_parallel::<McpManagerStatus, _>(move |_| {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    })
    .unwrap();
    manager.restart("live").await.unwrap();
    wait_until(|| later.load(Ordering::SeqCst) > 0).await;
    wait_status(&manager, "live", "connected").await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn real_client_server_name_collision_is_reported_without_disturbing_static_instance() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("collision");
    let entry = live_entry("static", &starts);
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.provide::<Credentials>(Arc::new(TestCredentials::new(None)))
        .await
        .unwrap();
    let static_client = root
        .plugin::<McpClient>(direct_client_config("shared", &entry))
        .unwrap();
    static_client.settled().await.unwrap();
    let static_tool = hoh_mcp_client::public_tool_name("shared", "static");
    wait_until(|| has_tool(&root, &static_tool)).await;

    let manager_fiber = root
        .plugin::<McpManager>(json!({
            "servers": { "shared": serde_json::to_value(&entry).unwrap() }
        }))
        .unwrap();
    manager_fiber.settled().await.unwrap();
    let manager = root.get::<McpManager>().unwrap();
    wait_status(&manager, "shared", "failed").await;
    let view = manager.list().into_iter().next().unwrap();
    let McpManagedServerStatus::LoadFailed { last_error } = view.status else {
        panic!("expected collision load failure")
    };
    assert!(
        last_error.contains("serverName \"shared\" is already in use"),
        "{last_error}"
    );
    assert!(has_tool(&root, &static_tool));
    kernel.shutdown().await;
}

#[tokio::test]
async fn removal_and_disposal_overtake_real_gated_credential_resolution() {
    let _guard = live_tests().lock().await;
    let executable = fixture_executable().to_string_lossy().into_owned();

    let starts = StartLog::new("credential-remove");
    let credentials = Arc::new(TestCredentials::new(Some(&executable)));
    let (entered, release) = credentials.gate_next();
    let mut entry = live_entry("removed", &starts);
    entry.command = "${credential:CMD}".into();
    let provider: Arc<dyn CredentialProvider> = credentials.clone();
    let (kernel, fiber, manager) = world_with_credentials(
        json!({ "servers": { "live": serde_json::to_value(&entry).unwrap() } }),
        Some(provider),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    fiber
        .commit_volatile(json!({ "servers": {} }))
        .await
        .unwrap();
    release.notify_one();
    wait_until(|| manager.list().is_empty()).await;
    kernel.shutdown().await;
    assert_eq!(starts.starts(), 0);

    let starts = StartLog::new("credential-dispose");
    let credentials = Arc::new(TestCredentials::new(Some(&executable)));
    let (entered, release) = credentials.gate_next();
    let mut entry = live_entry("disposed", &starts);
    entry.command = "${credential:CMD}".into();
    let provider: Arc<dyn CredentialProvider> = credentials.clone();
    let (kernel, fiber, manager) = world_with_credentials(
        json!({ "servers": { "live": serde_json::to_value(&entry).unwrap() } }),
        Some(provider),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let disposing = tokio::spawn(async move { fiber.dispose().await });
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), disposing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(starts.starts(), 0);
    assert_eq!(
        manager.restart("live").await,
        Err(McpManagerError::Disposed)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn credential_refresh_rechecks_a_queued_update_and_removal_wins_a_late_rebuild() {
    let _guard = live_tests().lock().await;
    let starts = StartLog::new("credential-queue");
    let credentials = Arc::new(TestCredentials::new(Some("alpha")));
    let mut entry = live_entry("unused", &starts);
    entry
        .env
        .insert(HELPER_TOOL_ENV.into(), "${credential:TOOL}".into());
    let provider: Arc<dyn CredentialProvider> = credentials.clone();
    let (kernel, fiber, manager) = world_with_credentials(
        json!({ "servers": { "live": serde_json::to_value(&entry).unwrap() } }),
        Some(provider),
    )
    .await;
    let root = kernel.root();
    let reference = credential_ref("TOOL").unwrap();
    let alpha = hoh_mcp_client::public_tool_name("live", "alpha");
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &alpha)).await;

    let (entered, release) = credentials.gate_next();
    credentials.fail_next();
    credentials.notify_updated(&root, &reference);
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    credentials.replace(Some("gamma"));
    credentials.notify_updated(&root, &reference);
    release.notify_one();
    let gamma = hoh_mcp_client::public_tool_name("live", "gamma");
    wait_status(&manager, "live", "connected").await;
    wait_until(|| has_tool(&root, &gamma)).await;
    assert!(
        root.logger()
            .buffer()
            .iter()
            .all(|record| !record.message.contains("gamma"))
    );

    let starts_before_removal = starts.starts();
    let (entered, release) = credentials.gate_next();
    credentials.replace(Some("late-secret"));
    credentials.notify_updated(&root, &reference);
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    fiber
        .commit_volatile(json!({ "servers": {} }))
        .await
        .unwrap();
    release.notify_one();
    wait_until(|| manager.list().is_empty()).await;
    wait_until(|| !has_tool(&root, &gamma)).await;
    kernel.shutdown().await;
    assert_eq!(starts.starts(), starts_before_removal);
}

#[test]
fn managed_status_json_matches_the_remote_contract() {
    let status = McpManagedServerStatus::LoadFailed {
        last_error: "boom".into(),
    };
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(
        value,
        json!({ "state": "failed", "reason": "load", "lastError": "boom" })
    );
    assert_eq!(
        serde_json::from_value::<McpManagedServerStatus>(value).unwrap(),
        status
    );
}

#[test]
fn cleanup_status_json_round_trips() {
    let status = McpManagedServerStatus::CleanupFailed {
        result: hoh_mcp_client::McpCloseResult {
            close_confirmed: false,
            cleanup_error: Some("late".into()),
        },
    };
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(
        serde_json::from_value::<McpManagedServerStatus>(value).unwrap(),
        status
    );
}
