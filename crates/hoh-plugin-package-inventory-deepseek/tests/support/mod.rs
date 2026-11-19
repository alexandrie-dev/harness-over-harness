#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::AgentRegistryPlugin;
use hoh_agent::{
    Agent, AgentDetach, AgentOptions, AgentStatus, Agents, CancelOptions, Inbox, InboxTarget,
    MaintenanceFuture, MaintenanceTask, Model,
};
use hoh_deepseek_llm_api_extensions::{
    DeepSeekLlmApiExtensionRegistry, DeepSeekLlmApiExtensionRequest, DeepSeekLlmApiExtensions,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Fiber, Kernel, NoConfig, Plugin, Service, Undo};
use hoh_llm::UserMessage;
use hoh_loader::{BootOptions, EntryInfo, EvalEnv, LoaderKey, PluginRegistry, boot};
use hoh_plugin_package_inventory_deepseek::{
    InventoryRuntime, InventoryRuntimeKey, PluginPackageInventoryDeepSeek, PluginPackages,
    PluginPackagesKey,
};
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope};
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_timeout::AbortController;
use serde_json::{Map, Value, json};
use std::{path::Path, sync::Arc};
use tempfile::TempDir;
use url::Url;

pub const FIELD: &str = "dsh_plugin_packages";

pub struct Noop;

impl Plugin for Noop {
    const NAME: &'static str = "inventory-test-noop";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        ctx.effect("inventory-test-noop", || Undo::sync(|| {}))?;
        Ok(())
    }
}

pub fn file_url(path: &Path) -> String {
    Url::from_file_path(path).unwrap().into()
}

pub fn package_plugin(
    root: &Path,
    relative: &str,
    manifest: Value,
    registry: &mut PluginRegistry,
) -> String {
    let directory = root.join(relative);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(directory.join("plugin.mjs"), "export default () => {}\n").unwrap();
    let module = format!("./{relative}/plugin.mjs");
    registry.add::<Noop>(&module);
    module
}

fn eval_env(root: &Path) -> EvalEnv {
    EvalEnv {
        home: root.into(),
        cwd: root.into(),
        exe: root.join("hoh"),
        platform: "linux",
        var: Arc::new(|_| None),
    }
}

pub fn request(session_id: Option<&str>) -> DeepSeekLlmApiExtensionRequest {
    DeepSeekLlmApiExtensionRequest {
        body: Map::new(),
        session_id: session_id.map(str::to_owned),
        purpose: None,
        signal: AbortController::new().signal(),
    }
}

pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub root: TempDir,
    pub inventory: Fiber,
    pub extensions: Service<DeepSeekLlmApiExtensions>,
    pub loader: Service<LoaderKey>,
}

struct BareAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope_key: ScopeKey,
    options: AgentOptions,
}

impl Agent for BareAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn scope_key(&self) -> ScopeKey {
        self.scope_key.clone()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn model(&self) -> &dyn Model {
        panic!("model is not used by inventory tests")
    }
    fn inbox(&self) -> &dyn Inbox {
        panic!("inbox is not used by inventory tests")
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
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("maintenance is not used by inventory tests")
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("send is not used by inventory tests")
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("followup is not used by inventory tests")
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("steer is not used by inventory tests")
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("inject is not used by inventory tests")
    }
}

impl Harness {
    pub async fn start(
        enabled: Option<bool>,
        setup: impl FnOnce(&Path, &mut PluginRegistry) -> String,
    ) -> Self {
        Self::start_with_packages(enabled, setup, None, true).await
    }

    pub async fn start_with_packages(
        enabled: Option<bool>,
        setup: impl FnOnce(&Path, &mut PluginRegistry) -> String,
        packages: Option<PluginPackages>,
        publish_runtime: bool,
    ) -> Self {
        let entry_base =
            publish_runtime.then(|| Arc::new(|_: &EntryInfo| None) as Arc<EntryBaseResolver>);
        Self::start_custom(enabled, setup, packages, entry_base).await
    }

    pub async fn start_with_entry_bases(
        enabled: Option<bool>,
        setup: impl FnOnce(&Path, &mut PluginRegistry) -> String,
        entry_base: Arc<EntryBaseResolver>,
    ) -> Self {
        Self::start_custom(enabled, setup, None, Some(entry_base)).await
    }

    async fn start_custom(
        enabled: Option<bool>,
        setup: impl FnOnce(&Path, &mut PluginRegistry) -> String,
        packages: Option<PluginPackages>,
        entry_base: Option<Arc<EntryBaseResolver>>,
    ) -> Self {
        let root = TempDir::new().unwrap();
        let mut registry = PluginRegistry::default();
        let composition_text = setup(root.path(), &mut registry);
        let composition = root.path().join("cordis.yml");
        std::fs::write(&composition, composition_text).unwrap();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let loader = boot(
            &ctx,
            BootOptions {
                registry: Arc::new(registry),
                base: composition.clone(),
                layers: vec![],
                env: eval_env(root.path()),
            },
        )
        .await
        .unwrap();
        if let Some(entry_base) = entry_base {
            ctx.provide::<InventoryRuntimeKey>(Arc::new(InventoryRuntime::new(
                file_url(&composition),
                move |entry| entry_base(entry),
            )))
            .await
            .unwrap();
        }
        if let Some(packages) = packages {
            ctx.provide::<PluginPackagesKey>(Arc::new(packages))
                .await
                .unwrap();
        }
        for fiber in [
            ctx.plugin::<SessionProjectionRegistry>(json!(null))
                .unwrap(),
            ctx.plugin::<AgentRegistryPlugin>(json!(null)).unwrap(),
            ctx.plugin::<DeepSeekLlmApiExtensionRegistry>(json!(null))
                .unwrap(),
        ] {
            fiber.settled().await.unwrap();
        }
        let config = enabled.map_or_else(|| json!({}), |enabled| json!({"enabled":enabled}));
        let inventory = ctx
            .plugin::<PluginPackageInventoryDeepSeek>(config)
            .unwrap();
        inventory.settled().await.unwrap();
        let extensions = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
        Self {
            kernel,
            ctx,
            root,
            inventory,
            extensions,
            loader,
        }
    }

    pub async fn fields(&self, session_id: Option<&str>) -> Map<String, Value> {
        self.extensions
            .prepare(request(session_id))
            .await
            .unwrap()
            .fields()
            .clone()
    }

    pub fn enter_agent(&self, id: &str) -> (Scope, Arc<dyn Agent>, AgentDetach) {
        let scope_key = ScopeKey::new();
        let scope =
            create_scope(&self.ctx, scope_key.clone(), CreateScopeOptions::default()).unwrap();
        let session_id = SessionId::from(id);
        let session = Session::create(session_id.clone(), None, None, None, None).unwrap();
        let agent: Arc<dyn Agent> = Arc::new(BareAgent {
            id: session_id,
            session,
            ctx: scope.ctx.clone(),
            scope_key,
            options: AgentOptions::default(),
        });
        let detach = self
            .ctx
            .get::<Agents>()
            .unwrap()
            .enter(&self.ctx, agent.clone())
            .unwrap();
        (scope, agent, detach)
    }

    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}

pub type EntryBaseResolver = dyn Fn(&EntryInfo) -> Option<String> + Send + Sync;
