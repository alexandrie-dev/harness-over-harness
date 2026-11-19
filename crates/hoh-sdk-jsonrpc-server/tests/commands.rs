mod support;
use hoh_agent::Agents;
use hoh_error::SharedError;
use hoh_protocol::CommandResult;
use hoh_sdk_jsonrpc_server::command_helpers::{BuiltinCommandName, CommandDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{commands::Registry, *};
async fn setup() -> (Env, Arc<Registry>) {
    let h = Env::new().await;
    let commands = Registry::mount(&h).await;
    h.initialize().await;
    (h, commands)
}
#[tokio::test]
async fn accepts_leading_whitespace_through_the_query_wire_endpoint() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    for prefix in [" ", "\t\n", "\u{3000}"] {
        assert_eq!(
            h.query("query-space", &format!("{prefix}/compact"))
                .await
                .unwrap()["kind"],
            "command"
        );
    }
    assert!(lock(&c.calls).iter().all(|(line, _)| line == "/compact"));
    h.close().await;
}
#[tokio::test]
async fn returns_the_logged_native_success_or_error_without_a_model_turn() {
    for kind in ["success", "error"] {
        let (h, c) = setup().await;
        c.register("compact", Some(BuiltinCommandName::Compact));
        c.handler(Arc::new(move |_, _, _, _| {
            Box::pin(async move {
                Ok(Some(
                    serde_json::from_value(json!({"kind":kind,"text":"native outcome"})).unwrap(),
                ))
            })
        }));
        let execution = h.query("commands", "/compact").await.unwrap();
        assert_eq!(
            execution["execution"]["result"],
            json!({"kind":kind,"text":"native outcome"})
        );
        assert_eq!(
            types(&h.agent("commands"))
                .iter()
                .filter(|t| t.starts_with("command/"))
                .cloned()
                .collect::<Vec<_>>(),
            ["command/run", "command/done"]
        );
        assert!(lock(&h.model.requests).is_empty());
        h.close().await;
    }
}
#[tokio::test]
async fn rejects_malformed_content_before_creating_a_session() {
    let (h, _) = setup().await;
    let image = json!({"type":"image","data":"AQ==","mimeType":"image/png"});
    let invalid = vec![
        Value::Null,
        json!({}),
        json!("image"),
        json!([]),
        json!([null]),
        json!([1]),
        json!([[]]),
        json!([{}]),
        json!([{"type":1}]),
        json!([{"type":"text"}]),
        json!([{"type":"text","text":1}]),
        json!([{"type":"text","text":"go","extra":true}]),
        json!([{"type":"text","text":""}]),
        json!([{"type":"text","text":"  "},{"type":"text","text":"\n"}]),
        json!([{"type":"image","mimeType":"image/png"}]),
        json!([{"type":"image","attachment":null}]),
        json!([{"type":"image","attachment":[]}]),
        json!([{"type":"image","attachment":"missing"}]),
        json!([{ "type":"image","data":1,"mimeType":"image/png"}]),
        json!([{"type":"image","data":"AQ=="}]),
        json!([{"type":"image","data":"AQ==","mimeType":"image/svg+xml"}]),
        json!([{"type":"image","data":"AQ==","mimeType":"image/png","name":null}]),
        json!([{"type":"image","data":"AQ==","mimeType":"image/png","attachment":{"attachmentId":"other-session"}}]),
    ];
    for blocks in invalid {
        let e = h
            .server
            .handle_request(
                "session/query",
                json!({"sessionId":"invalid-content","contentBlocks":blocks}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            e.downcast_ref::<hoh_sdk_jsonrpc_server::error::SdkServerError>()
                .unwrap()
                .name(),
            "TypeError"
        );
        assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    }
    assert_eq!(image["type"], "image");
    h.close().await;
}
#[tokio::test]
async fn leaves_command_attachment_support_to_the_native_registry() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    c.handler(Arc::new(|_, _, images, _| {
        Box::pin(async move {
            assert_eq!(images.len(), 1);
            Ok(Some(CommandResult::Error {
                text: "/compact does not accept attachments".into(),
            }))
        })
    }));
    for mime in ["image/png", "image/jpeg", "image/webp", "image/gif"] {
        let result=h.server.handle_request("session/query",json!({"sessionId":"refused","contentBlocks":[{"type":"text","text":"/compact"},{"type":"image","data":"AQ==","mimeType":mime}]})).await.unwrap();
        assert_eq!(
            result["execution"]["result"]["text"],
            "/compact does not accept attachments"
        );
    }
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn admits_named_images_through_the_native_command_executor_in_both_orders() {
    let (h, c) = setup().await;
    c.register("plan", Some(BuiltinCommandName::Plan));
    for image_first in [true, false] {
        let text = json!({"type":"text","text":"/plan inspect"});
        let image =
            json!({"type":"image","data":"AQ==","mimeType":"image/png","name":"diagram.png"});
        let blocks = if image_first {
            vec![image, text]
        } else {
            vec![text, image]
        };
        assert_eq!(
            h.server
                .handle_request(
                    "session/query",
                    json!({"sessionId":"images","contentBlocks":blocks})
                )
                .await
                .unwrap()["kind"],
            "command"
        );
    }
    assert!(
        lock(&c.calls)
            .iter()
            .all(|(line, images)| line == "/plan inspect"
                && images.len() == 1
                && images[0].image.name.as_deref() == Some("diagram.png"))
    );
    h.close().await;
}
#[tokio::test]
async fn rejects_a_recognized_command_with_unsupported_content_without_executing_or_falling_back() {
    let (h, c) = setup().await;
    c.register("plan", Some(BuiltinCommandName::Plan));
    for extra in [
        json!({"type":"text","text":"another command paragraph"}),
        json!({"type":"text","text":""}),
        json!({"type":"image","attachment":{"attachmentId":"stored-image","mediaType":"image/png","bytes":1,"width":1,"height":1}}),
        json!({"type":"file","attachment":{"attachmentId":"stored-file","name":"notes.txt","bytes":1}}),
        json!({"type":"reasoning","text":"earlier reasoning"}),
    ] {
        assert_eq!(h.server.handle_request("session/query",json!({"sessionId":"refused-content","contentBlocks":[{"type":"image","data":"AQ==","mimeType":"image/png"},{"type":"text","text":"/plan off"},extra]})).await.unwrap_err().to_string(),"Native SDK commands require one text block and optional encoded image blocks");
    }
    assert!(lock(&c.calls).is_empty());
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn accepts_english_and_chinese_aliases_and_delivers_lifecycle_before_resolution() {
    let (h, c) = setup().await;
    for (name, id, line, alias) in [
        ("compact", BuiltinCommandName::Compact, "/compact", "/压缩"),
        (
            "permission",
            BuiltinCommandName::Permission,
            "/permission read-only",
            "/权限 read-only",
        ),
        ("plan", BuiltinCommandName::Plan, "/plan", "/计划"),
        ("goal", BuiltinCommandName::Goal, "/goal", "/目标"),
    ] {
        c.register(name, Some(id));
        for text in [line, alias] {
            let result = h.query("alias-command", text).await.unwrap();
            let id = &result["execution"]["commandId"];
            assert!(
                h.peer
                    .events("session.event")
                    .iter()
                    .any(|p| p["event"]["type"] == "command/done"
                        && p["event"]["data"]["commandId"] == *id)
            );
        }
    }
    assert_eq!(lock(&c.calls).len(), 8);
    h.close().await;
}
#[tokio::test]
async fn dispatches_renamed_definition_through_registered_name_and_aliases() {
    let (h, c) = setup().await;
    for (name, id, alias, args) in [
        ("compact", BuiltinCommandName::Compact, "压缩", ""),
        (
            "permission",
            BuiltinCommandName::Permission,
            "权限",
            " read-only",
        ),
        ("plan", BuiltinCommandName::Plan, "计划", " task"),
        ("goal", BuiltinCommandName::Goal, "目标", " task"),
    ] {
        let registered = format!("renamed-{name}");
        c.register(&registered, Some(id));
        for token in [name, alias, &registered] {
            assert_eq!(
                h.query("renamed", &format!("/{token}{args}"))
                    .await
                    .unwrap()["kind"],
                "command"
            );
            assert_eq!(
                lock(&c.calls).last().unwrap().0,
                format!("/{registered}{args}")
            );
        }
    }
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn keeps_an_unrelated_definition_outside_native_sdk_dispatch() {
    let (h, c) = setup().await;
    for (name, args) in [
        ("compact", ""),
        ("permission", " read-only"),
        ("plan", " task"),
        ("goal", " task"),
    ] {
        lock(&c.descriptors).push(CommandDescriptor {
            name: name.into(),
            definition_id: Some(format!("example/{name}").into()),
            description: "Unrelated".into(),
        });
        let line = format!("/{name}{args}");
        assert_eq!(h.query("unrelated", &line).await.unwrap()["kind"], "prompt");
        h.wait("unrelated").await;
        assert!(
            hoh_util_values::js_json_stringify(
                &serde_json::to_value(
                    &lock(&h.model.requests)
                        .last()
                        .unwrap_or_else(|| {
                            panic!(
                                "model was not called; events: {:#?}",
                                h.agent("unrelated").session().snapshot_events(None, None)
                            )
                        })
                        .messages,
                )
                .unwrap()
            )
            .contains(&line)
        );
    }
    assert!(lock(&c.calls).is_empty());
    h.close().await;
}
#[tokio::test]
async fn keeps_a_renamed_permission_command_without_a_preset_as_ordinary_input() {
    let (h, c) = setup().await;
    c.register("access", Some(BuiltinCommandName::Permission));
    assert_eq!(
        h.query("empty-preset", "/access").await.unwrap()["kind"],
        "prompt"
    );
    h.wait("empty-preset").await;
    assert!(lock(&c.calls).is_empty());
    h.close().await;
}
#[tokio::test]
async fn reports_an_execution_unavailable_after_catalog_resolution() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    c.handler(Arc::new(|_, _, _, _| Box::pin(async { Ok(None) })));
    assert!(
        h.query("commands", "/compact")
            .await
            .unwrap_err()
            .to_string()
            .contains("selected command is unavailable")
    );
    h.close().await;
}
#[tokio::test]
async fn passes_explicit_permission_input_to_the_native_handler() {
    let (h, c) = setup().await;
    c.register("permission", Some(BuiltinCommandName::Permission));
    c.handler(Arc::new(|_, raw, _, _| {
        Box::pin(async move {
            assert_eq!(raw, "  deployment-preset");
            Ok(Some(CommandResult::Error {
                text: "native preset refusal".into(),
            }))
        })
    }));
    assert_eq!(
        h.query("permission", "/permission  deployment-preset")
            .await
            .unwrap()["execution"]["result"]["text"],
        "native preset refusal"
    );
    h.close().await;
}
#[tokio::test]
async fn delegates_plan_without_reconstructing_model_input() {
    let (h, c) = setup().await;
    c.register("plan", Some(BuiltinCommandName::Plan));
    for line in ["/plan", "/plan off", "/plan  explain the change"] {
        assert_eq!(h.query("plan", line).await.unwrap()["kind"], "command");
        assert_eq!(lock(&c.calls).last().unwrap().0, line);
    }
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn executes_unicode_separated_input_through_its_native_handler() {
    let (h, c) = setup().await;
    for (n, id) in [
        ("plan", BuiltinCommandName::Plan),
        ("goal", BuiltinCommandName::Goal),
        ("permission", BuiltinCommandName::Permission),
    ] {
        c.register(n, Some(id));
    }
    for (line, expected) in [
        (
            "/plan\u{3000}Inspect\u{3000}the\nchange",
            "/plan Inspect\u{3000}the\nchange",
        ),
        ("/计划\u{a0}\t设计\u{a0}方案", "/plan \t设计\u{a0}方案"),
        (
            "/goal\u{2003}Finish\u{3000}the task",
            "/goal Finish\u{3000}the task",
        ),
        ("/目标\u{3000}\u{a0}完成任务", "/goal \u{a0}完成任务"),
        ("/permission\u{a0}read-only", "/permission read-only"),
        ("/权限\u{202f}\tread-only", "/permission \tread-only"),
    ] {
        h.query("unicode", line).await.unwrap();
        assert_eq!(lock(&c.calls).last().unwrap().0, expected);
    }
    h.close().await;
}
#[tokio::test]
async fn passes_arguments_unchanged_and_propagates_handler_failures() {
    let (h, c) = setup().await;
    c.register("plan", Some(BuiltinCommandName::Plan));
    let sentinel = SharedError::new("handler saw:  extra");
    let cause = sentinel.clone();
    c.handler(Arc::new(move |_, raw, _, _| {
        let cause = cause.clone();
        Box::pin(async move {
            assert_eq!(raw, "  extra");
            Err(cause.into())
        })
    }));
    let e = h.query("commands", "/plan  extra").await.unwrap_err();
    assert!(e.downcast_ref::<SharedError>().unwrap().ptr_eq(&sentinel));
    assert_eq!(
        h.agent("commands")
            .session()
            .snapshot_events(None, None)
            .last()
            .unwrap()
            .event_type(),
        "command/done"
    );
    h.close().await;
}
#[tokio::test]
async fn rejects_a_disposed_agent_retained_by_the_sdk_handle() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    h.query("commands", "/compact").await.unwrap();
    h.loop_fiber.dispose().await;
    assert!(
        h.query("commands", "/compact")
            .await
            .unwrap_err()
            .to_string()
            .contains("disposed outside the server")
    );
    h.close().await;
}
#[tokio::test]
async fn cancels_an_in_flight_command_on_shutdown_and_rejects_later_admission() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    let entered = Arc::new(tokio::sync::Notify::new());
    let e = entered.clone();
    c.handler(Arc::new(move |_, _, _, signal| {
        let e = e.clone();
        Box::pin(async move {
            e.notify_one();
            Err(signal.cancelled().await.into())
        })
    }));
    let server = h.server.clone();
    let running = tokio::spawn(async move {
        server
            .handle_request(
                "session/query",
                json!({"sessionId":"commands","contentBlocks":[{"type":"text","text":"/compact"}]}),
            )
            .await
    });
    entered.notified().await;
    h.open("commands").await;
    h.server.shutdown().await.unwrap();
    assert!(
        running
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("shutting down")
    );
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    assert!(
        h.query("commands", "/compact")
            .await
            .unwrap_err()
            .to_string()
            .contains("shutting down")
    );
    h.close().await;
}
#[tokio::test]
async fn rejects_invalid_query_fields() {
    let (h, _) = setup().await;
    for p in [
        Value::Null,
        json!({}),
        json!({"sessionId":"","contentBlocks":[{"type":"text","text":"go"}]}),
        json!({"sessionId":"q","text":"go"}),
        json!({"sessionId":"q","contentBlocks":[{"type":"text","text":"go"}],"delivery":"queue"}),
        json!({"sessionId":"q","contentBlocks":[{"type":"text","text":"go"}],"permission":"full"}),
        json!({"sessionId":"q","contentBlocks":[{"type":"text","text":"go"}],"attachments":[]}),
    ] {
        assert!(
            h.server
                .handle_request("session/query", p)
                .await
                .unwrap_err()
                .to_string()
                .contains("session/query requires only")
        );
        assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    }
    h.close().await;
}
#[tokio::test]
async fn preserves_input_without_a_command_registry_and_requires_live_initialized_server() {
    for text in ["/compact", "/permission read-only"] {
        let h = Env::new().await;
        assert!(
            h.query("q", text)
                .await
                .unwrap_err()
                .to_string()
                .contains("not initialized")
        );
        h.initialize().await;
        assert_eq!(h.query("q", text).await.unwrap()["kind"], "prompt");
        h.wait("q").await;
        h.server.shutdown().await.unwrap();
        assert!(
            h.query("q", text)
                .await
                .unwrap_err()
                .to_string()
                .contains("shutting down")
        );
        h.close().await;
    }
}
#[tokio::test]
async fn returns_the_native_command_result_without_creating_a_prompt() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    c.handler(Arc::new(|_, _, _, _| {
        Box::pin(async {
            Ok(Some(CommandResult::Error {
                text: "native refusal".into(),
            }))
        })
    }));
    assert_eq!(
        h.query("q", " \n/compact\t ").await.unwrap()["execution"]["result"]["text"],
        "native refusal"
    );
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn preserves_unclaimed_query_text() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    for text in ["ordinary text", "/unknown", " \n/compact extra\t "] {
        assert_eq!(h.query("q", text).await.unwrap()["kind"], "prompt");
        h.wait("q").await;
        assert!(
            hoh_util_values::js_json_stringify(
                &serde_json::to_value(&lock(&h.model.requests).last().unwrap().messages).unwrap()
            )
            .contains(&text.replace('\n', "\\n").replace('\t', "\\t"))
        );
    }
    assert!(lock(&c.calls).is_empty());
    h.close().await;
}
#[tokio::test]
async fn preserves_existing_prompt_behavior_for_unimplemented_input() {
    let (h, c) = setup().await;
    c.register("feedback", None);
    c.register("permission", Some(BuiltinCommandName::Permission));
    for text in [
        "/feedback inspect",
        "/model",
        "/file",
        "/export",
        "/permission",
        " \n/权限\t ",
    ] {
        assert_eq!(h.query("q", text).await.unwrap()["kind"], "prompt");
        h.wait("q").await;
    }
    assert!(lock(&c.calls).is_empty());
    h.close().await;
}
#[tokio::test]
async fn queues_ordinary_mixed_blocks_in_their_original_order() {
    let (h, c) = setup().await;
    c.register("compact", Some(BuiltinCommandName::Compact));
    let store = support::images::Store::mount(&h).await;
    let stored =
        json!({"attachmentId":"stored","mediaType":"image/png","bytes":1,"width":1,"height":1});
    let blocks = json!([{"type":"text","text":"Keep the original ordering."},{"type":"image","data":"AQ==","mimeType":"image/png"},{"type":"text","text":"/compact"},{"type":"image","attachment":stored},{"type":"image","data":"Ag==","mimeType":"image/png"},{"type":"file","attachment":{"attachmentId":"saved-file","name":"notes.txt","bytes":1}},{"type":"reasoning","text":"existing content"}]);
    assert_eq!(
        h.server
            .handle_request(
                "session/query",
                json!({"sessionId":"mixed","contentBlocks":blocks})
            )
            .await
            .unwrap()["kind"],
        "prompt"
    );
    h.wait("mixed").await;
    let agent = h.agent("mixed");
    let events = agent.session().snapshot_events(None, None);
    let event = events
        .iter()
        .find(|e| e.event_type() == "user/message" && e.data()["source"]["kind"] == "user")
        .unwrap();
    let content = &event.data()["content"];
    assert_eq!(content[0], blocks[0]);
    assert_eq!(content[1]["attachment"]["attachmentId"], "sha256:image-0");
    assert_eq!(content[2], blocks[2]);
    assert_eq!(content[3], blocks[3]);
    assert_eq!(content[4]["attachment"]["attachmentId"], "sha256:image-1");
    assert_eq!(content[5], blocks[5]);
    assert_eq!(content[6], blocks[6]);
    assert_eq!(
        lock(&store.saved)[0]
            .iter()
            .map(|i| i.data.clone())
            .collect::<Vec<_>>(),
        vec![vec![1], vec![2]]
    );
    assert!(lock(&c.calls).is_empty());
    h.close().await;
}
#[tokio::test]
async fn does_not_execute_command_or_prompt_while_its_session_is_being_created_at_shutdown() {
    for command in [false, true] {
        let (h, c) = setup().await;
        c.register("compact", Some(BuiltinCommandName::Compact));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let (e, r) = (entered.clone(), release.clone());
        h.ctx
            .on_serial::<hoh_agent::AgentCreated, (), _>(move |_| {
                let (e, r) = (e.clone(), r.clone());
                async move {
                    e.notify_one();
                    r.acquire().await.unwrap().forget();
                    Ok(None)
                }
            })
            .unwrap();
        let server = h.server.clone();
        let running = tokio::spawn(async move {
            server.handle_request("session/query",json!({"sessionId":"at-shutdown","contentBlocks":[{"type":"text","text":if command{"/compact"}else{"ordinary input"}}]})).await
        });
        entered.notified().await;
        let shutdown = h.server.shutdown();
        release.add_permits(1);
        assert!(
            running
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("shutting down")
        );
        shutdown.await.unwrap();
        assert!(lock(&c.calls).is_empty());
        assert!(lock(&h.model.requests).is_empty());
        assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
        h.close().await;
    }
}
