use super::*;
use hoh_kernel::{Fiber, Kernel, Realm};
use hoh_loader::{
    BootOptions, Bundle, BundleSource, EvalEnv, LoadError, PluginRegistry, boot,
    config_editor::ConfigEditor,
};
use hoh_settings::{
    Settings, SettingsApi, SettingsDescribeOptions, SettingsDescriptor, SettingsFuture,
    SettingsPathOp, SettingsPresentation, SettingsUndo,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::sync::Notify;

fn model(provider: &str, model: &str, reasoning_effort: Option<&str>) -> ModelSelection {
    ModelSelection {
        provider: provider.into(),
        model: model.into(),
        reasoning_effort: reasoning_effort.map(Into::into),
    }
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[tokio::test]
async fn reads_complete_selections_from_volatile_config_and_clears_omitted_reasoning_effort() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<AgentDefaultModelConfig>(json!({"provider":"p","model":"m"}))
        .expect("default-model plugin should be accepted");
    fiber
        .settled()
        .await
        .expect("default-model plugin should load");
    let consumer = ctx
        .get::<AgentDefaultModel>()
        .expect("default-model service should be available");

    assert!(
        fiber
            .commit_volatile(json!({"provider":"q","model":"n","reasoningEffort":"high"}))
            .await
            .expect("volatile update should succeed")
    );
    assert_eq!(consumer.current_selection(), model("q", "n", Some("high")));

    assert!(
        fiber
            .commit_volatile(json!({"provider":"p","model":"m"}))
            .await
            .expect("volatile replacement should succeed")
    );
    assert_eq!(consumer.current_selection(), model("p", "m", None));
    consumer
        .save_selection(&ctx, model("unsaved", "unsaved", None))
        .await
        .expect("saving without an owning entry should be a no-op");
    assert_eq!(consumer.current_selection(), model("p", "m", None));

    let definition = hoh_kernel::PluginDef::of::<AgentDefaultModelConfig>();
    let schema = definition.schema();
    assert_eq!(schema["properties"]["provider"]["x-volatile"], true);
    assert_eq!(schema["properties"]["model"]["x-volatile"], true);
    assert_eq!(schema["properties"]["reasoningEffort"]["x-volatile"], true);
    assert_eq!(schema["required"], json!(["provider", "model"]));

    kernel.shutdown().await;
}

struct FixtureBundles;

impl BundleSource for FixtureBundles {
    fn bundle(&self, name: &str) -> Result<Bundle, LoadError> {
        if name != "default-model" {
            return Err(LoadError::Runtime(format!(
                "unexpected bundle request {name}"
            )));
        }
        Ok(Bundle {
            name: name.into(),
            dir: Some(PathBuf::from("/bundles/default-model")),
            platforms: None,
            files: vec![(
                "cordis.patch.yml".into(),
                "- insert: [{id: default-model, name: default-model, config: {provider: test, model: original}}]\n".into(),
            )],
        })
    }
}

fn environment(root: &Path) -> EvalEnv {
    EvalEnv {
        home: root.join("home"),
        cwd: root.into(),
        exe: root.join("hoh"),
        platform: "linux",
        var: Arc::new(|_| None),
    }
}

fn profile(root: &Path, name: &str) -> PathBuf {
    let dir = root.join("profiles").join(name);
    std::fs::create_dir_all(&dir).expect("profile directory should be created");
    std::fs::write(
        dir.join("package.json"),
        json!({"dsh":{"profile":{"bundles":["default-model"]}}}).to_string(),
    )
    .expect("profile manifest should be written");
    std::fs::write(dir.join("cordis.patch.yml"), "[]\n").expect("profile patch should be written");
    dir
}

struct LoaderFixture {
    _temp: TempDir,
    kernel: Kernel,
    ctx: Context,
    root_profile: PathBuf,
}

impl LoaderFixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary directory should be created");
        let base = temp.path().join("cordis.yml");
        std::fs::write(&base, "[]\n").expect("base composition should be written");
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let mut registry = PluginRegistry::default();
        registry.add::<AgentDefaultModelConfig>("default-model");
        let loader = boot(
            &ctx,
            BootOptions {
                registry: Arc::new(registry),
                base,
                layers: vec![],
                env: environment(temp.path()),
            },
        )
        .await
        .expect("loader should boot");
        let root_profile = profile(temp.path(), "root");
        let bundles: Arc<dyn BundleSource> = Arc::new(FixtureBundles);
        let loaded = hoh_loader::Profile::load(
            &root_profile,
            bundles.as_ref(),
            &temp.path().join("home"),
            &[],
            &environment(temp.path()),
        )
        .expect("profile should load");
        loader
            .recompose(loaded.layers)
            .await
            .expect("profile should compose");
        ConfigEditor::mount(
            &ctx,
            root_profile.clone(),
            bundles,
            temp.path().join("home"),
            vec![],
            environment(temp.path()),
        )
        .await
        .expect("root config editor should mount");
        Self {
            _temp: temp,
            kernel,
            ctx,
            root_profile,
        }
    }

    fn service(&self) -> Service<AgentDefaultModel> {
        self.ctx
            .get::<AgentDefaultModel>()
            .expect("default-model service should be active")
    }
}

#[tokio::test]
async fn persists_complete_selections_through_its_owning_profile_entry() {
    let fixture = LoaderFixture::new().await;
    let service = fixture.service();

    service
        .save_selection(&fixture.ctx, model("test", "next", Some("high")))
        .await
        .expect("complete selection should be saved");
    assert_eq!(
        service.current_selection(),
        model("test", "next", Some("high"))
    );

    service
        .save_selection(&fixture.ctx, model("test", "final", None))
        .await
        .expect("selection without reasoning effort should be saved");
    assert_eq!(service.current_selection(), model("test", "final", None));
    let patch = std::fs::read_to_string(fixture.root_profile.join("cordis.patch.yml"))
        .expect("profile patch should be readable");
    assert!(patch.contains("provider: test"));
    assert!(patch.contains("model: final"));
    assert!(!patch.contains("reasoningEffort"));

    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn standalone_plugin_with_a_visible_editor_keeps_its_own_selection() {
    let fixture = LoaderFixture::new().await;
    let standalone_parent = fixture.ctx.isolate("agentDefaultModel", Realm::new());
    let standalone = standalone_parent
        .plugin::<AgentDefaultModelConfig>(json!({"provider":"test","model":"original"}))
        .expect("standalone default-model plugin should be accepted");
    standalone
        .settled()
        .await
        .expect("standalone default-model plugin should load");
    let service = standalone_parent
        .get::<AgentDefaultModel>()
        .expect("standalone service should be visible from its own context");

    service
        .save_selection(&fixture.ctx, model("test", "ignored", None))
        .await
        .expect("an entryless plugin should ignore a visible editor");
    assert_eq!(service.current_selection(), model("test", "original", None));
    assert_eq!(
        std::fs::read_to_string(fixture.root_profile.join("cordis.patch.yml"))
            .expect("profile patch should be readable"),
        "[]\n"
    );

    standalone.dispose().await;
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn serializes_overlapping_saves_and_continues_after_a_rejected_write() {
    let fixture = LoaderFixture::new().await;
    let service = fixture.service();
    let blocker_entered = Arc::new(Notify::new());
    let blocker_release = Arc::new(Notify::new());
    let entered = blocker_entered.clone();
    let release = blocker_release.clone();
    let blocker = service.saves.enqueue(Box::pin(async move {
        entered.notify_one();
        release.notified().await;
        Ok(())
    }));
    blocker_entered.notified().await;

    let first = service.save_selection(&fixture.ctx, model("test", "rejected", None));
    let between_entered = Arc::new(Notify::new());
    let between_release = Arc::new(Notify::new());
    let entered = between_entered.clone();
    let release = between_release.clone();
    let between = service.saves.enqueue(Box::pin(async move {
        entered.notify_one();
        release.notified().await;
        Ok(())
    }));
    let mut last_selection = model("test", "final", None);
    let last = service.save_selection(&fixture.ctx, last_selection.clone());
    last_selection.model = "mutated".into();
    let last_waiter = tokio::spawn(last);

    std::fs::write(fixture.root_profile.join("cordis.patch.yml"), "- id: [\n")
        .expect("invalid patch fixture should be written");
    blocker_release.notify_waiters();
    blocker.await.expect("queue blocker should finish");
    let error = first.await.expect_err("the first save should fail");
    assert!(error.to_string().contains("cordis.patch.yml"));
    between_entered.notified().await;
    std::fs::write(fixture.root_profile.join("cordis.patch.yml"), "[]\n")
        .expect("valid patch fixture should be restored");
    between_release.notify_waiters();
    between.await.expect("between-save gate should finish");
    last_waiter
        .await
        .expect("last save waiter should not panic")
        .expect("a later save should still run");
    assert_eq!(service.current_selection(), model("test", "final", None));
    let patch = std::fs::read_to_string(fixture.root_profile.join("cordis.patch.yml"))
        .expect("profile patch should be readable");
    assert!(patch.contains("model: final"));
    assert!(!patch.contains("mutated"));

    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn queued_saves_outlive_plugin_disposal_without_blocking_it() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<AgentDefaultModelConfig>(json!({"provider":"p","model":"m"}))
        .expect("default-model plugin should be accepted");
    fiber
        .settled()
        .await
        .expect("default-model plugin should load");
    let service = ctx
        .get::<AgentDefaultModel>()
        .expect("default-model service should be available");
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first_entered = entered.clone();
    let first_release = release.clone();
    let first = service.saves.enqueue(Box::pin(async move {
        first_entered.notify_one();
        first_release.notified().await;
        Ok(())
    }));
    let completed = Arc::new(AtomicUsize::new(0));
    let second_completed = completed.clone();
    let second = service.saves.enqueue(Box::pin(async move {
        second_completed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    entered.notified().await;

    tokio::time::timeout(Duration::from_millis(250), fiber.dispose())
        .await
        .expect("plugin disposal must not wait for an active save");
    release.notify_waiters();
    first
        .await
        .expect("active save should finish after disposal");
    second.await.expect("queued save should run after disposal");
    assert_eq!(completed.load(Ordering::SeqCst), 1);

    drop(service);
    kernel.shutdown().await;
}

#[tokio::test]
async fn kernel_shutdown_closes_an_active_save() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let queue = SaveQueue::start(&ctx).expect("save queue should start");
    let entered = Arc::new(Notify::new());
    let wait = entered.clone();
    let pending = queue.enqueue(Box::pin(async move {
        wait.notify_one();
        std::future::pending::<Result<(), BoxError>>().await
    }));
    entered.notified().await;

    tokio::time::timeout(Duration::from_millis(250), kernel.shutdown())
        .await
        .expect("kernel shutdown must cancel an active save");
    let error = pending
        .await
        .expect_err("the active save should close with the kernel");
    assert!(matches!(
        error.downcast_ref::<KernelError>(),
        Some(KernelError::Closed)
    ));
}

#[tokio::test]
async fn resolves_the_configuration_editor_from_the_callers_service_realm() {
    let fixture = LoaderFixture::new().await;
    let isolated_profile = profile(fixture._temp.path(), "isolated");
    let caller = fixture.ctx.isolate("configEditor", Realm::new());
    ConfigEditor::mount(
        &caller,
        isolated_profile.clone(),
        Arc::new(FixtureBundles),
        fixture._temp.path().join("home"),
        vec![],
        environment(fixture._temp.path()),
    )
    .await
    .expect("isolated config editor should mount");

    fixture
        .service()
        .save_selection(&caller, model("isolated", "chosen", None))
        .await
        .expect("caller-visible editor should save the selection");
    let isolated = std::fs::read_to_string(isolated_profile.join("cordis.patch.yml"))
        .expect("isolated patch should be readable");
    let root = std::fs::read_to_string(fixture.root_profile.join("cordis.patch.yml"))
        .expect("root patch should be readable");
    assert!(isolated.contains("model: chosen"));
    assert_eq!(root, "[]\n");

    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_the_entry_when_the_caller_cannot_see_a_configuration_editor() {
    let fixture = LoaderFixture::new().await;
    let hidden = fixture.ctx.isolate("configEditor", Realm::new());
    fixture
        .service()
        .save_selection(&hidden, model("test", "ignored", None))
        .await
        .expect("a caller without an editor should get a no-op");
    assert_eq!(
        std::fs::read_to_string(fixture.root_profile.join("cordis.patch.yml"))
            .expect("profile patch should be readable"),
        "[]\n"
    );
    assert_eq!(
        fixture.service().current_selection(),
        model("test", "original", None)
    );
    fixture.kernel.shutdown().await;
}

struct FakeSettings {
    configured: Mutex<Option<oneshot::Sender<(SettingsPresentation, u32)>>>,
    released: Arc<AtomicUsize>,
}

impl SettingsApi for FakeSettings {
    fn configure(
        &self,
        presentation: SettingsPresentation,
        owner: Fiber,
    ) -> Result<SettingsUndo, BoxError> {
        if let Some(configured) = lock(&self.configured).take() {
            let _ = configured.send((presentation, owner.uid()));
        }
        let released = self.released.clone();
        Ok(Box::new(move || {
            released.fetch_add(1, Ordering::SeqCst);
        }))
    }

    fn writable(&self) -> bool {
        unreachable!("the default-model plugin only configures settings presentation")
    }

    fn document_path(&self) -> PathBuf {
        unreachable!("the default-model plugin only configures settings presentation")
    }

    fn prepare_document(&self) -> SettingsFuture<PathBuf> {
        unreachable!("the default-model plugin only configures settings presentation")
    }

    fn describe(&self, _: SettingsDescribeOptions) -> SettingsFuture<Vec<SettingsDescriptor>> {
        unreachable!("the default-model plugin only configures settings presentation")
    }

    fn update(&self, _: String, _: Value, _: Option<u64>) -> SettingsFuture<()> {
        unreachable!("the default-model plugin only configures settings presentation")
    }

    fn replace(&self, _: String, _: Value, _: Option<u64>) -> SettingsFuture<()> {
        unreachable!("the default-model plugin only configures settings presentation")
    }

    fn mutate(&self, _: String, _: Vec<SettingsPathOp>, _: Option<u64>) -> SettingsFuture<()> {
        unreachable!("the default-model plugin only configures settings presentation")
    }
}

#[tokio::test]
async fn suppresses_the_generated_settings_page_for_its_plugin_instance() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (configured, observed) = oneshot::channel();
    let released = Arc::new(AtomicUsize::new(0));
    let settings: Arc<dyn SettingsApi> = Arc::new(FakeSettings {
        configured: Mutex::new(Some(configured)),
        released: released.clone(),
    });
    ctx.provide::<Settings>(settings)
        .await
        .expect("settings service should be provided");
    let fiber = ctx
        .plugin::<AgentDefaultModelConfig>(json!({"provider":"p","model":"m"}))
        .expect("default-model plugin should be accepted");
    fiber
        .settled()
        .await
        .expect("default-model plugin should load");
    let (presentation, owner) = observed
        .await
        .expect("settings presentation should be configured");
    assert_eq!(presentation.auto, Some(false));
    assert_eq!(owner, fiber.uid());

    fiber.dispose().await;
    assert_eq!(released.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}
