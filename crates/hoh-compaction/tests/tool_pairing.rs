mod common;

use common::*;
use hoh_compaction::{
    CompactionId, compact_checkpoint_source, tool_pairing_balanced_after,
    tool_pairing_balanced_before,
};
use hoh_session::{SessionSeq, SurfaceIntent};
use serde_json::json;

fn before(session: &hoh_session::Session, seq: SessionSeq) -> bool {
    tool_pairing_balanced_before(session, seq).expect("balance")
}

fn after(session: &hoh_session::Session, seq: SessionSeq) -> bool {
    tool_pairing_balanced_after(session, seq).expect("balance")
}

fn closed_tool_step() -> hoh_session::Session {
    let session = session("closed-tool-step");
    append_user(&session, "go");
    append_assistant_calls(&session, &["c1"]);
    append_tool_result(&session, "c1");
    session
}

#[test]
fn classifies_closed_and_open_single_call_steps() {
    let closed = closed_tool_step();
    let nodes = closed.surface().unwrap().nodes().unwrap();
    assert!(before(&closed, nodes[0]));
    assert!(after(&closed, nodes[0]));
    assert!(before(&closed, nodes[1]));
    assert!(!after(&closed, nodes[1]));
    assert!(!before(&closed, nodes[2]));
    assert!(after(&closed, nodes[2]));

    let open = session("open-tool-step");
    let assistant = append_assistant_calls(&open, &["open"]);
    assert!(!after(&open, assistant.seq()));
}

#[test]
fn requires_every_result_from_a_multiple_call_assistant_message() {
    let session = session("multiple-calls");
    append_assistant_calls(&session, &["c1", "c2"]);
    let first = append_tool_result(&session, "c1");
    let second = append_tool_result(&session, "c2");
    assert!(!after(&session, first.seq()));
    assert!(after(&session, second.seq()));
}

#[test]
fn keeps_neutral_nodes_inside_an_open_pair_unbalanced_and_free_nodes_balanced() {
    let middle = session("neutral-mid-step");
    append_assistant_calls(&middle, &["c1"]);
    let user = append_user(&middle, "background update");
    append_tool_result(&middle, "c1");
    assert!(!before(&middle, user.seq()));
    assert!(!after(&middle, user.seq()));

    let free = session("neutral-free");
    let user = append_user(&free, "idle injection");
    assert!(before(&free, user.seq()));
    assert!(after(&free, user.seq()));
}

#[test]
fn rebuilds_after_replace_and_rejects_sequences_removed_from_current_membership() {
    let session = closed_tool_step();
    let old_nodes = session.surface().unwrap().nodes().unwrap();
    let stale_tail = *old_nodes.last().unwrap();
    assert!(after(&session, stale_tail));

    let checkpoint = compact_checkpoint_source(CompactionId::from("tool-pairing-compaction"), None);
    let checkpoint = session
        .append(
            "user/message",
            json!({
                "content": [{"type":"text","text":"checkpoint"}],
                "source": checkpoint,
                "role": "user",
                "id": "checkpoint",
            }),
            Some(SurfaceIntent::replace(
                old_nodes[0],
                *old_nodes.last().unwrap(),
                old_nodes,
            )),
        )
        .unwrap();
    assert!(before(&session, checkpoint.seq()));
    assert!(after(&session, checkpoint.seq()));
    for result in [
        tool_pairing_balanced_before(&session, stale_tail),
        tool_pairing_balanced_after(&session, stale_tail),
    ] {
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}

#[test]
fn answers_repeated_queries_from_balances() {
    let session = closed_tool_step();
    let assistant = session.surface().unwrap().nodes().unwrap()[1];
    assert!(!after(&session, assistant));
    assert!(!after(&session, assistant));
}

#[test]
fn rejects_missing_seqs_before_and_after_including_an_empty_surface() {
    let session = session("missing-membership");
    for result in [
        tool_pairing_balanced_before(&session, seq(999)),
        tool_pairing_balanced_after(&session, seq(999)),
    ] {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("surface seq 999 not found")
        );
    }
    let first = append_user(&session, "first node after empty cache");
    assert!(after(&session, first.seq()));
}

#[test]
fn follows_log_only_growth_appended_nodes_and_replacements() {
    let session = closed_tool_step();
    let tail = *session.surface().unwrap().nodes().unwrap().last().unwrap();
    assert!(after(&session, tail));
    session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    assert!(after(&session, tail));
    let appended = append_user(&session, "tail");
    assert!(after(&session, appended.seq()));
    append_assistant_calls(&session, &["c2"]);
    let result = append_tool_result(&session, "c2");
    assert!(after(&session, result.seq()));
}

#[test]
fn throws_for_an_orphan_result_during_a_rebuild() {
    let session = session("orphan-rebuild");
    let orphan = append_tool_result(&session, "orphan");
    assert!(
        tool_pairing_balanced_after(&session, orphan.seq())
            .unwrap_err()
            .to_string()
            .contains("no matching tool-call")
    );
}

#[test]
fn retries_an_orphan_result_in_an_appended_tail_without_hidden_partial_state() {
    let session = session("orphan-tail");
    let head = append_user(&session, "safe head");
    assert!(after(&session, head.seq()));
    let orphan = append_tool_result(&session, "orphan");
    for _ in 0..2 {
        assert!(
            tool_pairing_balanced_after(&session, orphan.seq())
                .unwrap_err()
                .to_string()
                .contains("no matching tool-call")
        );
    }
}
