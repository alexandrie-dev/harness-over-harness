mod support;

use hoh_session::{Session, SessionSeq};
use hoh_session_title::{
    SessionTitleSource, fallback_session_title, fold_session_title, normalize_session_title,
    truncate_title_utf8,
};
use serde_json::json;

use support::*;

#[test]
fn removes_terminal_controls_collapses_whitespace_and_applies_caps() {
    assert_eq!(
        normalize_session_title("\u{1b}]0;stolen\u{7}  Hello\t brave\nnew world  ", 80).unwrap(),
        "Hello brave new world"
    );
    assert_eq!(
        fallback_session_title("one two three four", 3, 80).unwrap(),
        "one two three"
    );
    assert_eq!(fallback_session_title("你好世界", 5, 7).unwrap(), "你好");
    assert_eq!(fallback_session_title("😀😀", 5, 5).unwrap().len(), 4);
}

#[test]
fn terminal_escape_removal_is_sequential_like_the_source_regex_pipeline() {
    assert_eq!(
        normalize_session_title("\u{1b}\u{1b}]x\u{7}[31mA", 80).unwrap(),
        "A"
    );
    assert_eq!(
        normalize_session_title("\u{1b}\u{1b}[31m[", 80).unwrap(),
        ""
    );
}

#[test]
fn normalization_matches_javascript_whitespace_boundaries() {
    assert_eq!(
        normalize_session_title("\u{feff} A\u{2028}B \u{180e}", 80).unwrap(),
        "A B \u{180e}"
    );
}

#[test]
fn rejects_non_positive_public_limits() {
    assert_eq!(
        truncate_title_utf8("title", 0).unwrap_err(),
        "maxBytes must be a positive integer"
    );
    assert_eq!(
        fallback_session_title("title", 0, 10).unwrap_err(),
        "maxWords must be a positive integer"
    );
}

#[tokio::test]
async fn logs_and_folds_an_immediate_fallback() {
    let env = setup().await;
    let s = session(&env, "fresh");
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    let seq = append_human(&s, "  Build\nlog-backed session titles please  ");
    settle().await;
    let title = env.titles.get(&s).unwrap();
    assert_eq!(title.title, "Build log-backed session titles please");
    assert_eq!(title.message_seqs, vec![seq]);
    assert!(matches!(title.source, SessionTitleSource::Fallback));
    assert_eq!(s.derive_messages().unwrap().len(), 1);
    env.shutdown().await;
}

#[tokio::test]
async fn derives_from_direct_prompt_instead_of_injected_context() {
    let env = setup().await;
    let s = session(&env, "prefixed");
    append_source(&s, "session-reference", "Referenced snapshot");
    append_human(&s, "Explain this referenced session");
    settle().await;
    assert_eq!(
        env.titles.get(&s).unwrap().title,
        "Explain this referenced session"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn waits_through_ineligible_messages_and_keeps_first_fallback() {
    let env = setup().await;
    let s = session(&env, "eligibility");
    append_source(&s, "seed", "plugin text");
    append_human(&s, " \n\t ");
    settle().await;
    assert!(env.titles.get(&s).is_none());
    let first = append_human(&s, "first real prompt");
    settle().await;
    append_human(&s, "later prompt");
    settle().await;
    assert_eq!(env.titles.get(&s).unwrap().message_seqs, vec![first]);
    assert_eq!(
        s.snapshot_events(None, None)
            .iter()
            .filter(|e| e.event_type() == "session/title")
            .count(),
        1
    );
    env.shutdown().await;
}

#[test]
fn folds_the_latest_title_event_during_replay() {
    let s = Session::create("source".into(), None, None, None, None).unwrap();
    s.append(
        "session/title",
        json!({"title":"Earlier","messageSeqs":[1],"source":{"kind":"fallback"}}),
        None,
    )
    .unwrap();
    s.append("session/title", json!({"title":"Later","messageSeqs":[1,4],"source":{"kind":"provider","provider":"test","model":{"provider":"mock","model":"title-model"}}}), None).unwrap();
    let title = fold_session_title(&s.snapshot_events(None, None)).unwrap();
    assert_eq!(title.title, "Later");
    assert_eq!(
        title.message_seqs,
        vec![SessionSeq::new(1).unwrap(), SessionSeq::new(4).unwrap()]
    );
}

#[test]
fn folds_empty_or_titleless_log_to_none() {
    assert!(fold_session_title(&[]).is_none());
    let s = Session::create("empty".into(), None, None, None, None).unwrap();
    assert!(fold_session_title(&s.snapshot_events(None, None)).is_none());
}
