use hoh_message_quotes::{
    apply,
    client::{
        INJECT, MESSAGE_QUOTES_NAMESPACE, NAME, QUOTE_ACTION_SLOT, QUOTE_INTERACTION_ID,
        QUOTE_RULE_ORDER, QUOTED_TEXT_PRIORITY, QUOTED_TEXT_SLOT, QuoteInteractionTrigger,
        SessionQuoteState, SubagentMode, UserTextNode, activate_quote_action,
        admit_quote_interaction, evaluate_quote_text, message_quote_ranges, quote_add_label,
        quote_rule_match, quote_selection_enabled, quote_text_of,
    },
};

#[test]
fn normalizes_line_endings_and_decodes_one_quote_level() {
    assert_eq!(
        quote_text_of("> first\r\n  > > nested\n>third"),
        "first\n> nested\nthird"
    );
    assert_eq!(quote_text_of("> a\rb\r> c"), "a\nb\nc");
}

#[test]
fn recognizes_consecutive_explicit_quote_lines_and_preserves_utf16_offsets() {
    let text = "before\n\n> first\n> line\n\n> second\n\nafter";
    assert_eq!(
        message_quote_ranges(text),
        vec![
            hoh_message_quotes::client::MessageQuoteRange { start: 8, end: 22 },
            hoh_message_quotes::client::MessageQuoteRange { start: 24, end: 32 },
        ]
    );
    assert_eq!(
        message_quote_ranges("  > first\r\n>second"),
        vec![hoh_message_quotes::client::MessageQuoteRange { start: 0, end: 18 }]
    );
    assert_eq!(
        message_quote_ranges("😀\n> quote"),
        vec![hoh_message_quotes::client::MessageQuoteRange { start: 3, end: 10 }]
    );
}

#[test]
fn leaves_comparisons_indented_code_and_blank_excerpts_as_ordinary_text() {
    assert!(message_quote_ranges("a > b\n    > code\n> \n>").is_empty());
    assert!(message_quote_ranges("").is_empty());
    assert!(message_quote_ranges("> \u{feff}").is_empty());
}

#[test]
fn rule_preserves_quote_and_separator_source_offsets() {
    let text = "before\n\n> first\n> line\n\n> second\n\nafter";
    let nodes = evaluate_quote_text(text);
    assert_eq!(
        nodes
            .iter()
            .filter(|node| matches!(node, UserTextNode::Quote { .. }))
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            UserTextNode::Quote {
                start: 8,
                end: 22,
                text: "first\nline".into(),
            },
            UserTextNode::Quote {
                start: 24,
                end: 32,
                text: "second".into(),
            },
        ]
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|node| matches!(
                node,
                UserTextNode::Text {
                    display: Some(_),
                    ..
                }
            ))
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            UserTextNode::Text {
                start: 6,
                end: 8,
                text: "\n\n".into(),
                display: Some(" ".into()),
            },
            UserTextNode::Text {
                start: 22,
                end: 24,
                text: "\n\n".into(),
                display: Some(" ".into()),
            },
            UserTextNode::Text {
                start: 32,
                end: 34,
                text: "\n\n".into(),
                display: Some(" ".into()),
            },
        ]
    );
    let source = nodes
        .iter()
        .map(|node| match node {
            UserTextNode::Quote { start, end, .. } | UserTextNode::Text { start, end, .. } => {
                let mut units = text.encode_utf16();
                let selected = units
                    .by_ref()
                    .skip(*start)
                    .take(end - start)
                    .collect::<Vec<_>>();
                String::from_utf16(&selected).unwrap()
            }
        })
        .collect::<String>();
    assert_eq!(source, text);
}

#[test]
fn rule_respects_from_and_keeps_nested_markers_literal() {
    let text = "before\n\n> first\n> line\n\n> second\n\nafter";
    assert_eq!(
        quote_rule_match(text, 10),
        Some(UserTextNode::Text {
            start: 22,
            end: 24,
            text: "\n\n".into(),
            display: Some(" ".into()),
        })
    );
    assert_eq!(
        quote_rule_match(text, 23),
        Some(UserTextNode::Quote {
            start: 24,
            end: 32,
            text: "second".into(),
        })
    );
    assert_eq!(quote_rule_match(text, 34), None);
    assert_eq!(
        quote_rule_match("> > nested", 0),
        Some(UserTextNode::Quote {
            start: 0,
            end: 10,
            text: "> nested".into(),
        })
    );
}

#[test]
fn quote_node_keeps_credentials_references_and_markdown_literal() {
    let excerpt = "sk-1234567890abcdefghijklmnopqrstuvwxyz @notes.md /review **literal**";
    assert_eq!(
        quote_rule_match(&format!("> {excerpt}"), 0),
        Some(UserTextNode::Quote {
            start: 0,
            end: excerpt.encode_utf16().count() + 2,
            text: excerpt.into(),
        })
    );
}

#[test]
fn selection_action_clears_and_dismisses_only_after_success() {
    let enabled = SessionQuoteState::default();
    let rejected = activate_quote_action(enabled, Some("Hello world"), false);
    assert_eq!(rejected.insert_text.as_deref(), Some("Hello world"));
    assert!(!rejected.clear_selection);
    assert!(!rejected.dismiss);

    let inserted = activate_quote_action(enabled, Some("Hello world"), true);
    assert!(inserted.clear_selection);
    assert!(inserted.dismiss);
}

#[test]
fn selection_action_dismisses_invalidated_or_unwritable_sources() {
    let missing = activate_quote_action(SessionQuoteState::default(), None, true);
    assert!(missing.visible);
    assert!(missing.dismiss);
    assert!(missing.insert_text.is_none());

    let removed = activate_quote_action(
        SessionQuoteState {
            removed: true,
            subagent_mode: None,
        },
        Some("stale"),
        true,
    );
    assert!(!removed.visible);
    assert!(removed.dismiss);
    assert!(removed.insert_text.is_none());
}

#[test]
fn selection_allows_continuable_subagents_and_rejects_one_shot_records() {
    assert!(quote_selection_enabled(SessionQuoteState {
        removed: false,
        subagent_mode: Some(SubagentMode::Continuable),
    }));
    assert!(!quote_selection_enabled(SessionQuoteState {
        removed: false,
        subagent_mode: Some(SubagentMode::OneShot),
    }));
    assert!(!quote_selection_enabled(SessionQuoteState {
        removed: false,
        subagent_mode: Some(SubagentMode::Unknown),
    }));
}

#[test]
fn browser_interaction_opens_only_readable_selections_in_writable_sessions() {
    let writable = SessionQuoteState::default();
    assert!(admit_quote_interaction(
        QuoteInteractionTrigger::Selection,
        Some("selected text"),
        Some(writable),
    ));
    assert!(!admit_quote_interaction(
        QuoteInteractionTrigger::ContextMenu,
        Some("selected text"),
        Some(writable),
    ));
    assert!(!admit_quote_interaction(
        QuoteInteractionTrigger::Selection,
        None,
        Some(writable),
    ));
    assert!(!admit_quote_interaction(
        QuoteInteractionTrigger::Selection,
        Some("selected text"),
        None,
    ));
}

#[test]
fn locale_namespace_and_copy_are_exact() {
    assert_eq!(MESSAGE_QUOTES_NAMESPACE, "messageQuotes");
    assert_eq!(quote_add_label("zh"), Some("引用文本"));
    assert_eq!(quote_add_label("en"), Some("Quote text"));
    assert_eq!(quote_add_label("ja"), Some("テキストを引用"));
    assert_eq!(quote_add_label("es"), Some("Texto de cotización"));
    assert_eq!(quote_add_label("fr"), Some("Texte de citation"));
    assert_eq!(quote_add_label("de"), Some("Zitattext"));
    assert_eq!(quote_add_label("ru"), Some("Текст цитаты"));
    assert_eq!(quote_add_label("pt"), Some("Citação de texto"));
    assert_eq!(quote_add_label("unknown"), None);
}

#[test]
fn client_registration_metadata_is_exact() {
    assert_eq!(NAME, "message-quotes");
    assert_eq!(INJECT, ["slots", "locale", "sessions", "uiConversation"]);
    assert_eq!(QUOTE_INTERACTION_ID, "quote");
    assert_eq!(QUOTE_RULE_ORDER, -10);
    assert_eq!(QUOTE_ACTION_SLOT, "conversation.chat.text-interaction");
    assert_eq!(QUOTED_TEXT_SLOT, "conversation.user.text.node");
    assert_eq!(QUOTED_TEXT_PRIORITY, 100);
}

#[test]
fn host_entry_has_no_service_or_hook_requirements() {
    apply();
}
