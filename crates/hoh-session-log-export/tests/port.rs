use flate2::read::DeflateDecoder;
use futures_util::{
    future::BoxFuture,
    stream::{self, BoxStream},
};
use hoh_attachment::{
    AttachmentError, AttachmentErrorCode, AttachmentStore, ImageAttachmentLimits,
    ImageAttachmentRef, SaveImageAttachment, StoredImageAttachment,
};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Kernel;
use hoh_session::{CreateSessionOptions, SessionEvent, SessionHeader, SessionId, SessionStore};
use hoh_session_log_export::*;
use hoh_session_persistence::{
    SessionAccess, SessionHandle, SessionHandleAppendOptions, SessionHandleFlushOptions,
    SessionHandleReadOptions, SessionHandleReadResult, SessionPersistence,
    SessionPersistenceCreateOptions, SessionPersistenceIdentity, SessionPersistenceListOptions,
    SessionPersistenceNotFoundError, SessionPersistenceOpenOptions, SessionPersistenceSnapshot,
    SessionPersistenceStatOptions,
};
use hoh_session_query::{
    Config as QueryConfig, SessionEventSearchPage, SessionEventSearchRequest, SessionQueryEngine,
    SessionSearchExecContext, SessionSearchHit, SessionSearchPage, SessionSearchProvider,
    SessionSearchRequest,
};
use hoh_timeout::{AbortController, AbortError};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

fn event(value: Value) -> SessionEvent {
    SessionEvent::from_value(value)
}

fn append(kind: &str, data: Value) -> SessionEvent {
    event(json!({"type":kind,"seq":0,"time":0,"surfaceOp":"append","data":data}))
}

fn create_lineage_session(
    store: &SessionStore,
    ctx: hoh_kernel::Context,
    id: &str,
    parent: Option<&str>,
    created_at: u64,
) -> SessionHeader {
    let mut meta = json!({"createdAt": created_at});
    if let Some(parent) = parent {
        meta["parentSession"] = Value::String(parent.to_owned());
    }
    let session = store
        .create(
            &ctx,
            Some(SessionId::from(id)),
            CreateSessionOptions {
                meta: Some(meta),
                ..Default::default()
            },
        )
        .unwrap();
    serde_json::from_value((*session.header()).clone()).unwrap()
}

#[test]
fn routes_and_command_match_the_host_contract() {
    assert_eq!(SESSION_LOG_EXPORT_PATH, "/api/session.export");
    assert_eq!(SESSION_LOG_EXPORT_ROUTE, "api/session.export");
    assert_eq!(SESSION_MARKDOWN_EXPORT_PATH, "/api/session.markdown");
    assert_eq!(export_command(" \n\t"), Ok(EXPORT_REQUESTED_TEXT));
    assert_eq!(export_command("\u{feff}"), Ok(EXPORT_REQUESTED_TEXT));
    assert_eq!(export_command("./out"), Err(EXPORT_ARGUMENT_ERROR_TEXT));
}

#[test]
fn route_query_validation_is_strict() {
    assert_eq!(
        validate_session_log_export_query(None, None),
        Err(INVALID_SESSION_QUERY_TEXT)
    );
    assert_eq!(
        validate_session_log_export_query(Some(""), None),
        Err(INVALID_SESSION_QUERY_TEXT)
    );
    assert_eq!(
        validate_session_log_export_query(Some("s"), Some("1")),
        Err(INVALID_SESSION_QUERY_TEXT)
    );
    assert!(
        !validate_session_log_export_query(Some("s"), None)
            .unwrap()
            .include_descendants
    );
    assert!(
        validate_session_log_export_query(Some("s"), Some("true"))
            .unwrap()
            .include_descendants
    );
    assert_eq!(validate_session_markdown_query(Some("s")), Ok("s"));
}

#[test]
fn client_routes_are_document_relative_and_url_encoded() {
    assert_eq!(
        download_route("a/b c"),
        "api/session.export?sessionId=a%2Fb+c&includeDescendants=true"
    );
    assert_eq!(
        markdown_route("a/b c"),
        "api/session.markdown?sessionId=a%2Fb+c"
    );
}

#[test]
fn compression_config_defaults_and_rejects_non_integer_or_out_of_range() {
    assert_eq!(resolve_compression_level(None), Ok(6));
    for level in 0..=9 {
        assert_eq!(resolve_compression_level(Some(f64::from(level))), Ok(level));
    }
    for invalid in [-1.0, 1.5, 10.0, f64::NAN, f64::INFINITY] {
        assert!(resolve_compression_level(Some(invalid)).is_err());
    }
}

#[test]
fn session_and_attachment_paths_are_safe_and_stable() {
    assert_eq!(safe_session_id_segment("../x/💥"), "___x___");
    assert_eq!(session_log_zip_filename("../x"), "dsh-session-___x.zip");
    let image = serde_json::from_value(json!({
        "attachmentId":"img", "mediaType":"image/jpeg", "bytes":1, "width":2, "height":3
    }))
    .unwrap();
    assert_eq!(media_entry_path(&image), "media/img.jpg");
    let file = serde_json::from_value(json!({
        "attachmentId":"sha256:abcdef", "name":"../a\\b\u{0000}", "bytes":1
    }))
    .unwrap();
    assert_eq!(file_entry_path(&file), "files/ab/abcdef/.._a_b_");
}

#[test]
fn canonical_jsonl_selects_physical_header_fields_and_trailing_newline() {
    let mut header = SessionHeader::new(SessionId::from("s"), 42);
    header.cwd = Some("/work".into());
    header.extra.insert("private".into(), json!(true));
    let events = [event(
        json!({"type":"turn/start","seq":0,"time":42,"data":{}}),
    )];
    let text = serialize_session_log(&header, &events).unwrap();
    assert_eq!(
        text,
        concat!(
            "{\"type\":\"session\",\"version\":4,\"id\":\"s\",\"createdAt\":42,\"cwd\":\"/work\",\"isSeeded\":false,\"delegationDepth\":0}\n",
            "{\"type\":\"turn/start\",\"seq\":0,\"time\":42,\"data\":{}}\n"
        )
    );
    assert_eq!(SESSION_LOG_FILENAME.as_str(), "session.v4.jsonl");
}

#[test]
fn canonical_jsonl_and_tool_arguments_use_javascript_number_rendering() {
    let header = SessionHeader::new(SessionId::from("s"), 1);
    let events = [event(json!({
        "type":"plugin/record","seq":0,"time":1,"data":{"float":1.0,"large":9007199254740993_u64}
    }))];
    let text = serialize_session_log(&header, &events).unwrap();
    assert!(text.contains("\"float\":1"));
    assert!(text.contains("\"large\":9007199254740992"));

    let tool = [event(json!({"type":"tool/call","data":{
        "callId":"1","name":"n","arguments":"{\"float\":1.0,\"large\":9007199254740993}"
    }}))];
    let markdown = format_session_markdown(&tool, None);
    assert!(markdown.contains("  \"float\": 1,"));
    assert!(markdown.contains("  \"large\": 9007199254740992"));
}

#[test]
fn markdown_uses_latest_title_and_one_final_newline() {
    let events = [
        event(json!({"type":"session/title","data":{"title":"old"}})),
        event(json!({"type":"session/title","data":{"title":"  latest\r\ntitle  "}})),
    ];
    assert_eq!(format_session_markdown(&events, None), "# latest title\n");
    assert_eq!(
        format_session_markdown(&[] as &[SessionEvent], None),
        "# Untitled Session\n"
    );
}

#[test]
fn markdown_source_path_is_verbatim_in_a_grown_fence() {
    let markdown = format_session_markdown(&[] as &[SessionEvent], Some("/tmp/```/session"));
    assert_eq!(
        markdown,
        "# Untitled Session\n\nSession file:\n\n````text\n/tmp/```/session\n````\n"
    );
}

#[test]
fn markdown_quotes_users_and_keeps_only_last_plain_assistant_as_final() {
    let events = [
        event(json!({"type":"turn/start","data":{}})),
        append(
            "user/message",
            json!({"source":{"kind":"user"},"content":[{"type":"text","text":"hello\n\nworld\n"}]}),
        ),
        append(
            "assistant/message",
            json!({"message":{"content":[{"type":"text","text":"draft"},{"type":"tool-call","id":"c"}]}}),
        ),
        append(
            "assistant/message",
            json!({"message":{"content":[{"type":"text","text":"final"}]}}),
        ),
    ];
    let markdown = format_session_markdown(&events, None);
    assert!(markdown.contains("> hello\n>\n> world"));
    assert!(markdown.contains(
        "<details><summary>1 previous message</summary>\n\n**Assistant**\n\ndraft\n\n</details>"
    ));
    assert!(markdown.ends_with("\n\nfinal\n"));
}

#[test]
fn steering_message_flushes_prior_activity_before_it() {
    let events = [
        append(
            "assistant/message",
            json!({"message":{"content":[{"type":"text","text":"working"},{"type":"tool-call"}]}}),
        ),
        append(
            "user/message",
            json!({"source":{"kind":"user"},"content":[{"type":"text","text":"steer"}]}),
        ),
        append(
            "assistant/message",
            json!({"message":{"content":[{"type":"text","text":"done"}]}}),
        ),
    ];
    let markdown = format_session_markdown(&events, None);
    let detail = markdown.find("<details>").unwrap();
    let user = markdown.find("> steer").unwrap();
    let final_reply = markdown.rfind("done").unwrap();
    assert!(detail < user && user < final_reply);
}

#[test]
fn tools_render_arguments_results_and_orphans() {
    let events = [
        event(
            json!({"type":"tool/call","data":{"callId":"1","name":"read","arguments":"{\"a\":1}"}}),
        ),
        append(
            "tool/result",
            json!({"message":{"toolCallId":"1","content":[{"type":"text","text":"ok"}],"isError":false}}),
        ),
        append(
            "tool/result",
            json!({"message":{"toolCallId":"2","content":[],"isError":true},"error":{"reason":"denied"}}),
        ),
    ];
    let markdown = format_session_markdown(&events, None);
    assert!(markdown.contains("**Tool: `read`**\n\n```json\n{\n  \"a\": 1\n}\n```"));
    assert!(markdown.contains("**Tool: `unknown`**\n\n(no arguments)"));
    assert!(markdown.contains("**Result (error)**\n\nError: denied"));
}

#[test]
fn markdown_closes_unfinished_fences_and_escapes_generated_wrapper_breakouts() {
    let events = [
        append(
            "assistant/message",
            json!({"message":{"content":[
                {"type":"text","text":"outside </details>\n```md\ninside </details>"},
                {"type":"tool-call"}
            ]}}),
        ),
        append(
            "assistant/message",
            json!({"message":{"content":[{"type":"text","text":"```txt\nfinal"}]}}),
        ),
    ];
    let markdown = format_session_markdown(&events, None);
    assert!(markdown.contains("outside &lt;/details>\n```md\ninside </details>\n```"));
    assert!(markdown.ends_with("```txt\nfinal\n```\n"));
}

#[test]
fn markdown_does_not_open_a_fence_across_javascript_line_separators() {
    let events = [append(
        "assistant/message",
        json!({"message":{"content":[{"type":"text","text":"```js\u{2028}x"}]}}),
    )];
    assert_eq!(
        format_session_markdown(&events, None),
        "# Untitled Session\n\n```js\u{2028}x\n"
    );
}

#[test]
fn markdown_labels_attachments_and_omits_hidden_message_kinds() {
    let events = [
        append(
            "system/message",
            json!({"message":{"content":[{"type":"text","text":"secret"}]}}),
        ),
        append(
            "user/message",
            json!({"source":{"kind":"user"},"content":[
                {"type":"image","attachment":{"attachmentId":"i","mediaType":"image/png","bytes":12,"width":3,"height":4,"name":"  a  ]  b "}},
                {"type":"file","attachment":{"attachmentId":"f","bytes":9,"name":""}}
            ]}),
        ),
    ];
    let markdown = format_session_markdown(&events, None);
    assert!(markdown.contains("[Image: a \\] b (3×4, 12 bytes)]"));
    assert!(markdown.contains("[File: attachment (9 bytes)]"));
    assert!(!markdown.contains("secret"));
}

#[test]
fn replacement_surface_events_are_ignored() {
    let events = [event(json!({
        "type":"user/message","surfaceOp":{"op":"replace","startSeq":0,"endSeq":0},
        "data":{"source":{"kind":"user"},"content":[{"type":"text","text":"replacement"}]}
    }))];
    assert_eq!(
        format_session_markdown(&events, None),
        "# Untitled Session\n"
    );
}

#[test]
fn attachment_collection_reads_only_declared_carriers() {
    let artifact = concat!(
        "{\"type\":\"user/message\",\"data\":{\"content\":[{\"type\":\"image\",\"attachment\":{\"attachmentId\":\"i\",\"mediaType\":\"image/png\",\"bytes\":1,\"width\":2,\"height\":3}}]}}\n",
        "{\"type\":\"unknown\",\"data\":{\"content\":[{\"type\":\"file\",\"attachment\":{\"attachmentId\":\"hidden\",\"name\":\"x\",\"bytes\":1}}]}}\n",
        "not json\n"
    );
    let refs = attachment_refs_in_artifact(artifact);
    assert_eq!(refs.images.len(), 1);
    assert_eq!(refs.images[0].attachment_id.to_string(), "i");
    assert!(refs.files.is_empty());
}

#[test]
fn attachment_collection_scans_inbox_summary_and_completed_stream_blocks() {
    let artifact = serde_json::json!({"type":"agent/inbox/spliced","data":{"inserted":[{"content":[{
        "type":"file","attachment":{"attachmentId":"sha256:aa","name":"a","bytes":1}
    }]}]}})
    .to_string()
        + "\n"
        + &serde_json::json!({"type":"assistant/attempt","data":{"stream":[{
            "type":"chunk","chunk":{"type":"block-end","block":{"type":"image","attachment":{
                "attachmentId":"i","mediaType":"image/webp","bytes":1,"width":1,"height":1
            }}}
        }]}})
        .to_string();
    let refs = attachment_refs_in_artifact(&artifact);
    assert_eq!(refs.files.len(), 1);
    assert_eq!(refs.images.len(), 1);
}

#[test]
fn attachment_collection_deduplicates_images_and_file_name_pairs() {
    let block = |name: &str| {
        json!({"type":"user/message","data":{"content":[
        {"type":"image","attachment":{"attachmentId":"i","mediaType":"image/png","bytes":1,"width":1,"height":1}},
        {"type":"file","attachment":{"attachmentId":"sha256:aa","name":name,"bytes":1}}
    ]}}).to_string()
    };
    let refs = attachment_refs_in_artifact(&(block("a") + "\n" + &block("a") + "\n" + &block("b")));
    assert_eq!(refs.images.len(), 1);
    assert_eq!(refs.files.len(), 2);
}

#[test]
fn zip_writer_preserves_order_names_contents_and_empty_entries() {
    let mut bytes = Vec::new();
    write_zip(
        &mut bytes,
        [
            ZipEntry::text("first.txt", "hello"),
            ZipEntry::bytes("empty", []),
        ],
        6,
    )
    .unwrap();
    let entries = read_local_zip_entries(&bytes);
    assert_eq!(
        entries,
        vec![
            ("first.txt".into(), b"hello".to_vec()),
            ("empty".into(), vec![])
        ]
    );
    assert!(
        bytes
            .windows(4)
            .any(|window| window == [0x50, 0x4b, 0x05, 0x06])
    );
}

#[test]
fn zip_writer_checks_config_before_writing() {
    let mut bytes = Vec::new();
    assert!(write_zip(&mut bytes, [], 10).is_err());
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn persistence_read_serializes_and_always_closes_the_handle() {
    let closed = Arc::new(AtomicBool::new(false));
    let persistence = FakePersistence::stored(closed.clone());
    let text = read_session_log_text(&persistence, &SessionId::from("s"), None)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("\"type\":\"session\""));
    assert!(text.contains("\"type\":\"turn/start\""));
    assert!(closed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn persistence_read_maps_only_not_found_to_absence() {
    let persistence = FakePersistence::not_found();
    assert!(
        read_session_log_text(&persistence, &SessionId::from("missing"), None)
            .await
            .unwrap()
            .is_none()
    );
    let persistence = FakePersistence::failure("disk secret");
    let error = read_session_log_text(&persistence, &SessionId::from("s"), None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "disk secret");
}

#[tokio::test]
async fn archive_entries_include_declared_images_and_streamed_files() {
    let kernel = Kernel::start();
    let query = SessionQueryEngine::new(
        kernel.root(),
        hoh_session::SessionStore::new(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let persistence = FakePersistence::not_found();
    let attachments = FakeAttachments::default();
    let root = concat!(
        "{\"type\":\"user/message\",\"data\":{\"content\":[",
        "{\"type\":\"image\",\"attachment\":{\"attachmentId\":\"img\",\"mediaType\":\"image/png\",\"bytes\":3,\"width\":1,\"height\":1}},",
        "{\"type\":\"file\",\"attachment\":{\"attachmentId\":\"sha256:abcdef\",\"name\":\"a.txt\",\"bytes\":4}}]}}\n"
    );
    let deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &persistence,
        attachments: &attachments,
        sessions: None,
    };
    let entries =
        collect_session_log_zip_entries(&deps, root.to_owned(), &SessionId::from("s"), false, None)
            .await
            .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["session.v4.jsonl", "media/img.png", "files/ab/abcdef/a.txt",]
    );
    assert_eq!(entries[1].data, b"png");
    assert_eq!(entries[2].data, b"file");
    let zip = build_session_log_zip(
        &deps,
        root.to_owned(),
        &SessionId::from("s"),
        false,
        6,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        read_central_zip_entries(&zip),
        vec![
            ("session.v4.jsonl".into(), root.as_bytes().to_vec()),
            ("media/img.png".into(), b"png".to_vec()),
            ("files/ab/abcdef/a.txt".into(), b"file".to_vec()),
        ]
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn unreadable_attachment_becomes_a_manifest_entry() {
    let kernel = Kernel::start();
    let query = SessionQueryEngine::new(
        kernel.root(),
        hoh_session::SessionStore::new(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let persistence = FakePersistence::not_found();
    let attachments = FakeAttachments {
        missing: true,
        ..Default::default()
    };
    let root = "{\"type\":\"user/message\",\"data\":{\"content\":[{\"type\":\"image\",\"attachment\":{\"attachmentId\":\"img\",\"mediaType\":\"image/png\",\"bytes\":3,\"width\":1,\"height\":1}}]}}\n";
    let deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &persistence,
        attachments: &attachments,
        sessions: None,
    };
    let entries =
        collect_session_log_zip_entries(&deps, root.to_owned(), &SessionId::from("s"), false, None)
            .await
            .unwrap();
    assert_eq!(entries.last().unwrap().path, MISSING_ATTACHMENTS_FILENAME);
    assert_eq!(
        String::from_utf8(entries.last().unwrap().data.clone()).unwrap(),
        "media/img.png\tmissing img\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn malformed_image_references_are_reported_in_the_missing_manifest() {
    let kernel = Kernel::start();
    let query = SessionQueryEngine::new(
        kernel.root(),
        hoh_session::SessionStore::new(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let persistence = FakePersistence::not_found();
    let attachments = FakeAttachments {
        invalid: true,
        ..Default::default()
    };
    let root = "{\"type\":\"user/message\",\"data\":{\"content\":[{\"type\":\"image\",\"attachment\":{\"attachmentId\":\"x\"}}]}}\n";
    let deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &persistence,
        attachments: &attachments,
        sessions: None,
    };
    let entries =
        collect_session_log_zip_entries(&deps, root.to_owned(), &SessionId::from("s"), false, None)
            .await
            .unwrap();
    assert_eq!(entries.last().unwrap().path, MISSING_ATTACHMENTS_FILENAME);
    assert_eq!(
        String::from_utf8(entries.last().unwrap().data.clone()).unwrap(),
        "media/x.undefined\tinvalid img\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn file_reference_without_a_string_name_aborts_the_export_like_typescript() {
    let kernel = Kernel::start();
    let query = SessionQueryEngine::new(
        kernel.root(),
        SessionStore::new(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let persistence = FakePersistence::not_found();
    let attachments = FakeAttachments {
        invalid_file: true,
        ..Default::default()
    };
    let root = "{\"type\":\"user/message\",\"data\":{\"content\":[{\"type\":\"file\",\"attachment\":{\"attachmentId\":\"x\"}}]}}\n";
    let deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &persistence,
        attachments: &attachments,
        sessions: None,
    };
    let error =
        collect_session_log_zip_entries(&deps, root.to_owned(), &SessionId::from("s"), false, None)
            .await
            .unwrap_err();
    assert_eq!(error.to_string(), "file attachment name is not a string");
    kernel.shutdown().await;
}

#[tokio::test]
async fn descendants_are_exported_preorder_and_contribute_media_to_both_archive_paths() {
    let kernel = Kernel::start();
    let store = SessionStore::new();
    let root_header = create_lineage_session(&store, kernel.root(), "root", None, 1);
    let a_header = create_lineage_session(&store, kernel.root(), "a", Some("root"), 2);
    let a1_header = create_lineage_session(&store, kernel.root(), "a1", Some("a"), 3);
    let b_header = create_lineage_session(&store, kernel.root(), "b", Some("root"), 4);
    let query = SessionQueryEngine::new(
        kernel.root(),
        store.clone(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let image_event = Arc::new(event(json!({
        "type":"user/message","seq":0,"time":3,"surfaceOp":"append","data":{"content":[{
            "type":"image","attachment":{"attachmentId":"desc","mediaType":"image/png","bytes":3,"width":1,"height":1}
        }]}
    })));
    let file_event = Arc::new(event(json!({
        "type":"user/message","seq":0,"time":4,"surfaceOp":"append","data":{"content":[{
            "type":"file","attachment":{"attachmentId":"sha256:abcdef","name":"b.txt","bytes":4}
        }]}
    })));
    let persistence = FakePersistence::stored_by_id([
        (a_header, Vec::new()),
        (a1_header, vec![image_event]),
        (b_header, vec![file_event]),
    ]);
    let attachments = FakeAttachments::default();
    let deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &persistence,
        attachments: &attachments,
        sessions: Some(&store),
    };
    let root = serialize_session_log(&root_header, &[] as &[SessionEvent]).unwrap();
    let entries =
        collect_session_log_zip_entries(&deps, root.clone(), &SessionId::from("root"), true, None)
            .await
            .unwrap();
    let expected_paths = vec![
        "session.v4.jsonl",
        "subagents/a/session.v4.jsonl",
        "subagents/a1/session.v4.jsonl",
        "subagents/b/session.v4.jsonl",
        "media/desc.png",
        "files/ab/abcdef/b.txt",
    ];
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        expected_paths
    );
    let zip = write_session_log_zip(
        &deps,
        root,
        &SessionId::from("root"),
        true,
        6,
        None,
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        read_central_zip_entries(&zip)
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        expected_paths
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn a_descendant_without_a_stored_log_aborts_both_archive_paths() {
    let kernel = Kernel::start();
    let store = SessionStore::new();
    let root_header = create_lineage_session(&store, kernel.root(), "root", None, 1);
    create_lineage_session(&store, kernel.root(), "child", Some("root"), 2);
    let query = SessionQueryEngine::new(
        kernel.root(),
        store.clone(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let persistence = FakePersistence::not_found();
    let attachments = FakeAttachments::default();
    let deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &persistence,
        attachments: &attachments,
        sessions: Some(&store),
    };
    let root = serialize_session_log(&root_header, &[] as &[SessionEvent]).unwrap();
    let collect_error =
        collect_session_log_zip_entries(&deps, root.clone(), &SessionId::from("root"), true, None)
            .await
            .unwrap_err();
    assert_eq!(
        collect_error.to_string(),
        "subagent \"child\" has no stored log"
    );
    let stream_error = write_session_log_zip(
        &deps,
        root,
        &SessionId::from("root"),
        true,
        6,
        None,
        Vec::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        stream_error.to_string(),
        "subagent \"child\" has no stored log"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn cancellation_during_a_missing_descendant_read_wins_over_missing_log() {
    let kernel = Kernel::start();
    let store = SessionStore::new();
    let root_header = create_lineage_session(&store, kernel.root(), "root", None, 1);
    create_lineage_session(&store, kernel.root(), "child", Some("root"), 2);
    let query = SessionQueryEngine::new(
        kernel.root(),
        store.clone(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let attachments = FakeAttachments::default();
    let root = serialize_session_log(&root_header, &[] as &[SessionEvent]).unwrap();

    let collect_controller = Arc::new(AbortController::new());
    let collect_signal = collect_controller.signal();
    let collect_persistence = FakePersistence::abort_then_not_found(collect_controller);
    let collect_deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &collect_persistence,
        attachments: &attachments,
        sessions: Some(&store),
    };
    let collect_error = collect_session_log_zip_entries(
        &collect_deps,
        root.clone(),
        &SessionId::from("root"),
        true,
        Some(&collect_signal),
    )
    .await
    .unwrap_err();
    assert!(
        collect_error
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&collect_signal.reason().unwrap())
    );
    assert_eq!(
        collect_error.to_string(),
        "cancelled during descendant read"
    );

    let stream_controller = Arc::new(AbortController::new());
    let stream_signal = stream_controller.signal();
    let stream_persistence = FakePersistence::abort_then_not_found(stream_controller);
    let stream_deps = SessionLogExportReady {
        session_query: &query,
        session_persistence: &stream_persistence,
        attachments: &attachments,
        sessions: Some(&store),
    };
    let stream_error = write_session_log_zip(
        &stream_deps,
        root,
        &SessionId::from("root"),
        true,
        6,
        Some(&stream_signal),
        Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(
        stream_error
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&stream_signal.reason().unwrap())
    );
    assert_eq!(stream_error.to_string(), "cancelled during descendant read");
    kernel.shutdown().await;
}

#[tokio::test]
async fn archive_head_preflights_only_the_root_and_returns_no_body() {
    let kernel = Kernel::start();
    let query = SessionQueryEngine::new(
        kernel.root(),
        hoh_session::SessionStore::new(),
        Arc::new(DisabledSearch),
        QueryConfig::default(),
    )
    .unwrap();
    let persistence = FakePersistence::stored(Arc::new(AtomicBool::new(false)));
    let attachments = FakeAttachments {
        missing: true,
        ..Default::default()
    };
    let response = session_log_export_head_response(
        SessionLogExportServices {
            session_query: Some(&query),
            session_persistence: Some(&persistence),
            attachments: Some(&attachments),
            sessions: None,
        },
        Some("s"),
        Some("true"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, Some("application/zip"));
    assert!(response.body.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn download_controller_coalesces_requests_and_preserves_dismissal() {
    let adapter = Arc::new(FakeDownloadAdapter::new(ClientResponse::ok("")));
    let controller = SessionLogDownloadController::new(adapter.clone());
    let first = controller.download("a/b");
    let second = controller.download("a/b");
    controller.dismiss("a/b");
    futures_util::future::join(first, second).await;
    assert_eq!(adapter.fetches.load(Ordering::SeqCst), 1);
    assert_eq!(
        adapter.saves.lock().unwrap().as_slice(),
        &[(
            "api/session.export?sessionId=a%2Fb&includeDescendants=true".to_owned(),
            "dsh-session-a_b.zip".to_owned(),
        )]
    );
    let state = controller.state();
    let entry = &state.by_session["a/b"];
    assert_eq!(entry.status, SessionLogDownloadStatus::Success);
    assert!(!entry.open);
}

#[tokio::test]
async fn controller_operations_are_eager_and_state_is_observable() {
    let adapter = Arc::new(FakeDownloadAdapter::new(ClientResponse::ok("")));
    let controller = SessionLogDownloadController::new(adapter.clone());
    let mut changes = controller.subscribe();
    let _operation = controller.download("s");
    assert_eq!(
        changes.borrow().by_session["s"].status,
        SessionLogDownloadStatus::Downloading
    );
    tokio::task::yield_now().await;
    changes.changed().await.unwrap();
    assert_eq!(adapter.fetches.load(Ordering::SeqCst), 1);
    assert_eq!(
        changes.borrow().by_session["s"].status,
        SessionLogDownloadStatus::Success
    );
}

#[tokio::test]
async fn download_controller_publishes_sanitized_http_failure() {
    let adapter = Arc::new(FakeDownloadAdapter::new(ClientResponse {
        status: 503,
        body: "busy".into(),
    }));
    let controller = SessionLogDownloadController::new(adapter);
    controller.download("s").await;
    let state = controller.state();
    let entry = &state.by_session["s"];
    assert_eq!(entry.status, SessionLogDownloadStatus::Error);
    assert_eq!(entry.error.as_deref(), Some("Export failed: HTTP 503 busy"));
}

#[tokio::test]
async fn markdown_copy_fallback_publishes_success_and_dismisses_notice() {
    let adapter = Arc::new(FakeCopyAdapter::fallback(
        ClientResponse::ok("# title\n"),
        true,
    ));
    let controller = SessionMarkdownCopyController::new(adapter.clone());
    controller.copy("s").await;
    assert_eq!(
        adapter.writes.lock().unwrap().as_slice(),
        &["# title\n".to_owned()]
    );
    assert_eq!(
        controller.state().notice.unwrap().kind,
        SessionMarkdownCopyNoticeKind::Success
    );
    controller.dismiss();
    assert!(controller.state().notice.is_none());
}

#[tokio::test]
async fn markdown_copy_starts_deferred_write_synchronously_and_classifies_http_failure() {
    let adapter = Arc::new(FakeCopyAdapter::deferred(ClientResponse {
        status: 404,
        body: String::new(),
    }));
    let controller = SessionMarkdownCopyController::new(adapter.clone());
    let done = controller.copy("s");
    assert_eq!(adapter.deferred_starts.load(Ordering::SeqCst), 1);
    done.await;
    assert_eq!(
        controller.state().notice.unwrap().kind,
        SessionMarkdownCopyNoticeKind::PreparationFailed
    );
}

struct FakeDownloadAdapter {
    response: ClientResponse,
    fetches: AtomicUsize,
    saves: Mutex<Vec<(String, String)>>,
}

impl FakeDownloadAdapter {
    fn new(response: ClientResponse) -> Self {
        Self {
            response,
            fetches: AtomicUsize::new(0),
            saves: Mutex::new(vec![]),
        }
    }
}

impl SessionLogDownloadAdapter for FakeDownloadAdapter {
    fn preflight(
        &self,
        _: String,
        _: hoh_timeout::AbortSignal,
    ) -> BoxFuture<'static, Result<ClientResponse, BoxError>> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        let response = self.response.clone();
        Box::pin(async move { Ok(response) })
    }
    fn save(&self, route: &str, filename: &str) -> Result<(), BoxError> {
        self.saves
            .lock()
            .unwrap()
            .push((route.to_owned(), filename.to_owned()));
        Ok(())
    }
}

struct FakeCopyAdapter {
    response: ClientResponse,
    deferred: bool,
    accepted: bool,
    deferred_starts: AtomicUsize,
    writes: Arc<Mutex<Vec<String>>>,
}

impl FakeCopyAdapter {
    fn fallback(response: ClientResponse, accepted: bool) -> Self {
        Self {
            response,
            deferred: false,
            accepted,
            deferred_starts: AtomicUsize::new(0),
            writes: Arc::new(Mutex::new(vec![])),
        }
    }
    fn deferred(response: ClientResponse) -> Self {
        Self {
            response,
            deferred: true,
            accepted: true,
            deferred_starts: AtomicUsize::new(0),
            writes: Arc::new(Mutex::new(vec![])),
        }
    }
}

impl SessionMarkdownCopyAdapter for FakeCopyAdapter {
    fn fetch_markdown(
        &self,
        _: String,
        _: hoh_timeout::AbortSignal,
    ) -> BoxFuture<'static, Result<ClientResponse, BoxError>> {
        let response = self.response.clone();
        Box::pin(async move { Ok(response) })
    }
    fn supports_deferred_write(&self) -> bool {
        self.deferred
    }
    fn write_deferred(
        &self,
        data: BoxFuture<'static, Result<Arc<String>, hoh_error::SharedError>>,
    ) -> Result<BoxFuture<'static, Result<(), BoxError>>, BoxError> {
        self.deferred_starts.fetch_add(1, Ordering::SeqCst);
        let writes = self.writes.clone();
        Ok(Box::pin(async move {
            let text = data.await.map_err(|error| Box::new(error) as BoxError)?;
            writes.lock().unwrap().push((*text).clone());
            Ok(())
        }))
    }
    fn write_text(&self, text: String) -> BoxFuture<'static, Result<bool, BoxError>> {
        self.writes.lock().unwrap().push(text);
        let accepted = self.accepted;
        Box::pin(async move { Ok(accepted) })
    }
}

struct DisabledSearch;

impl SessionSearchProvider for DisabledSearch {
    fn search_sessions<'a>(
        &'a self,
        _: SessionSearchRequest,
        _: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionSearchPage<SessionSearchHit>, BoxError>> {
        Box::pin(async {
            Ok(SessionSearchPage {
                items: vec![],
                next_cursor: None,
            })
        })
    }
    fn search_events<'a>(
        &'a self,
        _: SessionEventSearchRequest,
        _: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionEventSearchPage, BoxError>> {
        Box::pin(async { Err("unused".into()) })
    }
}

struct FakeAttachments {
    missing: bool,
    invalid: bool,
    invalid_file: bool,
    limits: ImageAttachmentLimits,
}

impl Default for FakeAttachments {
    fn default() -> Self {
        Self {
            missing: false,
            invalid: false,
            invalid_file: false,
            limits: ImageAttachmentLimits {
                max_image_bytes: 1024,
                max_images_per_message: 8,
                max_message_image_bytes: 8192,
                max_image_pixels: 1024,
                max_image_dimension: 1024,
                media_types: vec![hoh_attachment::ImageMediaType::Png],
            },
        }
    }
}

impl AttachmentStore for FakeAttachments {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }
    fn validate_image<'a>(
        &'a self,
        _: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
    fn save_image<'a>(
        &'a self,
        _: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async { Err("unused".into()) })
    }
    fn read_image<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        _: Option<&'a hoh_timeout::AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async move {
            if self.invalid {
                Err(Box::new(AttachmentError::new(
                    "invalid img",
                    AttachmentErrorCode::InvalidAttachmentRef,
                )) as BoxError)
            } else if self.missing {
                Err(Box::new(AttachmentError::new(
                    "missing img",
                    AttachmentErrorCode::AttachmentNotFound,
                )) as BoxError)
            } else {
                Ok(StoredImageAttachment {
                    r#ref: reference.clone(),
                    data: b"png".to_vec(),
                })
            }
        })
    }
    fn read_file_stream<'a>(
        &'a self,
        _: &'a hoh_attachment::FileAttachmentRef,
        _: Option<&'a hoh_timeout::AbortSignal>,
    ) -> BoxStream<'a, Result<Vec<u8>, BoxError>> {
        if self.invalid_file {
            Box::pin(stream::iter([Err(Box::new(AttachmentError::new(
                "invalid file",
                AttachmentErrorCode::InvalidAttachmentRef,
            )) as BoxError)]))
        } else {
            Box::pin(stream::iter([Ok(b"fi".to_vec()), Ok(b"le".to_vec())]))
        }
    }
}

#[derive(Clone)]
enum OpenResult {
    Stored(Arc<FakeHandle>),
    StoredById(Arc<HashMap<SessionId, Arc<FakeHandle>>>),
    AbortThenNotFound(Arc<AbortController>),
    NotFound,
    Failed(&'static str),
}

struct FakePersistence {
    identity: SessionPersistenceIdentity,
    result: OpenResult,
}

impl FakePersistence {
    fn stored(closed: Arc<AtomicBool>) -> Self {
        let header = SessionHeader::new(SessionId::from("s"), 1);
        let events = vec![Arc::new(event(
            json!({"type":"turn/start","seq":0,"time":1,"data":{}}),
        ))];
        Self {
            identity: SessionPersistenceIdentity::new(),
            result: OpenResult::Stored(Arc::new(FakeHandle {
                header,
                events,
                closed,
            })),
        }
    }

    fn not_found() -> Self {
        Self {
            identity: SessionPersistenceIdentity::new(),
            result: OpenResult::NotFound,
        }
    }

    fn stored_by_id(
        entries: impl IntoIterator<Item = (SessionHeader, Vec<Arc<SessionEvent>>)>,
    ) -> Self {
        let handles = entries
            .into_iter()
            .map(|(header, events)| {
                (
                    header.id.clone(),
                    Arc::new(FakeHandle {
                        header,
                        events,
                        closed: Arc::new(AtomicBool::new(false)),
                    }),
                )
            })
            .collect();
        Self {
            identity: SessionPersistenceIdentity::new(),
            result: OpenResult::StoredById(Arc::new(handles)),
        }
    }

    fn abort_then_not_found(controller: Arc<AbortController>) -> Self {
        Self {
            identity: SessionPersistenceIdentity::new(),
            result: OpenResult::AbortThenNotFound(controller),
        }
    }

    fn failure(message: &'static str) -> Self {
        Self {
            identity: SessionPersistenceIdentity::new(),
            result: OpenResult::Failed(message),
        }
    }
}

impl SessionPersistence for FakePersistence {
    fn identity(&self) -> SessionPersistenceIdentity {
        self.identity.clone()
    }

    fn create<'a>(
        &'a self,
        _: &'a SessionHeader,
        _: Option<&'a SessionPersistenceCreateOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async { Err("unused".into()) })
    }

    fn open<'a>(
        &'a self,
        id: &'a SessionId,
        _: SessionAccess,
        _: Option<&'a SessionPersistenceOpenOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async move {
            match &self.result {
                OpenResult::Stored(handle) => Ok(handle.clone() as Arc<dyn SessionHandle>),
                OpenResult::StoredById(handles) => handles
                    .get(id)
                    .cloned()
                    .map(|handle| handle as Arc<dyn SessionHandle>)
                    .ok_or_else(|| {
                        Box::new(SessionPersistenceNotFoundError::new(id.clone())) as BoxError
                    }),
                OpenResult::AbortThenNotFound(controller) => {
                    controller.abort_with(AbortError::new("cancelled during descendant read"));
                    Err(Box::new(SessionPersistenceNotFoundError::new(id.clone())) as BoxError)
                }
                OpenResult::NotFound => {
                    Err(Box::new(SessionPersistenceNotFoundError::new(id.clone())) as BoxError)
                }
                OpenResult::Failed(message) => Err((*message).into()),
            }
        })
    }

    fn flush(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn stat<'a>(
        &'a self,
        _: &'a SessionId,
        _: Option<&'a SessionPersistenceStatOptions>,
    ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>> {
        Box::pin(async { Ok(None) })
    }

    fn list<'a>(
        &'a self,
        _: Option<&'a SessionPersistenceListOptions>,
    ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>> {
        Box::pin(async { Ok(vec![]) })
    }
}

struct FakeHandle {
    header: SessionHeader,
    events: Vec<Arc<SessionEvent>>,
    closed: Arc<AtomicBool>,
}

impl SessionHandle for FakeHandle {
    fn id(&self) -> &SessionId {
        &self.header.id
    }
    fn header(&self) -> &SessionHeader {
        &self.header
    }
    fn inherited_event_count(&self) -> hoh_session::SessionLogOffset {
        hoh_session::SessionLogOffset::new(0).unwrap()
    }
    fn access(&self) -> SessionAccess {
        SessionAccess::Read
    }
    fn read<'a>(
        &'a self,
        _: Option<u64>,
        _: Option<u64>,
        _: Option<&'a SessionHandleReadOptions>,
    ) -> BoxFuture<'a, Result<SessionHandleReadResult, BoxError>> {
        Box::pin(async move {
            Ok(SessionHandleReadResult {
                event_state: hoh_session::SessionSeedEventState::Detached,
                events: self.events.clone(),
            })
        })
    }
    fn append<'a>(
        &'a self,
        _: &'a [Arc<SessionEvent>],
        _: Option<&'a SessionHandleAppendOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err("read only".into()) })
    }
    fn flush<'a>(
        &'a self,
        _: Option<&'a SessionHandleFlushOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err("read only".into()) })
    }
    fn close(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async move {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

fn read_local_zip_entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut offset = 0;
    let mut entries = Vec::new();
    while bytes.get(offset..offset + 4) == Some(&[0x50, 0x4b, 0x03, 0x04]) {
        let compressed =
            u32::from_le_bytes(bytes[offset + 18..offset + 22].try_into().unwrap()) as usize;
        let name_len =
            u16::from_le_bytes(bytes[offset + 26..offset + 28].try_into().unwrap()) as usize;
        let extra_len =
            u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
        let name_start = offset + 30;
        let data_start = name_start + name_len + extra_len;
        let name = String::from_utf8(bytes[name_start..name_start + name_len].to_vec()).unwrap();
        let mut decoder = DeflateDecoder::new(&bytes[data_start..data_start + compressed]);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data).unwrap();
        entries.push((name, data));
        offset = data_start + compressed;
    }
    entries
}

fn read_central_zip_entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let eocd = bytes
        .windows(4)
        .rposition(|window| window == [0x50, 0x4b, 0x05, 0x06])
        .unwrap();
    let count = u16::from_le_bytes(bytes[eocd + 10..eocd + 12].try_into().unwrap()) as usize;
    let mut central = u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    let mut entries = Vec::new();
    for _ in 0..count {
        assert_eq!(&bytes[central..central + 4], &[0x50, 0x4b, 0x01, 0x02]);
        let compressed =
            u32::from_le_bytes(bytes[central + 20..central + 24].try_into().unwrap()) as usize;
        let name_len =
            u16::from_le_bytes(bytes[central + 28..central + 30].try_into().unwrap()) as usize;
        let extra_len =
            u16::from_le_bytes(bytes[central + 30..central + 32].try_into().unwrap()) as usize;
        let comment_len =
            u16::from_le_bytes(bytes[central + 32..central + 34].try_into().unwrap()) as usize;
        let local =
            u32::from_le_bytes(bytes[central + 42..central + 46].try_into().unwrap()) as usize;
        let name =
            String::from_utf8(bytes[central + 46..central + 46 + name_len].to_vec()).unwrap();
        let local_name_len =
            u16::from_le_bytes(bytes[local + 26..local + 28].try_into().unwrap()) as usize;
        let local_extra_len =
            u16::from_le_bytes(bytes[local + 28..local + 30].try_into().unwrap()) as usize;
        let data_start = local + 30 + local_name_len + local_extra_len;
        let mut decoder = DeflateDecoder::new(&bytes[data_start..data_start + compressed]);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data).unwrap();
        entries.push((name, data));
        central += 46 + name_len + extra_len + comment_len;
    }
    entries
}
