use hoh_package_manifest::LocalizedText;
use hoh_util_text_search::{SearchKeys, TextSearchIndex, localized_values};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    title: Option<String>,
    description: Option<String>,
}

fn row(id: &str) -> Row {
    Row {
        id: id.into(),
        title: None,
        description: None,
    }
}

fn full_row(id: &str, title: &str, description: &str) -> Row {
    Row {
        id: id.into(),
        title: Some(title.into()),
        description: Some(description.into()),
    }
}

fn rows() -> Vec<Row> {
    vec![
        full_row(
            "compact",
            "Compact context",
            "Summarize earlier turns to free model context",
        ),
        full_row(
            "clipboard-paste",
            "Clipboard paste",
            "Paste images and text into the composer",
        ),
        full_row(
            "computer-use",
            "Computer use",
            "Control the mouse, keyboard, and screen",
        ),
        full_row(
            "session-title",
            "Session title",
            "Generate a title for each session with a model",
        ),
    ]
}

fn keys(row: &Row) -> SearchKeys {
    SearchKeys {
        fuzzy: vec![Some(row.id.clone()), row.title.clone()],
        substring: vec![row.description.clone()],
    }
}

fn ids(query: &str, items: Vec<Row>) -> Vec<String> {
    TextSearchIndex::new(items, keys)
        .search(query)
        .iter()
        .map(|row| row.id.clone())
        .collect()
}

#[test]
fn localized_values_returns_every_language() {
    assert!(localized_values(None).is_empty());
    let plain = LocalizedText::Text("Plain".into());
    assert_eq!(localized_values(Some(&plain)), ["Plain"]);
    let translated = LocalizedText::Translations(
        [("en".into(), "Filter".into()), ("zh".into(), "筛选".into())]
            .into_iter()
            .collect(),
    );
    assert_eq!(localized_values(Some(&translated)), ["Filter", "筛选"]);
}

#[test]
fn blank_query_returns_build_order_and_item_references() {
    let index = TextSearchIndex::new(rows(), keys);
    let found = index.search("   ");
    assert_eq!(found, index.items().iter().collect::<Vec<_>>());
    assert!(std::ptr::eq(found[0], &index.items()[0]));
}

#[test]
fn fuzzy_keys_match_as_subsequences() {
    let found = ids("cpt", rows());
    assert!(found.contains(&"compact".into()));
    assert!(found.contains(&"computer-use".into()));
    assert!(!found.contains(&"session-title".into()));
    assert!(ids("xyz", rows()).is_empty());
}

#[test]
fn ranks_the_four_match_tiers() {
    let mut notes = row("notes");
    notes.description = Some("Keep model notes".into());
    assert_eq!(
        ids(
            "model",
            vec![notes, row("mxoxdxexl"), row("big-model"), row("model-list")]
        ),
        ["model-list", "big-model", "mxoxdxexl", "notes"]
    );
}

#[test]
fn orders_fuzzy_hits_by_fuzzysort_score() {
    assert_eq!(
        ids("ftr", vec![row("filter"), row("f-t-r-x")]),
        ["f-t-r-x", "filter"]
    );
}

#[test]
fn equal_matches_keep_build_order() {
    assert_eq!(
        ids("open", vec![row("open-z"), row("open-a"), row("x-open")]),
        ["open-z", "open-a", "x-open"]
    );
}

#[test]
fn fuzzy_subsequence_must_start_at_a_word_start() {
    let mut refactor = row("refactor");
    refactor.title = Some("Refactor build pipeline".into());
    let mut flash = row("flash");
    flash.title = Some("DeepSeekFlash".into());
    let mut zh = row("zh");
    zh.title = Some("插件筛选".into());
    let items = vec![refactor, flash, zh];
    assert!(ids("fab", items.clone()).is_empty());
    assert_eq!(ids("rbp", items.clone()), ["refactor"]);
    assert_eq!(ids("sflsh", items.clone()), ["flash"]);
    assert_eq!(ids("件选", items), ["zh"]);
    assert_eq!(ids("bz", vec![row("foo-bar baz")]), ["foo-bar baz"]);
}

#[test]
fn finds_later_word_start_after_earlier_mid_word_letter() {
    assert_eq!(
        ids("plg", vec![row("Open plugin manager")]),
        ["Open plugin manager"]
    );
    assert_eq!(ids("cpt", vec![row("Script compact")]), ["Script compact"]);
}

#[test]
fn multi_term_query_can_use_the_same_key() {
    assert_eq!(
        ids(
            "plgn mngr",
            vec![row("Open plugin manager"), row("Open plugin list")]
        ),
        ["Open plugin manager"]
    );
    assert_eq!(
        ids("plgn plg", vec![row("Open plugin manager")]),
        ["Open plugin manager"]
    );
}

#[test]
fn opaque_substring_ids_match_only_contiguously() {
    let mut first = row("title-only");
    first.description = Some("session-3f2a9c1d-77aa-4b1e-9d0e-5a6b7c8d9e0f".into());
    let mut second = row("other");
    second.description = Some("session-a1b2c3d4-0000-4000-8000-000000000000".into());
    let items = vec![first, second];
    assert_eq!(ids("a1", items.clone()), ["other"]);
    assert_eq!(ids("3f2a9c", items), ["title-only"]);
}

#[test]
fn substring_keys_reject_scattered_letters() {
    assert_eq!(ids("mouse", rows()), ["computer-use"]);
    assert!(ids("invoice", rows()).is_empty());
}

#[test]
fn every_query_term_is_required() {
    assert_eq!(ids("paste composer", rows()), ["clipboard-paste"]);
    assert!(ids("paste mouse", rows()).is_empty());
}

#[test]
fn every_localized_language_matches() {
    #[derive(Debug, PartialEq, Eq)]
    struct LocalizedRow {
        id: String,
        title: LocalizedText,
        description: LocalizedText,
    }
    let item = LocalizedRow {
        id: "plugin-filter".into(),
        title: LocalizedText::Translations(
            [
                ("en".into(), "Plugin filter".into()),
                ("zh".into(), "插件筛选".into()),
            ]
            .into_iter()
            .collect(),
        ),
        description: LocalizedText::Translations(
            [
                ("en".into(), "Filter by name".into()),
                ("zh".into(), "按名称筛选".into()),
            ]
            .into_iter()
            .collect(),
        ),
    };
    let index = TextSearchIndex::new(vec![item], |item| SearchKeys {
        fuzzy: std::iter::once(Some(item.id.clone()))
            .chain(
                localized_values(Some(&item.title))
                    .into_iter()
                    .map(|text| Some(text.into())),
            )
            .collect(),
        substring: localized_values(Some(&item.description))
            .into_iter()
            .map(|text| Some(text.into()))
            .collect(),
    });
    for query in ["筛选", "名称", "plugin 筛选"] {
        assert_eq!(index.search(query).len(), 1);
    }
    assert!(index.search("选筛").is_empty());
}

#[test]
fn ignores_case_width_and_diacritics() {
    let mut item = row("ＷＥＢ-search");
    item.description = Some("Café résumé".into());
    for query in ["web", "WEB", "cafe", "RESUME"] {
        assert_eq!(ids(query, vec![item.clone()]), ["ＷＥＢ-search"]);
    }
}

#[test]
fn fuzzy_subsequences_use_the_same_normalization() {
    for (query, name) in [
        ("ｃｐｔ", "Compact"),
        ("wb", "ＷＥＢ-search"),
        ("re\u{0301}sm", "Résumé"),
        ("resm", "Résumé"),
        ("lodz", "Łódź"),
        ("ostf", "Østfold"),
        ("mlg", "Małgorzata"),
    ] {
        assert_eq!(ids(query, vec![row(name)]), [name]);
    }
    assert!(ids("か", vec![row("が")]).is_empty());
}

#[test]
fn combining_mark_only_term_is_ignored() {
    let items = vec![row("alpha"), row("beta")];
    let index = TextSearchIndex::new(items.clone(), keys);
    assert_eq!(
        index.search("\u{0301}"),
        index.items().iter().collect::<Vec<_>>()
    );
    assert_eq!(ids("\u{0301} beta", items), ["beta"]);
}

#[test]
fn word_starts_are_found_by_code_point() {
    assert_eq!(ids("𠀀b", vec![row("中𠀀x-b")]), ["中𠀀x-b"]);
    assert!(ids("x𠀀", vec![row("a 𠀀")]).is_empty());
}

#[test]
fn absent_and_empty_values_are_skipped() {
    let mut empty = row("");
    empty.title = None;
    let mut kept = row("kept");
    kept.title = Some(String::new());
    let items = vec![empty, kept];
    let index = TextSearchIndex::new(items, |row| SearchKeys {
        fuzzy: vec![Some(row.id.clone()), row.title.clone(), None],
        substring: vec![None, Some(String::new())],
    });
    assert_eq!(index.search("kept"), vec![&index.items()[1]]);
    let no_keys = TextSearchIndex::new(vec![row("kept")], |_| SearchKeys::default());
    assert!(no_keys.search("kept").is_empty());
}

#[test]
fn update_searches_the_complete_new_list() {
    let original = rows();
    let index = TextSearchIndex::new(original.clone(), keys);
    let mut changed: Vec<_> = original
        .into_iter()
        .filter(|row| row.id != "compact")
        .collect();
    let mut added = row("context-window");
    added.title = Some("Context window".into());
    changed.push(added);
    let next = index.update(changed, keys);
    assert!(!next.search("cpt").iter().any(|row| row.id == "compact"));
    assert_eq!(
        next.search("ctx")
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["context-window"]
    );
    assert_eq!(next.search(""), next.items().iter().collect::<Vec<_>>());
}

#[test]
fn update_reads_changed_keys_and_leaves_old_index_valid() {
    let original = rows();
    let index = TextSearchIndex::new(original.clone(), keys);
    let renamed = original
        .into_iter()
        .map(|mut row| {
            if row.id == "compact" {
                row.id = "shrink".into();
                row.title = Some("Shrink history".into());
            }
            row
        })
        .collect();
    let next = index.update(renamed, keys);
    assert_eq!(
        next.search("shrnk")
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["shrink"]
    );
    assert!(next.search("cmpct").is_empty());
    assert_eq!(
        index
            .search("cmpct")
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["compact"]
    );
    let blank = index.search("");
    assert_eq!(blank, index.items().iter().collect::<Vec<_>>());
    assert!(std::ptr::eq(blank[0], &index.items()[0]));
}

#[test]
fn updated_index_matches_a_fresh_build() {
    let index = TextSearchIndex::new(rows(), keys);
    let queries = ["cpt", "model", "mouse", "paste composer", "ses tit"];
    for query in queries {
        index.search(query);
    }
    let changed: Vec<_> = rows()
        .into_iter()
        .rev()
        .filter(|row| row.id != "computer-use")
        .collect();
    let next = index.update(changed.clone(), keys);
    let fresh = TextSearchIndex::new(changed, keys);
    for query in queries {
        assert_eq!(next.search(query), fresh.search(query));
    }
}

#[test]
fn keys_are_read_only_while_building() {
    let mut calls = 0;
    let index = TextSearchIndex::new(rows(), |row| {
        calls += 1;
        keys(row)
    });
    index.search("cpt");
    index.search("model");
    assert_eq!(calls, 4);
    index.update(rows(), |row| {
        calls += 1;
        keys(row)
    });
    assert_eq!(calls, 8);
}
