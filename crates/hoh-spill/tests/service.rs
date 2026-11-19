use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::ToolCallId;
use hoh_session::SessionId;
use hoh_spill::{
    SaveTextSpill, SpillLocator, SpillOwner, SpillRef, SpillSource, SpillStore, SpillStoreApi,
    spill_locator,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Default)]
struct StubStore {
    last: Mutex<Option<SaveTextSpill>>,
}

impl SpillStoreApi for StubStore {
    fn save_text(
        &self,
        input: SaveTextSpill,
    ) -> Pin<Box<dyn Future<Output = Result<SpillRef, BoxError>> + Send + '_>> {
        let result = SpillRef {
            locator: spill_locator(format!("/stub/{}", input.suggested_name)),
            bytes: input.content.len() as u64,
            retrieval_hint: "Use the stub reader.".into(),
        };
        *lock(&self.last) = Some(input);
        Box::pin(async move { Ok(result) })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn request(content: &str) -> SaveTextSpill {
    SaveTextSpill {
        owner: SpillOwner {
            session_id: SessionId::from("s1"),
        },
        source: SpillSource::Tool {
            tool_name: "web_fetch".into(),
            call_id: ToolCallId::from("c1"),
            label: "result".into(),
        },
        suggested_name: "web_fetch.txt".into(),
        content: content.into(),
    }
}

fn mount(ctx: &Context, store: Arc<StubStore>) -> Fiber {
    ctx.inject(move |ctx, (): ()| {
        let store = store.clone();
        async move {
            ctx.provide::<SpillStore>(store).await?;
            Ok(())
        }
    })
    .expect("active root context accepts a child fiber")
}

#[tokio::test]
async fn registers_as_ctx_spill_store_and_saves_text() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let stub = Arc::new(StubStore::default());
    let fiber = mount(&ctx, stub.clone());
    fiber.settled().await.unwrap();

    let spill_store = ctx.get::<SpillStore>().unwrap();
    let saved = spill_store.save_text(request("hello")).await.unwrap();

    assert_eq!(
        saved,
        SpillRef {
            locator: SpillLocator::from("/stub/web_fetch.txt"),
            bytes: 5,
            retrieval_hint: "Use the stub reader.".into(),
        }
    );
    assert_eq!(lock(&stub.last).as_ref().unwrap().content, "hello");
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_second_implementation_one_per_context() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let first = mount(&ctx, Arc::new(StubStore::default()));
    first.settled().await.unwrap();

    let second = mount(&ctx, Arc::new(StubStore::default()));
    assert!(second.settled().await.is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn releases_the_service_on_disposal() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = mount(&ctx, Arc::new(StubStore::default()));
    fiber.settled().await.unwrap();
    assert!(ctx.get::<SpillStore>().is_some());

    fiber.dispose().await;

    assert!(ctx.get::<SpillStore>().is_none());
    kernel.shutdown().await;
}

#[test]
fn preserves_exact_public_wire_vocabulary() {
    let tool = request("hello");
    assert_eq!(
        serde_json::to_value(&tool).unwrap(),
        serde_json::json!({
            "owner": { "sessionId": "s1" },
            "source": {
                "kind": "tool",
                "toolName": "web_fetch",
                "callId": "c1",
                "label": "result"
            },
            "suggestedName": "web_fetch.txt",
            "content": "hello"
        })
    );

    let session_reference = SpillSource::SessionReference {
        session_id: SessionId::from("source-session"),
        label: "transcript".into(),
    };
    assert_eq!(
        serde_json::to_value(session_reference).unwrap(),
        serde_json::json!({
            "kind": "session-reference",
            "sessionId": "source-session",
            "label": "transcript"
        })
    );
}
