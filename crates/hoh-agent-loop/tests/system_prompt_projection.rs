//! Port of system-prompt-projection.spec.ts.
use hoh_agent_loop::{SystemPromptCommit, SystemPromptDecisionInput, SystemPromptProjection};
use hoh_kernel::Kernel;
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_session::{
    CreateSessionOptions, Session, SessionEvent, SessionId, SessionSeq, SessionStore, SurfaceIntent,
};
use serde_json::{Value, json};
use std::sync::Arc;

const REPLACING: SystemPromptDecisionInput = SystemPromptDecisionInput {
    in_history: false,
    starts_series: false,
};
const CONTINUING: SystemPromptDecisionInput = SystemPromptDecisionInput {
    in_history: true,
    starts_series: false,
};
const NEW_SERIES: SystemPromptDecisionInput = SystemPromptDecisionInput {
    in_history: true,
    starts_series: true,
};

struct Harness {
    _kernel: Kernel,
    store: SessionStore,
    ctx: hoh_kernel::Context,
}

fn store() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    Harness {
        _kernel: kernel,
        store: SessionStore::new(),
        ctx,
    }
}

fn session(h: &Harness, id: &str) -> Session {
    h.store
        .create(
            &h.ctx,
            Some(SessionId::from(id)),
            CreateSessionOptions::default(),
        )
        .expect("session")
}

fn append_user(session: &Session, text: &str) -> Arc<SessionEvent> {
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
}

fn message_json(commit: &SystemPromptCommit) -> Value {
    serde_json::to_value(&commit.message).unwrap()
}

fn commit(
    session: &Session,
    turn: u64,
    decision: Option<&SystemPromptCommit>,
) -> Arc<SessionEvent> {
    let decision = decision.expect("expected a system prompt commit");
    session
        .append(
            "system/message",
            json!({"turn": turn, "step": 1, "message": message_json(decision)}),
            Some(decision.intent.clone()),
        )
        .unwrap()
}

fn replace_of(seq: SessionSeq) -> Value {
    serde_json::to_value(SurfaceIntent::replace(seq, seq, vec![seq])).unwrap()
}

fn intent(commit: &SystemPromptCommit) -> Value {
    serde_json::to_value(&commit.intent).unwrap()
}

fn roles(session: &Session) -> Vec<String> {
    session
        .derive_messages()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_owned())
        .collect()
}

fn system_texts(session: &Session) -> Vec<Value> {
    session
        .derive_messages()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"][0].clone())
        .collect()
}

#[tokio::test]
async fn clears_multiblock_and_nontext_system_nodes_rather_than_treating_them_as_dormant() {
    let h = store();
    let session = session(&h, "system-prompt-multiblock");
    let base = serde_json::to_value(hoh_llm::create_system_message("old")).unwrap();
    let mut multi = base.clone();
    multi["content"] =
        json!([{"type": "text", "text": "old "}, {"type": "text", "text": "instructions"}]);
    session
        .append(
            "system/message",
            json!({"turn": 1, "step": 1, "message": multi}),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    append_user(&session, "hello");
    let mut tail = serde_json::to_value(hoh_llm::create_system_message("tail")).unwrap();
    tail["content"] = json!([{"type": "reasoning", "text": "retained content"}]);
    session
        .append(
            "system/message",
            json!({"turn": 1, "step": 1, "message": tail}),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let projection = SystemPromptProjection::new(session.clone());
    for update in projection.project("", CONTINUING).unwrap() {
        commit(&session, 2, Some(&update));
    }
    assert_eq!(roles(&session), ["user"]);
    assert!(projection.project("", CONTINUING).unwrap().is_empty());
}

#[tokio::test]
async fn appends_the_first_rendered_prompt_skips_an_unchanged_one_and_replaces_the_retained_node_on_change()
 {
    let h = store();
    let session = session(&h, "system-prompt-fresh");
    let projection = SystemPromptProjection::new(session.clone());

    let first = projection.project("v1", REPLACING).unwrap();
    let first = first.first().unwrap();
    assert_eq!(intent(first), json!({"surfaceOp": "append"}));
    let message = message_json(first);
    assert_eq!(message["role"], "system");
    assert_eq!(message["source"], json!({"kind": "system-prompt"}));
    let head = commit(&session, 1, Some(first));
    append_user(&session, "hello");

    assert!(projection.project("v1", REPLACING).unwrap().is_empty());
    let second = projection.project("v2", REPLACING).unwrap();
    assert_eq!(intent(&second[0]), replace_of(head.seq()));
    let replaced = commit(&session, 2, second.first());
    assert_eq!(
        session.surface().unwrap().nodes().unwrap()[0],
        replaced.seq()
    );
    assert!(projection.project("v2", REPLACING).unwrap().is_empty());

    // An emptied prompt keeps the head node with empty content, which projects to no wire message.
    let emptied = projection.project("", REPLACING).unwrap();
    assert_eq!(message_json(&emptied[0])["content"], json!([]));
    commit(&session, 3, emptied.first());
    assert_eq!(roles(&session), ["user"]);
    assert!(projection.project("", REPLACING).unwrap().is_empty());
    assert_eq!(
        intent(&projection.project("v3", REPLACING).unwrap()[0])["surfaceOp"]["op"],
        "replace"
    );
}

#[tokio::test]
async fn reserves_an_empty_head_before_user_history_and_replaces_it_when_a_prompt_appears() {
    let h = store();
    let session = session(&h, "system-prompt-empty-head");
    let projection = SystemPromptProjection::new(session.clone());
    let first = projection.project("", REPLACING).unwrap();
    assert_eq!(intent(&first[0]), json!({"surfaceOp": "append"}));
    assert_eq!(message_json(&first[0])["content"], json!([]));
    let head = commit(&session, 1, first.first());
    let user = append_user(&session, "hello");
    assert_eq!(
        session.surface().unwrap().nodes().unwrap(),
        [head.seq(), user.seq()]
    );
    assert_eq!(roles(&session), ["user"]);
    assert!(projection.project("", REPLACING).unwrap().is_empty());

    let next = projection
        .project("Follow this guidance.", REPLACING)
        .unwrap();
    assert_eq!(intent(&next[0]), replace_of(head.seq()));
    let replacement = commit(&session, 2, next.first());
    assert_eq!(
        session.surface().unwrap().nodes().unwrap(),
        [replacement.seq(), user.seq()]
    );
    assert_eq!(
        *session.derive_messages().unwrap(),
        [
            Arc::new(message_json(&next[0])),
            Arc::new(user.data().clone())
        ]
    );
}

#[tokio::test]
async fn reads_the_surviving_system_node_from_the_log_including_one_restored_as_no_prompt() {
    let h = store();
    let session = session(&h, "system-prompt-replay");
    let system =
        |text: &str| json!({"turn": 1, "step": 1, "message": hoh_llm::create_system_message(text)});
    let stale = session
        .append(
            "system/message",
            system("stale"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    append_user(&session, "hello");
    let current = session
        .append(
            "system/message",
            json!({"turn": 2, "step": 1, "message": hoh_llm::create_system_message("current")}),
            Some(SurfaceIntent::replace(
                stale.seq(),
                stale.seq(),
                vec![stale.seq()],
            )),
        )
        .unwrap();

    let projection = SystemPromptProjection::new(session.clone());
    assert!(projection.project("current", REPLACING).unwrap().is_empty());
    assert_eq!(
        intent(&projection.project("next", REPLACING).unwrap()[0]),
        replace_of(current.seq())
    );

    let empty_session = self::session(&h, "system-prompt-empty-replay");
    let empty = empty_session
        .append("system/message", system(""), Some(SurfaceIntent::append()))
        .unwrap();
    append_user(&empty_session, "hello");
    let empty_projection = SystemPromptProjection::new(empty_session);
    assert!(empty_projection.project("", REPLACING).unwrap().is_empty());
    assert_eq!(
        intent(&empty_projection.project("now present", REPLACING).unwrap()[0]),
        replace_of(empty.seq())
    );
}

#[tokio::test]
async fn appends_again_after_a_replacement_shadowed_a_system_node_that_was_not_the_head() {
    let h = store();
    let session = session(&h, "system-prompt-shadowed");
    let projection = SystemPromptProjection::new(session.clone());
    append_user(&session, "before any prompt");
    let late = projection.project("late prompt", REPLACING).unwrap();
    assert_eq!(intent(&late[0]), json!({"surfaceOp": "append"}));
    let node = commit(&session, 1, late.first());
    assert_eq!(
        session
            .surface()
            .unwrap()
            .nodes()
            .unwrap()
            .iter()
            .position(|seq| *seq == node.seq()),
        Some(1)
    );
    assert!(
        projection
            .project("late prompt", REPLACING)
            .unwrap()
            .is_empty()
    );

    let summary = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("summary")],
        source: MessageSource::new("test-compaction"),
    });
    session
        .append(
            "user/message",
            serde_json::to_value(summary).unwrap(),
            Some(SurfaceIntent::replace(
                node.seq(),
                node.seq(),
                vec![node.seq()],
            )),
        )
        .unwrap();
    assert_eq!(
        intent(&projection.project("late prompt", REPLACING).unwrap()[0]),
        json!({"surfaceOp": "append"})
    );
}

#[tokio::test]
async fn appends_a_changed_prompt_after_cached_history_on_an_in_history_route_while_the_series_continues()
 {
    let h = store();
    let session = session(&h, "system-prompt-in-history");
    let projection = SystemPromptProjection::new(session.clone());
    let head = commit(
        &session,
        1,
        projection.project("v1", CONTINUING).unwrap().first(),
    );
    let user = append_user(&session, "hello");

    assert!(projection.project("v1", CONTINUING).unwrap().is_empty());
    let update = projection.project("v2", CONTINUING).unwrap();
    assert_eq!(intent(&update[0]), json!({"surfaceOp": "append"}));
    let appended = commit(&session, 2, update.first());
    assert_eq!(
        session.surface().unwrap().nodes().unwrap(),
        [head.seq(), user.seq(), appended.seq()]
    );
    assert_eq!(roles(&session), ["system", "user", "system"]);
    assert!(projection.project("v2", CONTINUING).unwrap().is_empty());

    // The effective prompt is the latest surviving node: a further change compares against it.
    append_user(&session, "more");
    let third = projection.project("v3", CONTINUING).unwrap();
    assert_eq!(intent(&third[0]), json!({"surfaceOp": "append"}));
    commit(&session, 3, third.first());
    assert_eq!(
        system_texts(&session),
        [
            json!({"type": "text", "text": "v1"}),
            json!({"type": "text", "text": "v2"}),
            json!({"type": "text", "text": "v3"}),
        ]
    );
}

#[tokio::test]
async fn keeps_the_surviving_prompt_node_at_a_series_start_when_its_text_already_matches() {
    let h = store();
    let session = session(&h, "system-prompt-series-start-current");
    let projection = SystemPromptProjection::new(session.clone());
    let head = commit(
        &session,
        1,
        projection.project("v1", CONTINUING).unwrap().first(),
    );
    append_user(&session, "hello");
    let appended = commit(
        &session,
        2,
        projection.project("v2", CONTINUING).unwrap().first(),
    );

    // The effective text is already current: re-baselining node 0 would move the same text into
    // the request's first message and invalidate the provider prefix.
    assert!(projection.project("v2", NEW_SERIES).unwrap().is_empty());
    let nodes = session.surface().unwrap().nodes().unwrap();
    assert_eq!(nodes[0], head.seq());
    assert!(nodes.contains(&appended.seq()));
    assert_eq!(
        system_texts(&session),
        [
            json!({"type": "text", "text": "v1"}),
            json!({"type": "text", "text": "v2"})
        ]
    );
}

#[tokio::test]
async fn re_baselines_node_zero_and_empties_later_active_nodes_at_a_series_start() {
    let h = store();
    let session = session(&h, "system-prompt-series-start");
    let projection = SystemPromptProjection::new(session.clone());
    let head = commit(
        &session,
        1,
        projection.project("v1", CONTINUING).unwrap().first(),
    );
    append_user(&session, "hello");

    // A new series already costs the cache, so the change folds into node 0.
    let rebased = projection.project("v2", NEW_SERIES).unwrap();
    assert_eq!(intent(&rebased[0]), replace_of(head.seq()));
    let new_head = commit(&session, 2, rebased.first());
    assert_eq!(
        session.surface().unwrap().nodes().unwrap()[0],
        new_head.seq()
    );

    append_user(&session, "again");
    let later = commit(
        &session,
        3,
        projection.project("v3", CONTINUING).unwrap().first(),
    );
    let updates = projection.project("v4", NEW_SERIES).unwrap();
    assert_eq!(
        updates.iter().map(intent).collect::<Vec<_>>(),
        [replace_of(later.seq()), replace_of(new_head.seq())]
    );
    for update in &updates {
        commit(&session, 4, Some(update));
    }
    let systems: Vec<Value> = session
        .derive_messages()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].clone())
        .collect();
    assert_eq!(systems, [json!([{"type": "text", "text": "v4"}])]);
    assert!(projection.project("v4", CONTINUING).unwrap().is_empty());
}

#[tokio::test]
async fn empties_all_active_nodes_when_clearing_an_in_history_prompt() {
    let h = store();
    let session = session(&h, "system-prompt-in-history-clear");
    let projection = SystemPromptProjection::new(session.clone());
    commit(
        &session,
        1,
        projection.project("v1", CONTINUING).unwrap().first(),
    );
    append_user(&session, "hello");
    let update = commit(
        &session,
        2,
        projection.project("v2", CONTINUING).unwrap().first(),
    );

    let cleared = projection.project("", CONTINUING).unwrap();
    assert_eq!(cleared.len(), 2);
    assert_eq!(intent(&cleared[0]), replace_of(update.seq()));
    for empty in &cleared {
        assert_eq!(message_json(empty)["content"], json!([]));
        commit(&session, 3, Some(empty));
    }
    assert_eq!(roles(&session), ["user"]);
    assert!(projection.project("", CONTINUING).unwrap().is_empty());
    let after = projection.project("v3", REPLACING).unwrap();
    assert_eq!(
        after
            .iter()
            .map(|c| message_json(c)["content"].clone())
            .collect::<Vec<_>>(),
        [json!([{"type": "text", "text": "v3"}])]
    );
}
