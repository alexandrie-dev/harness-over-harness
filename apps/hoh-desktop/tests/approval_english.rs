use hoh_desktop::approval::Broker;
use serde_json::json;

#[test]
fn approval_source_has_no_cyrillic_except_the_legacy_journal_prefix() {
    let allowed =
        "const LEGACY_CORE_SIGNATURE_PREFIX: &str = \"подписано ядром без второго окна\";";
    let cyrillic_lines: Vec<_> = include_str!("../src/approval.rs")
        .lines()
        .filter(|line| {
            line.chars().any(|c| {
                ('\u{0400}'..='\u{052f}').contains(&c)
                    || ('\u{1c80}'..='\u{1c8f}').contains(&c)
                    || ('\u{2de0}'..='\u{2dff}').contains(&c)
                    || ('\u{a640}'..='\u{a69f}').contains(&c)
                    || ('\u{1e030}'..='\u{1e08f}').contains(&c)
            })
        })
        .map(str::trim)
        .collect();
    assert_eq!(cyrillic_lines, [allowed]);
}

#[test]
fn history_filters_both_legacy_and_english_core_signatures_from_disk() {
    let path = std::env::temp_dir().join(format!(
        "hoh-legacy-approval-history-{}.jsonl",
        std::process::id()
    ));
    let entries = [
        json!({"at":1, "who":"human", "via":"main-window", "method":"action", "humanText":"Person confirmed", "outcome":"allowed-once", "note":"подтверждено человеком действием в главном окне"}),
        json!({"at":2, "who":"core", "via":"main-window", "method":"action", "humanText":"Old duplicate", "outcome":"allowed-once", "note":"подписано ядром без второго окна: то же действие подтверждено человеком"}),
        json!({"at":3, "who":"core", "via":"main-window", "method":"action", "humanText":"New duplicate", "outcome":"allowed-once", "note":"signed by the core without a second window: same action"}),
        json!({"at":4, "who":"core", "via":"approval-window", "method":"action", "humanText":"Expired request", "outcome":"expired", "note":"expired without an answer"}),
    ];
    let content = entries
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, content).unwrap();
    let broker = Broker::new([0x33; 32])
        .unwrap()
        .with_audit_file(path.clone());
    let history = broker.history(10);
    assert_eq!(history.len(), 2, "{history:?}");
    assert_eq!(history[0]["humanText"], "Expired request");
    assert_eq!(history[1]["humanText"], "Person confirmed");
    std::fs::remove_file(path).unwrap();
}
