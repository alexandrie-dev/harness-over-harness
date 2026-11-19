//! Port of `tests/harness-dev-context.spec.ts`.

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, AssembleAgent, CancelOptions, Inbox, InboxTarget,
    MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_harness_dev_context::{HarnessDevContext, WebApp, WebAppApi};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{MessageId, UserMessage, create_system_message};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionId, SessionStore, Sessions,
    SurfaceIntent,
};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use hoh_shell_env::{InstallAnchor, InstallAnchorKey};
use hoh_system_prompt::{
    AssembleContext, PromptAssembly, SystemPrompt, SystemPromptKey, render_prompt,
};
use serde_json::json;
use std::sync::{Arc, Mutex, MutexGuard};

const SOURCE_PROMPT: &str = "The runtime context names the DeepSeek Harness implementation checkout. The checkout location and current working directory are separate values and may differ; never infer the working directory from the checkout path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend DSH itself.";
const WEB_PROMPT: &str = "You are interacting with the user through the DeepSeek Harness Web GUI whose URL the runtime context names. When the user refers to \"this page\", \"this GUI\", or \"this app\" without naming another target, they mean this GUI. The browser provides no implicit DOM, route, or screenshot context. The client-plugin HMR receiver is active, but client-plugin changes reload without a refresh only while `pnpm run dev:web` is also running from this same checkout to rebuild their bundles; verify that watcher before promising automatic updates. Every other change — the apps/web shell and plain packages — requires rebuilding the affected Web artifacts and verifying this existing URL after a page refresh. Starting another server does not update this GUI. The apps/web Vite entry builds the shell but is not a standalone application because only dsh web injects window.__DSH_BOOT__. Do not start a replacement server unless the user asks; if one is needed, use a managed background job and verify its exact URL.";
const LEGACY_SOURCE: &str = "The DeepSeek Harness implementation checkout is at /old/checkout/. The checkout location and current working directory are separate values and may differ; never infer the working directory from this path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend DSH itself.";
const LEGACY_WEB: &str = "You are interacting with the user through the DeepSeek Harness Web GUI at http://127.0.0.1:3100. When the user refers to \"this page\", \"this GUI\", or \"this app\" without naming another target, they mean this GUI. The browser provides no implicit DOM, route, or screenshot context. The client-plugin HMR receiver is active, but client-plugin changes reload without a refresh only while `pnpm run dev:web` is also running from this same checkout to rebuild their bundles; verify that watcher before promising automatic updates. Every other change — the apps/web shell and plain packages — requires rebuilding the affected Web artifacts and verifying this existing URL after a page refresh. Starting another server does not update this GUI. The apps/web Vite entry builds the shell but is not a standalone application because only dsh web injects window.__DSH_BOOT__. Do not start a replacement server unless the user asks; if one is needed, use a managed background job and verify its exact URL.";

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct MutableWebApp(Mutex<String>);

impl MutableWebApp {
    fn set(&self, url: &str) {
        *lock(&self.0) = url.into();
    }
}

impl WebAppApi for MutableWebApp {
    fn get_advertised_url(&self) -> String {
        lock(&self.0).clone()
    }
}

struct UnusedModel;

impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("unused model".into())
    }
}

struct UnusedInbox;

impl Inbox for UnusedInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        Vec::new()
    }

    fn next_step(&self) -> Vec<UserMessage> {
        Vec::new()
    }

    fn clear(&self) -> Result<(), BoxError> {
        Ok(())
    }

    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
        Ok(false)
    }

    fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
        Ok(false)
    }

    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Ok(Vec::new())
    }
}

struct TestAgent {
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
}

impl Agent for TestAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }

    fn scope_key(&self) -> ScopeKey {
        self.scope.clone()
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn model(&self) -> &dyn Model {
        &UnusedModel
    }

    fn inbox(&self) -> &dyn Inbox {
        &UnusedInbox
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
        Err("unused maintenance".into())
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

struct Fixture {
    kernel: Kernel,
    ctx: Context,
    store: Arc<SessionStore>,
    web: Arc<MutableWebApp>,
    fiber: Fiber,
}

impl Fixture {
    async fn new() -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let store = Arc::new(SessionStore::new());
        ctx.provide::<Sessions>(store.clone()).await.unwrap();
        let projections = Arc::new(SessionProjectionRegistry::new(&ctx).unwrap());
        ctx.provide::<SessionProjections>(projections)
            .await
            .unwrap();
        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.provide::<InstallAnchorKey>(Arc::new(InstallAnchor(
            "/test/install/package.json".into(),
        )))
        .await
        .unwrap();
        let web = Arc::new(MutableWebApp(Mutex::new("http://127.0.0.1:4567".into())));
        ctx.provide::<WebApp>(web.clone()).await.unwrap();
        let fiber = ctx.plugin::<HarnessDevContext>(json!({})).unwrap();
        fiber.settled().await.unwrap();
        Self {
            kernel,
            ctx,
            store,
            web,
            fiber,
        }
    }

    fn session(&self, id: &str) -> Session {
        self.store
            .create(&self.ctx, Some(id.into()), CreateSessionOptions::default())
            .unwrap()
    }

    fn resume(&self, source: &Session, id: &str) -> Session {
        self.store
            .create(
                &self.ctx,
                Some(id.into()),
                CreateSessionOptions {
                    seed: Some(source.snapshot_events(None, None).to_vec()),
                    ..CreateSessionOptions::default()
                },
            )
            .unwrap()
    }

    async fn assemble(&self, session: &Session) -> PromptAssembly {
        let agent: Arc<dyn Agent> = Arc::new(TestAgent {
            session: session.clone(),
            ctx: self.ctx.clone(),
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
        });
        self.ctx
            .get::<SystemPromptKey>()
            .unwrap()
            .assemble(AssembleContext::new(None).with_extension(AssembleAgent(agent)))
            .await
            .unwrap()
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn append_system(
    session: &Session,
    text: &str,
    intent: SurfaceIntent,
) -> Arc<hoh_session::SessionEvent> {
    session
        .append(
            "system/message",
            json!({"turn":1,"step":1,"message":create_system_message(text)}),
            Some(intent),
        )
        .unwrap()
}

fn legacy_session(fixture: &Fixture, id: &str, source: &str, web: &str) -> Session {
    let session = fixture.session(id);
    append_system(
        &session,
        &format!("{source}\n\n{web}"),
        SurfaceIntent::append(),
    );
    session
}

fn sections(assembly: &PromptAssembly) -> Vec<String> {
    assembly
        .sections
        .iter()
        .filter(|entry| entry.name == "harness:source" || entry.name == "app:web-surface")
        .map(|entry| entry.text.clone())
        .collect()
}

fn context<'a>(assembly: &'a PromptAssembly, name: &str) -> &'a str {
    &assembly
        .contexts
        .iter()
        .find(|entry| entry.name == name)
        .unwrap()
        .text
}

#[tokio::test]
async fn registers_value_free_sections_and_reports_checkout_and_web_gui_url_as_runtime_context() {
    let fixture = Fixture::new().await;
    let session = fixture.session("registrations");
    let assembly = fixture.assemble(&session).await;
    assert_eq!(sections(&assembly), [SOURCE_PROMPT, WEB_PROMPT]);
    assert!(!render_prompt(&assembly).unwrap().contains("127.0.0.1"));
    let source = context(&assembly, "harness:source");
    assert_eq!(
        source,
        "Current DeepSeek Harness implementation checkout: \"/test/install/\"."
    );
    assert_eq!(
        context(&assembly, "app:web-surface"),
        "Current DeepSeek Harness Web GUI URL: \"http://127.0.0.1:4567\"."
    );

    fixture.fiber.dispose().await;
    let disposed = fixture.assemble(&session).await;
    assert!(sections(&disposed).is_empty());
    assert!(disposed.contexts.is_empty());
    fixture.close().await;
}

async fn assert_legacy_resume(route: &str) {
    let fixture = Fixture::new().await;
    let original = legacy_session(&fixture, "earlier", LEGACY_SOURCE, LEGACY_WEB);
    let logged = original.snapshot_events(None, None);
    let mut session = original;
    for port in [4568, 4569] {
        fixture.web.set(&format!("http://127.0.0.1:{port}"));
        let resumed = fixture.resume(&session, &format!("{route}-{port}"));
        let before = resumed.snapshot_events(None, None);
        let assembly = fixture.assemble(&resumed).await;
        assert_eq!(sections(&assembly), [LEGACY_SOURCE, LEGACY_WEB]);
        assert_eq!(
            resumed.snapshot_events(None, None).as_ref(),
            before.as_ref()
        );
        assert_eq!(&before[..logged.len()], logged.as_ref());
        assert_eq!(before.len(), logged.len() + 1);
        assert_eq!(before.last().unwrap().event_type(), "session/end-seed");
        assert!(
            context(&assembly, "harness:source")
                .starts_with("Current DeepSeek Harness implementation checkout: \"")
        );
        assert_eq!(
            context(&assembly, "app:web-surface"),
            format!("Current DeepSeek Harness Web GUI URL: \"http://127.0.0.1:{port}\".")
        );
        session = resumed;
    }
    fixture.close().await;
}

#[tokio::test]
async fn resumes_earlier_format_session_on_other_ports_without_rewriting_replace_only_prompt() {
    assert_legacy_resume("replace-only").await;
}

#[tokio::test]
async fn resumes_earlier_format_session_on_other_ports_without_rewriting_in_history_prompt() {
    assert_legacy_resume("in-history").await;
}

#[tokio::test]
async fn keeps_earlier_paragraphs_after_series_consolidation_empties_later_prompt_version() {
    let fixture = Fixture::new().await;
    let session = legacy_session(&fixture, "consolidated-source", LEGACY_SOURCE, LEGACY_WEB);
    let first = session.snapshot_events(None, None)[0].clone();
    let updated = append_system(
        &session,
        &format!("{LEGACY_SOURCE}\n\n{LEGACY_WEB}\n\nUpdated instruction."),
        SurfaceIntent::replace(first.seq(), first.seq(), vec![first.seq()]),
    );
    append_system(
        &session,
        "",
        SurfaceIntent::replace(updated.seq(), updated.seq(), vec![updated.seq()]),
    );
    let resumed = fixture.resume(&session, "consolidated-resumed");
    assert_eq!(
        sections(&fixture.assemble(&resumed).await),
        [LEGACY_SOURCE, LEGACY_WEB]
    );
    fixture.close().await;
}

async fn assert_resembling_source_is_replaced(source: &str, id: &str) {
    let fixture = Fixture::new().await;
    let original = legacy_session(&fixture, "resembling-source", source, WEB_PROMPT);
    let resumed = fixture.resume(&original, id);
    let current = fixture.assemble(&resumed).await;
    assert_eq!(sections(&current), [SOURCE_PROMPT, WEB_PROMPT]);
    assert!(!render_prompt(&current).unwrap().contains("/old/"));
    fixture.close().await;
}

#[tokio::test]
async fn replaces_a_paragraph_without_the_fixed_ending() {
    assert_resembling_source_is_replaced(
        "The DeepSeek Harness implementation checkout is at /old/checkout/.",
        "resembling-ending",
    )
    .await;
}

#[tokio::test]
async fn replaces_a_paragraph_broken_across_lines() {
    assert_resembling_source_is_replaced(
        &LEGACY_SOURCE.replace("/old/checkout/", "/old/\ncheckout/"),
        "resembling-newline",
    )
    .await;
}

#[tokio::test]
async fn keeps_a_new_session_system_prompt_free_of_process_values_across_ports() {
    let fixture = Fixture::new().await;
    let original = fixture.session("fresh");
    let first = fixture.assemble(&original).await;
    let logged = render_prompt(&first).unwrap();
    assert!(logged.contains(SOURCE_PROMPT));
    assert!(logged.contains(WEB_PROMPT));
    assert!(!logged.contains("127.0.0.1"));
    append_system(&original, &logged, SurfaceIntent::append());

    fixture.web.set("http://127.0.0.1:4568");
    let resumed = fixture.resume(&original, "fresh-on-4568");
    let second = fixture.assemble(&resumed).await;
    assert_eq!(render_prompt(&second).unwrap(), logged);
    assert_eq!(
        context(&second, "app:web-surface"),
        "Current DeepSeek Harness Web GUI URL: \"http://127.0.0.1:4568\"."
    );
    fixture.close().await;
}
