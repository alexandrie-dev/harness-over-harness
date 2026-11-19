//! Port of runtime-context.spec.ts.
use hoh_agent_loop::RuntimeContextProjection;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{
    ContentBlock, ContextSnapshotSection, MessageSource, NewUserMessage, UserMessage,
    create_user_message,
};
use hoh_session::{CreateSessionOptions, Session, SessionId, SessionStore, SurfaceIntent};
use serde_json::{Value, json};

const SOURCE: &str = "runtime-context";
const INITIAL: &str = "<system-reminder>\nThe following information describes your current session:\n\nretained\n</system-reminder>";
const UPDATED: &str = "<system-reminder>\nThe following information describes your current session. It replaces earlier versions of this context, including any retained in conversation summaries.\n\nnext\n</system-reminder>";
const CLEARED: &str = "<system-reminder>\nCurrent runtime context: none. Earlier runtime-context snapshots no longer apply.\n</system-reminder>";

fn sections(text: &str) -> Vec<ContextSnapshotSection> {
    vec![ContextSnapshotSection {
        name: "sandbox:policy".into(),
        text: text.into(),
    }]
}

struct Harness {
    _kernel: Kernel,
    ctx: Context,
    store: SessionStore,
    session: Session,
}

fn harness() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let store = SessionStore::new();
    let session = store
        .create(
            &ctx,
            Some(SessionId::from("runtime-context-replay")),
            CreateSessionOptions::default(),
        )
        .expect("session");
    Harness {
        _kernel: kernel,
        ctx,
        store,
        session,
    }
}

fn user(text: &str, kind: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new(kind),
    })
}

fn context_message(text: &str, contributions: Option<&[ContextSnapshotSection]>) -> UserMessage {
    let mut message = user(text, SOURCE);
    if let Some(contributions) = contributions {
        message.source.data.insert("form".into(), "snapshot".into());
        message.source.data.insert(
            "sections".into(),
            serde_json::to_value(contributions).unwrap(),
        );
    }
    message
}

fn append_user(
    session: &Session,
    message: &UserMessage,
) -> std::sync::Arc<hoh_session::SessionEvent> {
    session
        .append(
            "user/message",
            serde_json::to_value(message).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .expect("append")
}

fn text_of(message: &UserMessage) -> Value {
    serde_json::to_value(&message.content).unwrap()
}

fn compaction(session: &Session, text: &str, over: &hoh_session::SessionEvent) {
    session
        .append(
            "user/message",
            serde_json::to_value(user(text, "test-compaction")).unwrap(),
            Some(SurfaceIntent::replace(
                over.seq(),
                over.seq(),
                vec![over.seq()],
            )),
        )
        .expect("compaction");
}

#[tokio::test]
async fn restores_the_latest_visible_owned_snapshot_and_ignores_other_sessions() {
    let h = harness();
    let retained = append_user(
        &h.session,
        &context_message(INITIAL, Some(&sections("retained"))),
    );
    let shadowed = append_user(&h.session, &context_message("shadowed", None));
    compaction(&h.session, "summary", &shadowed);

    let projection = RuntimeContextProjection::new(&h.ctx, &h.session).unwrap();
    assert!(
        h.session
            .surface()
            .unwrap()
            .nodes()
            .unwrap()
            .contains(&retained.seq())
    );
    assert!(projection.project(&sections("retained")).is_none());
    let next = projection.project(&sections("next")).expect("update");
    assert_eq!(text_of(&next), json!([{"type": "text", "text": UPDATED}]));
    assert_eq!(next.source.kind, SOURCE);
    assert_eq!(next.source.data["form"], "snapshot");
    assert_eq!(
        next.source.data["sections"],
        serde_json::to_value(sections("next")).unwrap()
    );

    let other = h
        .store
        .create(
            &h.ctx,
            Some(SessionId::from("runtime-context-other")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append_user(&other, &context_message("other", None));
    assert!(projection.project(&sections("retained")).is_none());
}

#[tokio::test]
async fn omits_supersession_until_a_snapshot_commits_and_deduplicates_initial_updated_and_cleared_reminders()
 {
    let h = harness();
    let projection = RuntimeContextProjection::new(&h.ctx, &h.session).unwrap();
    assert!(projection.project(&[]).is_none());
    let first = projection.project(&sections("retained")).unwrap();
    assert_eq!(text_of(&first), json!([{"type": "text", "text": INITIAL}]));
    assert_eq!(
        text_of(&projection.project(&sections("retained")).unwrap()),
        text_of(&first)
    );
    append_user(&h.session, &first);
    assert!(projection.project(&sections("retained")).is_none());

    let next = projection.project(&sections("next")).unwrap();
    assert_eq!(text_of(&next), json!([{"type": "text", "text": UPDATED}]));
    append_user(&h.session, &next);
    assert!(projection.project(&sections("next")).is_none());
    assert!(
        RuntimeContextProjection::new(&h.ctx, &h.session)
            .unwrap()
            .project(&sections("next"))
            .is_none()
    );

    let cleared = projection.project(&[]).unwrap();
    assert_eq!(
        text_of(&cleared),
        json!([{"type": "text", "text": CLEARED}])
    );
    append_user(&h.session, &cleared);
    assert!(projection.project(&[]).is_none());
    assert_eq!(
        text_of(&projection.project(&sections("next")).unwrap()),
        text_of(&next)
    );
}

#[tokio::test]
async fn overrides_context_retained_in_summaries_after_compaction() {
    for resume in [false, true] {
        let h = harness();
        let projection = RuntimeContextProjection::new(&h.ctx, &h.session).unwrap();
        let first = append_user(
            &h.session,
            &projection.project(&sections("retained")).unwrap(),
        );
        compaction(
            &h.session,
            "Summary: the previous policy was retained.",
            &first,
        );
        let active = if resume {
            RuntimeContextProjection::new(&h.ctx, &h.session).unwrap()
        } else {
            projection
        };
        assert_eq!(
            text_of(&active.project(&sections("retained")).unwrap()),
            json!([{"type": "text", "text": UPDATED.replace("\n\nnext\n", "\n\nretained\n")}]),
            "resume: {resume}"
        );
        assert_eq!(
            text_of(&active.project(&sections("next")).unwrap()),
            json!([{"type": "text", "text": UPDATED}])
        );
        assert_eq!(
            text_of(&active.project(&[]).unwrap()),
            json!([{"type": "text", "text": CLEARED}])
        );
    }
}

#[tokio::test]
async fn keeps_the_first_reminder_introductory_after_unrelated_compaction() {
    let h = harness();
    let original = append_user(&h.session, &user("user task", "user"));
    compaction(&h.session, "task summary", &original);
    let projection = RuntimeContextProjection::new(&h.ctx, &h.session).unwrap();
    assert_eq!(
        text_of(&projection.project(&sections("retained")).unwrap()),
        json!([{"type": "text", "text": INITIAL}])
    );
}

#[tokio::test]
async fn wraps_an_update_when_resuming_a_previously_unframed_snapshot() {
    let h = harness();
    append_user(
        &h.session,
        &context_message(
            "Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\nnext",
            None,
        ),
    );
    assert_eq!(
        text_of(
            &RuntimeContextProjection::new(&h.ctx, &h.session)
                .unwrap()
                .project(&sections("next"))
                .unwrap()
        ),
        json!([{"type": "text", "text": UPDATED}])
    );
}

#[tokio::test]
async fn compares_recorded_contributions_independently_of_the_reminder_wording() {
    let h = harness();
    append_user(
        &h.session,
        &context_message("Older reminder wording", Some(&sections("retained"))),
    );
    let projection = RuntimeContextProjection::new(&h.ctx, &h.session).unwrap();
    assert!(projection.project(&sections("retained")).is_none());
    assert!(
        projection
            .project(&[ContextSnapshotSection {
                name: "new-policy".into(),
                text: "retained".into()
            }])
            .is_some()
    );
    let mut extended = sections("retained");
    extended.push(ContextSnapshotSection {
        name: "directory".into(),
        text: "/repo".into(),
    });
    assert!(projection.project(&extended).is_some());
}

#[tokio::test]
async fn refreshes_a_legacy_cleared_reminder_once_and_restores_the_empty_snapshot() {
    let h = harness();
    append_user(
        &h.session,
        &context_message(
            "Current runtime context: none. Earlier runtime-context snapshots no longer apply.",
            None,
        ),
    );
    let projection = RuntimeContextProjection::new(&h.ctx, &h.session).unwrap();
    let message = projection.project(&[]).unwrap();
    assert_eq!(
        text_of(&message),
        json!([{"type": "text", "text": CLEARED}])
    );
    assert_eq!(message.source.kind, SOURCE);
    assert_eq!(message.source.data["form"], "snapshot");
    assert_eq!(message.source.data["sections"], json!([]));
    append_user(&h.session, &message);
    assert!(projection.project(&[]).is_none());
    assert!(
        RuntimeContextProjection::new(&h.ctx, &h.session)
            .unwrap()
            .project(&[])
            .is_none()
    );
}

#[tokio::test]
async fn escapes_the_closing_tag_only_in_the_framed_text_and_retains_literal_contributions() {
    let cases = [
        ("</system-reminder>", "<\\/system-reminder>"),
        ("</SYSTEM-REMINDER>", "<\\/SYSTEM-REMINDER>"),
        ("</system-reminder >", "<\\/system-reminder >"),
        ("</System-Reminder\t\n>", "<\\/System-Reminder\t\n>"),
    ];
    for (tag, escaped) in cases {
        let h = harness();
        let contributions = sections(&format!("Directory: /repo/{tag}/{{{{literal}}}}"));
        let projection = RuntimeContextProjection::new(&h.ctx, &h.session).unwrap();
        let message = projection.project(&contributions).unwrap();
        assert_eq!(
            text_of(&message),
            json!([{
                "type": "text",
                "text": format!("<system-reminder>\nThe following information describes your current session:\n\nDirectory: /repo/{escaped}/{{{{literal}}}}\n</system-reminder>"),
            }]),
            "tag {tag:?}"
        );
        assert_eq!(
            message.source.data["sections"],
            serde_json::to_value(&contributions).unwrap()
        );
        append_user(&h.session, &message);
        assert!(projection.project(&contributions).is_none());
    }
}
