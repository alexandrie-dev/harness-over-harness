mod common;
use common::*;
use hoh_llm::*;
use serde_json::{Value, json};
use std::{borrow::Cow, cell::RefCell, sync::Arc};
fn project_tool_updates<'a>(
    messages: &'a [RequestMessage],
    tools: Option<&'a [ToolSchema]>,
    mode: Option<&str>,
    history: Option<&ToolHistory>,
) -> ProjectedToolUpdates<'a> {
    hoh_llm::project_tool_updates(messages, tools, mode, history).unwrap()
}
fn result(content: Vec<ContentBlock>) -> RequestMessage {
    create_tool_result_message(ToolResultMessageInput {
        call_id: "shot".into(),
        content,
        is_error: false,
    })
    .into()
}
fn add(name: &str) -> ContentBlock {
    ToolAdditionBlock {
        tool_name: name.into(),
    }
    .into()
}
fn remove(name: &str) -> ContentBlock {
    ToolRemovalBlock {
        tool_name: name.into(),
    }
    .into()
}
fn roles(messages: &[RequestMessage]) -> Value {
    json!(
        messages
            .iter()
            .filter(|m| m.role() == "developer")
            .map(|m| value(m.content()))
            .collect::<Vec<_>>()
    )
}
fn deferred(name: &str) -> ToolSchema {
    ToolSchema {
        defer_loading: Some(true),
        ..tool(name)
    }
}
#[test]
fn visits_image_occurrences_in_user_and_tool_messages_in_message_order() {
    let seen = RefCell::new(vec![]);
    let messages = vec![
        user(vec![ContentBlock::text("before"), image(1, false)]),
        result(vec![ContentBlock::text("x"), image(2, false)]),
        result(vec![image(4, false)]),
        user(vec![image(3, false)]),
    ];
    let budget = decode(json!({"representation":"raw","maxImages":0}));
    assert_eq!(
        required_image_offload(&messages, &budget, |b| {
            seen.borrow_mut().push(b.attachment.bytes);
            b.attachment.bytes
        }),
        4
    );
    assert_eq!(*seen.borrow(), [1, 2, 4, 3]);
}
#[test]
fn keeps_request_only_inputs_identity_free_when_replacing_image_content() {
    let input: RequestMessage = decode(json!({"role":"user","content":[value(image(3,true))]}));
    let messages = vec![input.clone()];
    assert_eq!(
        value(project_offloaded_images(&messages, |_| "[omitted]".into()).as_ref()),
        json!([{"role":"user","content":[{"type":"text","text":"[omitted]"}]}])
    );
    assert_eq!(
        value(project_images_for_text_model(&messages).as_ref())[0]["content"][0]["text"],
        "[image omitted because this model accepts text only; attachment sha256:aaaaaaaa]"
    );
    assert_eq!(input.content(), [image(3, true)]);
}
#[test]
fn keeps_messages_without_offloaded_occurrences_by_identity() {
    let m = vec![user(vec![image(300, false)])];
    assert!(matches!(
        project_offloaded_images(&m, |_| "[omitted]".into()),
        Cow::Borrowed(_)
    ));
}
#[test]
fn replaces_marked_user_and_tool_occurrences_with_route_placeholders() {
    let m = vec![
        result(vec![image(3, true)]),
        user(vec![image(3, true), image(3, false)]),
    ];
    let p = project_offloaded_images(&m, |r| format!("[omitted]:{}", r.bytes));
    assert_eq!(p[0].content(), [ContentBlock::text("[omitted]:3")]);
    assert_eq!(
        p[1].content(),
        [ContentBlock::text("[omitted]:3"), image(3, false)]
    );
    assert_eq!(m[1].content()[0], image(3, true));
}
#[test]
fn keeps_unchanged_tool_content_while_replacing_a_later_image() {
    let unchanged = result(vec![ContentBlock::text("kept")]);
    let m = vec![unchanged.clone(), user(vec![image(3, true)])];
    let p = project_offloaded_images(&m, |_| "[omitted]".into());
    match (&p[0], &unchanged) {
        (RequestMessage::Durable(a), RequestMessage::Durable(b)) => assert!(Arc::ptr_eq(a, b)),
        _ => panic!("durable message"),
    };
    assert_eq!(p[1].content(), [ContentBlock::text("[omitted]")]);
}
fn offload(lengths: &[u64], offloaded: &[usize], budget: Value) -> u64 {
    let m = vec![user(
        lengths
            .iter()
            .enumerate()
            .map(|(i, b)| image(*b, offloaded.contains(&i)))
            .collect(),
    )];
    required_image_offload(&m, &decode(budget), |b| b.attachment.bytes)
}
#[test]
fn removes_nothing_under_unbounded_budgets_and_whole_quanta_past_them() {
    for b in [
        json!({"representation":"raw"}),
        json!({"representation":"raw","maxBytes":16}),
        json!({"representation":"raw","maxImages":4}),
    ] {
        assert_eq!(offload(&[4, 4, 4, 4], &[], b), 0);
    }
    assert_eq!(
        offload(
            &[4, 4, 4, 4, 4],
            &[],
            json!({"representation":"raw","maxImages":4,"countQuantum":2})
        ),
        2
    );
    assert_eq!(
        offload(
            &[4, 4, 4, 4, 1],
            &[],
            json!({"representation":"raw","maxBytes":16,"byteQuantum":5})
        ),
        2
    );
    let mib = 1024 * 1024;
    assert_eq!(
        offload(
            &vec![mib; 129],
            &[],
            json!({"representation":"raw","maxBytes":128*mib,"byteQuantum":64*mib})
        ),
        65
    );
}
#[test]
fn skips_offloaded_occurrences_and_accounts_inline_bytes_by_their_base64_length() {
    let b = json!({"representation":"base64","maxBytes":8});
    assert_eq!(offload(&[3, 3, 3], &[], b.clone()), 1);
    assert_eq!(offload(&[3, 3, 3], &[0], b), 0);
}
#[test]
fn describes_the_request_preview_immutable_normalized_path_and_source_uncertainty() {
    let reference = decode(
        json!({"attachmentId":format!("sha256:{}","b".repeat(64)),"mediaType":"image/png","bytes":4000,"width":2048,"height":1536,"name":"source \"map\".png"}),
    );
    let text = request_image_handle_text(
        &reference,
        923,
        692,
        Some(&ImageAttachmentAccess {
            readonly_path: "/tmp/.dsh/attachments/v1/objects/bb/object".into(),
        }),
    );
    assert_eq!(
        text,
        format!(
            "Image \"source \\\"map\\\".png\" (sha256:{}); request preview 923x692px. Normalized copy (read-only; may be resized or re-encoded): \"/tmp/.dsh/attachments/v1/objects/bb/object\" (2048x1536px, image/png). Source dimensions, format, and byte size may differ. Copy to a writable path ending in .png before editing.",
            "b".repeat(64)
        )
    );
}
#[test]
fn bridges_a_provider_host_object_only_through_the_mounted_filesystem_mapping() {
    let r = image(1, false).image().unwrap().attachment;
    let store = TestAttachments::new(Ok(Some("/host/.dsh/attachments/object".into())));
    assert_eq!(
        resolve_image_attachment_access(
            &store,
            |p| if p == "/host/.dsh/attachments/object" {
                Some("/workspace/.attachments/object".into())
            } else {
                None
            },
            &r
        )
        .unwrap(),
        Some(ImageAttachmentAccess {
            readonly_path: "/workspace/.attachments/object".into()
        })
    );
    assert!(
        resolve_image_attachment_access(&store, |_| None, &r)
            .unwrap()
            .is_none()
    );
    assert!(
        resolve_image_attachment_access(
            &TestAttachments::new(Ok(None)),
            |_| Some("unexpected".into()),
            &r
        )
        .unwrap()
        .is_none()
    );
}
#[test]
fn names_each_occurrence_from_its_own_reference_when_one_prepared_version_is_shared() {
    let mut r = image(4000, false).image().unwrap().attachment;
    r.name = Some("first.png".into());
    assert!(request_image_handle_text(&r, 8, 8, None).contains("\"first.png\""));
}
#[test]
fn keeps_a_useful_omission_identity_with_and_without_a_local_path() {
    let r = decode(
        json!({"attachmentId":format!("sha256:{}","d".repeat(64)),"mediaType":"image/jpeg","bytes":10,"width":10,"height":5,"name":"photo.jpg"}),
    );
    assert!(offloaded_image_text(&r, None).contains("No local normalized image path is available"));
    assert_eq!(
        offloaded_image_text(
            &r,
            Some(&ImageAttachmentAccess {
                readonly_path: "/tmp/object".into()
            })
        ),
        format!(
            "[image omitted to fit request image limits; \"photo.jpg\" (sha256:{}). Normalized copy (read-only; may be resized or re-encoded): \"/tmp/object\" (10x5px, image/jpeg). Source dimensions, format, and byte size may differ. Copy to a writable path ending in .jpg before editing.]",
            "d".repeat(64)
        )
    );
}
#[test]
fn names_the_writable_extension_for_each_media_type() {
    for (mime, ext) in [
        ("image/png", ".png"),
        ("image/jpeg", ".jpg"),
        ("image/webp", ".webp"),
        ("image/gif", ".gif"),
    ] {
        let r = decode(
            json!({"attachmentId":format!("sha256:{}","e".repeat(64)),"mediaType":mime,"bytes":1,"width":1,"height":1}),
        );
        assert!(
            offloaded_image_text(
                &r,
                Some(&ImageAttachmentAccess {
                    readonly_path: "/tmp/object".into()
                })
            )
            .contains(&format!("writable path ending in {ext}"))
        );
    }
}
#[test]
fn returns_image_free_history_unchanged() {
    for m in [
        vec![user(vec![ContentBlock::text("plain")])],
        vec![result(vec![ContentBlock::text("plain")])],
    ] {
        assert!(matches!(
            project_images_for_text_model(&m),
            Cow::Borrowed(_)
        ));
    }
}
#[test]
fn replaces_direct_images_while_retaining_unaffected_messages() {
    let m = vec![
        user(vec![ContentBlock::text("plain")]),
        user(vec![ContentBlock::text("lead"), image(3, false)]),
        result(vec![ContentBlock::text("unchanged")]),
        result(vec![
            ContentBlock::text("before"),
            image(3, false),
            ContentBlock::text("after"),
        ]),
    ];
    let p = project_images_for_text_model(&m);
    assert_eq!(p[0], m[0]);
    assert_eq!(p[2], m[2]);
    let placeholder = ContentBlock::text(
        "[image omitted because this model accepts text only; attachment sha256:aaaaaaaa]",
    );
    assert_eq!(
        p[1].content(),
        [ContentBlock::text("lead"), placeholder.clone()]
    );
    assert_eq!(
        p[3].content(),
        [
            ContentBlock::text("before"),
            placeholder,
            ContentBlock::text("after")
        ]
    );
}
#[test]
fn keeps_request_only_inputs_identity_free_when_replacing_file_content() {
    let input: RequestMessage = decode(json!({"role":"user","content":[value(file())]}));
    let m = vec![input.clone()];
    let p = project_files_to_text(&m, |_| Some("/copies/review.txt".into()));
    assert!(p[0].id().is_none());
    assert_eq!(p[0].content()[0].r#type, "text");
    assert_eq!(input.content(), [file()]);
}
#[test]
fn detects_file_blocks_anywhere_in_message_content() {
    assert!(!content_has_file(&[ContentBlock::text("x")]));
    assert!(content_has_file(&[file()]));
}
#[test]
fn scans_frozen_blocks_and_observes_later_mutations_in_file_free_content() {
    let mut c = vec![ContentBlock::text("plain")];
    assert!(!content_has_file(&[]));
    assert!(!content_has_file(&c));
    c.push(file());
    assert!(content_has_file(&c));
    c.pop();
    assert!(!content_has_file(&c));
    c.push(file());
    assert!(content_has_file(&c));
}
#[test]
fn renders_the_handle_with_the_read_path_or_the_explicit_no_path_fallback() {
    let r = file().file().unwrap().attachment;
    let with = file_handle_text(&r, Some("/home/.dsh/attachments/v1/files/ab/x/notes.pdf"));
    for s in [
        "\"notes.pdf\"",
        "42 bytes",
        "sha256:abababab",
        "\"/home/.dsh/attachments/v1/files/ab/x/notes.pdf\"",
        "include this saved path in the delegation prompt",
        "only subagents sharing this execution environment can read it",
    ] {
        assert!(with.contains(s));
    }
    let without = file_handle_text(&r, None);
    assert!(without.contains("current execution environment cannot access a readable path"));
    assert!(without.contains("do not claim to have read it"));
}
#[test]
fn replaces_every_file_occurrence_with_handle_text_and_keeps_file_free_history_identical() {
    let plain = vec![user(vec![ContentBlock::text("hi")])];
    assert!(matches!(
        project_files_to_text(&plain, |_| Some("/p".into())),
        Cow::Borrowed(_)
    ));
    let m = vec![
        plain[0].clone(),
        user(vec![file(), ContentBlock::text("keep")]),
        result(vec![file()]),
    ];
    let p = project_files_to_text(&m, |r| Some(format!("/copies/{}", r.name)));
    assert_eq!(p[0], m[0]);
    assert_eq!(p[1].content()[1], ContentBlock::text("keep"));
    assert_eq!(p[1].content()[0].r#type, "text");
    assert_eq!(p[2].content()[0].r#type, "text");
    assert_eq!(m[1].content()[0].r#type, "file");
    assert_eq!(m[2].content()[0].r#type, "file");
}
#[test]
fn returns_the_input_history_and_tools_by_identity_when_nothing_is_projected() {
    let m = vec![user(vec![ContentBlock::text("hi")])];
    let tools = vec![tool("search")];
    let h = ToolHistory {
        tools: tools.clone(),
        updates: vec![],
    };
    let p = project_tool_updates(&m, Some(&tools), Some("in-history"), Some(&h));
    assert!(matches!(p.messages, Cow::Borrowed(_)));
    assert_eq!(p.tools.unwrap().as_ref(), tools);
    assert!(matches!(
        project_tool_updates(&m, Some(&tools), None, None).messages,
        Cow::Borrowed(_)
    ));
}
#[test]
fn strips_deferred_loading_and_developer_updates_when_the_route_declares_no_mode() {
    let added = developer(vec![add("fetch")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![update(&added, vec![tool("fetch")])],
    };
    let m = vec![user(vec![]), added, user(vec![])];
    let t = vec![tool("search"), deferred("fetch")];
    let p = project_tool_updates(&m, Some(&t), None, Some(&h));
    assert_eq!(p.tools.unwrap().as_ref(), [tool("search"), tool("fetch")]);
    assert_eq!(p.messages.len(), 2);
    let plain = vec![tool("search")];
    assert!(matches!(
        project_tool_updates(&m, Some(&plain), None, None).tools,
        Some(Cow::Borrowed(_))
    ));
    assert!(project_tool_updates(&m, None, None, None).tools.is_none());
}
#[test]
fn activates_explicitly_deferred_baseline_tools_on_both_routes() {
    for mode in ["in-history", "addition-only"] {
        let added = developer(vec![add("search")]);
        let duplicate = developer(vec![add("search")]);
        let removed = developer(vec![remove("search")]);
        let restored = developer(vec![add("search")]);
        let h = ToolHistory {
            tools: vec![deferred("search")],
            updates: vec![
                update(&added, vec![deferred("search")]),
                update(&duplicate, vec![deferred("search")]),
                update(&removed, vec![]),
                update(&restored, vec![deferred("search")]),
            ],
        };
        let m = vec![
            user(vec![]),
            added.clone(),
            duplicate,
            removed.clone(),
            restored.clone(),
        ];
        let p = project_tool_updates(&m, None, Some(mode), Some(&h));
        if mode == "in-history" {
            assert_eq!(p.tools.unwrap().as_ref(), [deferred("search")]);
            assert_eq!(
                p.messages.as_ref(),
                [m[0].clone(), added, removed, restored]
            );
        } else {
            assert_eq!(p.tools.unwrap().as_ref(), [tool("search")]);
            assert_eq!(p.messages.len(), 1);
        }
    }
}
#[test]
fn rebuilds_when_adding_the_first_tools_to_an_empty_set() {
    for removed in [false, true] {
        let removal = developer(vec![remove("search")]);
        let added = developer(vec![
            add("fetch"),
            ContentBlock::text("Keep this guidance."),
        ]);
        let later = developer(vec![add("search")]);
        let mut h = ToolHistory {
            tools: if removed {
                vec![tool("search")]
            } else {
                vec![]
            },
            updates: vec![],
        };
        let mut m = vec![user(vec![])];
        if removed {
            h.updates.push(update(&removal, vec![]));
            m.push(removal);
        }
        h.updates.extend([
            update(&added, vec![tool("fetch")]),
            update(&later, vec![tool("search")]),
        ]);
        m.extend([added, later]);
        let saved = value(&h);
        let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
        assert_eq!(
            p.tools.unwrap().as_ref(),
            [tool("fetch"), deferred("search")]
        );
        assert_eq!(
            roles(&p.messages),
            json!([[{"type":"text","text":"Keep this guidance."}],[{"type":"tool-addition","toolName":"search"}]])
        );
        assert_eq!(value(&h), saved);
    }
}
#[test]
fn keeps_additions_deferred_when_an_unactivated_deferred_declaration_already_exists() {
    let added = developer(vec![add("fetch")]);
    let h = ToolHistory {
        tools: vec![deferred("search")],
        updates: vec![update(&added, vec![tool("fetch")])],
    };
    let m = vec![user(vec![]), added];
    let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
    assert_eq!(
        p.tools.unwrap().as_ref(),
        [deferred("search"), deferred("fetch")]
    );
    assert!(matches!(p.messages, Cow::Borrowed(_)));
}
#[test]
fn uses_current_declarations_without_updates_when_history_is_missing_or_the_prefix_omits_an_update()
{
    let added = developer(vec![add("fetch")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![update(&added, vec![tool("fetch")])],
    };
    let tools = vec![tool("search"), tool("fetch")];
    for (m, history) in [
        (vec![user(vec![]), added], None),
        (vec![user(vec![])], Some(&h)),
    ] {
        let p = project_tool_updates(&m, Some(&tools), Some("in-history"), history);
        assert!(matches!(p.tools, Some(Cow::Borrowed(_))));
        assert_eq!(p.messages.len(), 1);
    }
}
#[test]
fn omits_updates_from_an_earlier_declaration_series() {
    let earlier = developer(vec![add("search")]);
    let current = developer(vec![add("fetch")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![update(&current, vec![tool("fetch")])],
    };
    let m = vec![user(vec![]), earlier, current.clone()];
    let p = project_tool_updates(&m, None, Some("in-history"), Some(&h));
    assert_eq!(p.messages.as_ref(), [m[0].clone(), current]);
    assert_eq!(
        p.tools.unwrap().as_ref(),
        [tool("search"), deferred("fetch")]
    );
}
#[test]
fn defers_historical_additions_and_retains_removed_definitions_for_in_history_routes() {
    let added = developer(vec![add("fetch"), ContentBlock::text("fetch is available")]);
    let removed = developer(vec![remove("search")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![
            update(&added, vec![tool("fetch")]),
            update(&removed, vec![]),
        ],
    };
    let m = vec![user(vec![]), added.clone(), removed.clone()];
    let p = project_tool_updates(&m, None, Some("in-history"), Some(&h));
    assert_eq!(
        p.tools.unwrap().as_ref(),
        [tool("search"), deferred("fetch")]
    );
    assert_eq!(p.messages.as_ref(), m);
}
#[test]
fn omits_removed_definitions_and_removal_blocks_for_addition_only_routes() {
    let swapped = developer(vec![add("fetch"), remove("search")]);
    let removed = developer(vec![remove("fetch")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![
            update(&swapped, vec![tool("fetch")]),
            update(&removed, vec![]),
        ],
    };
    let m = vec![
        user(vec![]),
        swapped.clone(),
        user(vec![]),
        removed,
        user(vec![]),
    ];
    let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
    assert!(p.tools.unwrap().is_empty());
    assert_eq!(p.messages.len(), 3);
    let short = ToolHistory {
        tools: h.tools.clone(),
        updates: vec![h.updates[0].clone()],
    };
    let m = vec![user(vec![]), swapped, user(vec![])];
    let p = project_tool_updates(&m, None, Some("addition-only"), Some(&short));
    assert_eq!(p.tools.unwrap().as_ref(), [tool("fetch")]);
    assert_eq!(p.messages.len(), 2);
}
#[test]
fn re_offers_an_unchanged_restored_tool_through_its_recorded_blocks() {
    let removed = developer(vec![remove("search")]);
    let restored = developer(vec![add("search")]);
    let h = ToolHistory {
        tools: vec![tool("search"), tool("fetch")],
        updates: vec![
            update(&removed, vec![]),
            update(&restored, vec![tool("search")]),
        ],
    };
    let m = vec![user(vec![]), removed, restored];
    let p = project_tool_updates(&m, None, Some("in-history"), Some(&h));
    assert_eq!(p.tools.unwrap().as_ref(), [tool("search"), tool("fetch")]);
    assert_eq!(roles(&p.messages).as_array().unwrap().len(), 2);
    let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
    assert_eq!(
        p.tools.unwrap().as_ref(),
        [tool("fetch"), deferred("search")]
    );
    assert_eq!(
        roles(&p.messages),
        json!([[{"type":"tool-addition","toolName":"search"}]])
    );
}
#[test]
fn declares_a_deferred_tool_once_across_removal_and_re_addition() {
    let added = developer(vec![add("fetch")]);
    let removed = developer(vec![remove("fetch")]);
    let restored = developer(vec![add("fetch")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![
            update(&added, vec![tool("fetch")]),
            update(&removed, vec![]),
            update(&restored, vec![tool("fetch")]),
        ],
    };
    let m = vec![user(vec![]), added, removed, restored];
    let p = project_tool_updates(&m, None, Some("in-history"), Some(&h));
    assert_eq!(
        p.tools.unwrap().as_ref(),
        [tool("search"), deferred("fetch")]
    );
    assert_eq!(roles(&p.messages).as_array().unwrap().len(), 3);
    let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
    assert_eq!(
        roles(&p.messages),
        json!([[{"type":"tool-addition","toolName":"fetch"}]])
    );
}
#[test]
fn rebases_on_a_baseline_removal_and_defers_additions_after_the_reset() {
    let added = developer(vec![
        add("fetch"),
        ContentBlock::text("Keep this guidance."),
    ]);
    let removed = developer(vec![remove("search"), ContentBlock::text("Keep this too.")]);
    let next = developer(vec![add("later")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![
            update(&added, vec![tool("fetch")]),
            update(&removed, vec![]),
            update(&next, vec![tool("later")]),
        ],
    };
    let m = vec![user(vec![]), added, removed, next];
    let saved = value((&m, &h));
    let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
    assert_eq!(
        p.tools.unwrap().as_ref(),
        [tool("fetch"), deferred("later")]
    );
    assert_eq!(
        roles(&p.messages),
        json!([[{"type":"text","text":"Keep this guidance."}],[{"type":"text","text":"Keep this too."}],[{"type":"tool-addition","toolName":"later"}]])
    );
    assert_eq!(value((&m, &h)), saved);
}
#[test]
fn rebuilds_after_removing_a_later_activated_tool() {
    for replace in [false, true] {
        let added = developer(vec![add("fetch"), add("retained")]);
        let mut blocks = vec![];
        if replace {
            blocks.push(add("replacement"));
        }
        blocks.push(remove("fetch"));
        let swapped = developer(blocks);
        let next = developer(vec![add("later")]);
        let h = ToolHistory {
            tools: vec![tool("search")],
            updates: vec![
                update(&added, vec![tool("fetch"), tool("retained")]),
                update(
                    &swapped,
                    if replace {
                        vec![tool("replacement")]
                    } else {
                        vec![]
                    },
                ),
                update(&next, vec![tool("later")]),
            ],
        };
        let m = vec![user(vec![]), added, swapped, next];
        let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
        let mut expected = vec![tool("search"), tool("retained")];
        if replace {
            expected.push(tool("replacement"));
        }
        expected.push(deferred("later"));
        assert_eq!(p.tools.unwrap().as_ref(), expected);
        assert_eq!(
            roles(&p.messages),
            json!([[{"type":"tool-addition","toolName":"later"}]])
        );
    }
}
#[test]
fn preserves_explicitly_deferred_baseline_activation_across_a_reset() {
    for activated in [false, true] {
        let activate = developer(vec![add("pending")]);
        let swapped = developer(vec![remove("search"), add("fetch")]);
        let mut h = ToolHistory {
            tools: vec![tool("search"), deferred("pending")],
            updates: vec![],
        };
        let mut m = vec![user(vec![])];
        if activated {
            h.updates.push(update(&activate, vec![deferred("pending")]));
            m.push(activate);
        }
        h.updates.push(update(&swapped, vec![tool("fetch")]));
        m.push(swapped);
        let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
        let expected = vec![
            if activated {
                tool("pending")
            } else {
                deferred("pending")
            },
            tool("fetch"),
        ];
        assert_eq!(p.tools.unwrap().as_ref(), expected);
        assert_eq!(roles(&p.messages), json!([]));
        let reoffered = developer(vec![add("pending")]);
        h.updates
            .push(update(&reoffered, vec![deferred("pending")]));
        m.push(reoffered);
        let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
        assert_eq!(p.tools.unwrap().as_ref(), expected);
        assert_eq!(
            roles(&p.messages),
            if activated {
                json!([])
            } else {
                json!([[{"type":"tool-addition","toolName":"pending"}]])
            }
        );
    }
}
#[test]
fn uses_each_rebuilt_baseline_to_detect_subsequent_replacements() {
    let first = developer(vec![remove("search"), add("fetch")]);
    let second = developer(vec![remove("fetch"), add("replacement")]);
    let h = ToolHistory {
        tools: vec![tool("search")],
        updates: vec![
            update(&first, vec![tool("fetch")]),
            update(&second, vec![tool("replacement")]),
        ],
    };
    let m = vec![user(vec![]), first, second];
    let p = project_tool_updates(&m, None, Some("addition-only"), Some(&h));
    assert_eq!(p.tools.unwrap().as_ref(), [tool("replacement")]);
    assert_eq!(roles(&p.messages), json!([]));
}

#[test]
fn rejects_an_addition_whose_historical_definition_is_missing() {
    let added = developer(vec![add("missing")]);
    let history = ToolHistory {
        tools: vec![],
        updates: vec![update(&added, vec![])],
    };
    assert!(
        hoh_llm::project_tool_updates(&[added], None, Some("addition-only"), Some(&history))
            .is_err()
    );
}
