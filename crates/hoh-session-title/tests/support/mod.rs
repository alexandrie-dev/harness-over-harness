#![allow(dead_code)]

use std::{future::Future, sync::Arc};

use futures_util::future::BoxFuture;
use hoh_agent::AgentRegistryPlugin;
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_session::{
    CreateSessionOptions, Session, SessionPlugin, SessionStore, Sessions, SurfaceIntent,
};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use hoh_session_title::{
    SessionTitle, SessionTitleAutomaticMode, SessionTitlePlugin, SessionTitleProvider,
    SessionTitleProviderId, SessionTitleProviderRequest, SessionTitleProviderResult,
    SessionTitleService,
};
use serde_json::json;

pub fn config() -> serde_json::Value {
    json!({
        "fallbackMaxWords": 5,
        "fallbackMaxBytes": 40,
        "maxTitleBytes": 80,
    })
}

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub sessions: SessionStore,
    pub projections: SessionProjectionRegistry,
    pub titles: SessionTitleService,
}

pub struct BaseEnv {
    pub kernel: Kernel,
    pub ctx: Context,
    pub sessions: SessionStore,
    pub projections: SessionProjectionRegistry,
}

impl BaseEnv {
    pub async fn mount_titles(&self, title_config: serde_json::Value) -> hoh_kernel::Fiber {
        let fiber = self.ctx.plugin::<SessionTitlePlugin>(title_config).unwrap();
        fiber.settled().await.unwrap();
        fiber
    }

    pub async fn shutdown(self) {
        self.kernel.shutdown().await;
    }
}

impl Env {
    pub async fn shutdown(self) {
        self.kernel.shutdown().await;
    }
}

pub async fn setup() -> Env {
    setup_with(config()).await
}

pub async fn setup_with(config: serde_json::Value) -> Env {
    let BaseEnv {
        kernel,
        ctx,
        sessions,
        projections,
    } = setup_base().await;
    ctx.plugin::<SessionTitlePlugin>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let titles = (*ctx.get::<SessionTitle>().unwrap()).clone();
    Env {
        kernel,
        ctx,
        sessions,
        projections,
        titles,
    }
}

pub async fn setup_base() -> BaseEnv {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    let projections = (*ctx.get::<SessionProjections>().unwrap()).clone();
    BaseEnv {
        kernel,
        ctx,
        sessions,
        projections,
    }
}

pub fn session(env: &Env, id: &str) -> Session {
    env.sessions
        .create(&env.ctx, Some(id.into()), CreateSessionOptions::default())
        .unwrap()
}

pub fn append_human(session: &Session, text: &str) -> hoh_session::SessionSeq {
    let message = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    });
    session
        .append(
            "user/message",
            serde_json::to_value(message).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap()
        .seq()
}

pub fn append_source(session: &Session, kind: &str, text: &str) -> hoh_session::SessionSeq {
    let message = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new(kind),
    });
    session
        .append(
            "user/message",
            serde_json::to_value(message).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap()
        .seq()
}

pub fn append_route(session: &Session, reason: &str) {
    session
        .append(
            "request/header",
            json!({
                "header": { "config": { "provider": "main-route", "model": "chat-model" } },
                "reason": reason,
            }),
            None,
        )
        .unwrap();
}

pub async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

pub struct TestProvider {
    id: SessionTitleProviderId,
    mode: SessionTitleAutomaticMode,
    generate: Arc<
        dyn Fn(
                SessionTitleProviderRequest,
            ) -> BoxFuture<'static, Result<SessionTitleProviderResult, BoxError>>
            + Send
            + Sync,
    >,
}

impl TestProvider {
    pub fn new<F, Fut>(id: &str, mode: SessionTitleAutomaticMode, generate: F) -> Arc<Self>
    where
        F: Fn(SessionTitleProviderRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<SessionTitleProviderResult, BoxError>> + Send + 'static,
    {
        Arc::new(Self {
            id: id.into(),
            mode,
            generate: Arc::new(move |request| Box::pin(generate(request))),
        })
    }
}

impl SessionTitleProvider for TestProvider {
    fn id(&self) -> &SessionTitleProviderId {
        &self.id
    }
    fn automatic(&self) -> SessionTitleAutomaticMode {
        self.mode
    }
    fn generate(
        &self,
        request: SessionTitleProviderRequest,
    ) -> BoxFuture<'static, Result<SessionTitleProviderResult, BoxError>> {
        (self.generate)(request)
    }
}

pub fn result(title: &str, seqs: Vec<hoh_session::SessionSeq>) -> SessionTitleProviderResult {
    SessionTitleProviderResult {
        title: title.into(),
        message_seqs: seqs,
        model: None,
    }
}
