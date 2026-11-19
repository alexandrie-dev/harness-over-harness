use std::sync::{Arc, Mutex, MutexGuard};

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_file_reference::{FileReferenceCandidate, FileReferenceKind, FileReferenceService};
use hoh_kernel::Context;
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_timeout::{AbortController, AbortSignal};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

struct StubAgent {
    id: SessionId,
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn scope_key(&self) -> ScopeKey {
        panic!("the file-reference service test does not inspect the scope")
    }

    fn options(&self) -> &AgentOptions {
        panic!("the file-reference service test does not inspect agent options")
    }

    fn session(&self) -> &Session {
        panic!("the file-reference service test does not inspect the session")
    }

    fn model(&self) -> &dyn Model {
        panic!("the file-reference service test does not inspect the model")
    }

    fn inbox(&self) -> &dyn Inbox {
        panic!("the file-reference service test does not inspect the inbox")
    }

    fn status(&self) -> AgentStatus {
        panic!("the file-reference service test does not inspect agent status")
    }

    fn ctx(&self) -> &Context {
        panic!("the file-reference service test does not inspect the context")
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        panic!("the file-reference service test does not cancel the agent")
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        panic!("the file-reference service test does not await idle")
    }

    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("the file-reference service test does not run maintenance")
    }

    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("the file-reference service test does not send input")
    }

    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the file-reference service test does not queue follow-up input")
    }

    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the file-reference service test does not steer")
    }

    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the file-reference service test does not inject context")
    }
}

struct Call {
    agent: usize,
    query: String,
    signal: AbortSignal,
}

struct StubProvider {
    candidates: Mutex<Option<Vec<FileReferenceCandidate>>>,
    call: Mutex<Option<Call>>,
}

impl FileReferenceService for StubProvider {
    fn list<'a>(
        &'a self,
        agent: &'a Arc<dyn Agent>,
        query: &'a str,
        signal: &'a AbortSignal,
    ) -> BoxFuture<'a, Result<Vec<FileReferenceCandidate>, BoxError>> {
        *lock(&self.call) = Some(Call {
            agent: Arc::as_ptr(agent) as *const () as usize,
            query: query.to_owned(),
            signal: signal.clone(),
        });
        let candidates = lock(&self.candidates)
            .take()
            .expect("the stub is called exactly once");
        Box::pin(async move { Ok(candidates) })
    }
}

#[tokio::test]
async fn registers_a_provider_implementation_without_wrapping_its_discovery_member() {
    let candidates = vec![FileReferenceCandidate {
        path: "src".into(),
        kind: FileReferenceKind::Directory,
    }];
    let allocation = candidates.as_ptr();
    let provider = StubProvider {
        candidates: Mutex::new(Some(candidates)),
        call: Mutex::new(None),
    };
    let agent: Arc<dyn Agent> = Arc::new(StubAgent {
        id: SessionId::from("target"),
    });
    let agent_identity = Arc::as_ptr(&agent) as *const () as usize;
    let controller = AbortController::new();
    let signal = controller.signal();

    let result = provider.list(&agent, "sr", &signal).await.unwrap();

    assert_eq!(result.as_ptr(), allocation);
    assert_eq!(result.len(), 1);
    let call = lock(&provider.call).take().unwrap();
    assert_eq!(call.agent, agent_identity);
    assert_eq!(call.query, "sr");
    controller.abort();
    let expected_reason = signal.reason().unwrap();
    assert!(call.signal.reason().unwrap().ptr_eq(&expected_reason));
}
