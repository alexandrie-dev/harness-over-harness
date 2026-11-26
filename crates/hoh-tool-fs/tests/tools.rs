//! Consumer API tests over a fake provider and the real observation policy: schemas,
//! validation, formatting, typed errors, intent dispatch, and observation-driven authorization.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use hoh_fs::{FsErrorCode, FsType, FsVersionGuard, FsWriteIntent};
use hoh_llm::ContentBlock;
use hoh_tool_fs::Env;
use hoh_tool_fs::read::STREAM_MIN_SIZE;
use hoh_tool_fs::read_render::{FileReadOutcome, FileTextLine, format_read_output};
use hoh_tool_fs::session_cwd::session_resolve_options;
use hoh_tools::Agent;
use serde_json::{Value, json};
use support::*;

fn agent() -> Arc<dyn Agent> {
    TestAgent::new()
}

fn invalid_args() -> Option<(String, String)> {
    named("ToolArgsError", "INVALID_ARGS")
}

mod current_directory_resolution {
    use super::*;

    #[tokio::test]
    async fn uses_the_directory_service_and_keeps_non_agent_provider_defaults() {
        let setup = setup(json!({})).await;
        let env = Env {
            ctx: setup.ctx.clone(),
            fs: setup.ctx.get().unwrap(),
            working_directory: setup.ctx.get().unwrap(),
            tools: setup.tools.clone(),
            system_prompt: setup.ctx.get().unwrap(),
        };
        let signal = live_signal();
        // A call with an agent resolves the Session's current directory through the service.
        let seam: Arc<dyn Agent> = TestAgent::new();
        let agent = support::ensure_live(&setup.ctx, &seam);
        let mut input =
            hoh_tools::ToolExecutionInput::new("c".into(), "read", json!({}), signal.clone());
        input.agent = Some(agent);
        let pipeline = hoh_tools::ToolExecutionPipeline::new(setup.ctx.clone()).unwrap();
        let exec = pipeline.prepare(input).await.unwrap().exec().clone();
        let options = session_resolve_options(&env, &exec).await.unwrap();
        assert_eq!(options.cwd.as_deref(), setup.workspace.path().to_str());
        assert!(options.signal.is_some());
        // A call without an agent leaves the directory to the provider.
        let bare = hoh_tools::ToolExecutionInput::new("c2".into(), "read", json!({}), signal);
        let exec = pipeline.prepare(bare).await.unwrap().exec().clone();
        let options = session_resolve_options(&env, &exec).await.unwrap();
        assert_eq!(options.cwd, None);
        assert!(options.signal.is_some());
        setup.kernel.shutdown().await;
    }
}

mod registration {
    use super::*;

    async fn omits_disabled(config: Value, tools: &[&str], sections: &[&str]) {
        let setup = setup(config).await;
        assert_eq!(tool_names(&setup), tools);
        assert_eq!(tool_sections(&setup).await, sections);
        let excluded = ["read", "write", "edit", "apply_patch"]
            .into_iter()
            .find(|name| !tools.contains(name))
            .unwrap();
        assert!(
            call(&setup.ctx, excluded, json!({ "file_path": "a.txt" }), None)
                .await
                .is_error()
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn omits_disabled_tools_and_their_guidance_read_false() {
        omits_disabled(
            json!({ "read": false }),
            &["edit", "write"],
            &["tool:edit", "tool:write"],
        )
        .await;
    }

    #[tokio::test]
    async fn omits_disabled_tools_and_their_guidance_write_false() {
        omits_disabled(
            json!({ "write": false }),
            &["edit", "read"],
            &["tool:edit", "tool:read"],
        )
        .await;
    }

    #[tokio::test]
    async fn omits_disabled_tools_and_their_guidance_edit_false() {
        omits_disabled(
            json!({ "edit": false }),
            &["read", "write"],
            &["tool:read", "tool:write"],
        )
        .await;
    }

    #[tokio::test]
    async fn omits_disabled_tools_and_their_guidance_apply_patch_beside_edit() {
        omits_disabled(
            json!({ "write": false, "applyPatch": true }),
            &["apply_patch", "edit", "read"],
            &["tool:apply_patch", "tool:edit", "tool:read"],
        )
        .await;
    }

    #[tokio::test]
    async fn omits_disabled_tools_and_their_guidance_everything_off() {
        omits_disabled(
            json!({ "read": false, "readImage": false, "write": false, "edit": false }),
            &[],
            &[],
        )
        .await;
    }

    #[tokio::test]
    async fn rejects_non_boolean_switches_at_configuration_parsing() {
        for config in [
            json!({ "read": "false" }),
            json!({ "readImage": 1 }),
            json!({ "write": [] }),
            json!({ "edit": {} }),
            json!({ "applyPatch": "true" }),
        ] {
            assert!(
                try_setup(config.clone()).await.is_err(),
                "{config} must be rejected at configuration parsing"
            );
        }
    }

    #[tokio::test]
    async fn registers_read_write_and_edit() {
        let setup = setup(json!({})).await;
        assert_eq!(tool_names(&setup), ["edit", "read", "write"]);
        setup.kernel.shutdown().await;
    }

    async fn requests_file_path_first(name: &str, fields: &[&str], hint: &str) {
        let setup = setup(json!({})).await;
        let schema = setup
            .tools
            .schemas(None, None)
            .unwrap()
            .into_iter()
            .find(|tool| tool.name == name)
            .unwrap();
        let parameters = serde_json::to_value(&schema.parameters).unwrap();
        let keys: Vec<&String> = parameters["properties"]
            .as_object()
            .unwrap()
            .keys()
            .take(fields.len() + 1)
            .collect();
        let mut expected: Vec<String> = vec!["file_path".into()];
        expected.extend(fields.iter().map(|field| (*field).to_owned()));
        // The workspace builds serde_json with `preserve_order`, so this is the declaration order.
        assert_eq!(keys, expected.iter().collect::<Vec<_>>());
        assert_eq!(parameters["required"], json!(expected));
        assert!(
            parameters["properties"]["file_path"]["description"]
                .as_str()
                .unwrap()
                .contains(hint)
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn requests_file_path_before_content_fields_for_write() {
        requests_file_path_first(
            "write",
            &["content"],
            "Provide `file_path` before `content` in the arguments.",
        )
        .await;
    }

    #[tokio::test]
    async fn requests_file_path_before_content_fields_for_edit() {
        requests_file_path_first(
            "edit",
            &["old_string", "new_string"],
            "Provide `file_path` before `old_string` and `new_string` in the arguments.",
        )
        .await;
    }

    #[tokio::test]
    async fn declares_read_parallel_safe_while_write_and_edit_remain_exclusive() {
        let setup = setup(json!({})).await;
        let pipeline = hoh_tools::ToolExecutionPipeline::new(setup.ctx.clone()).unwrap();
        let mode = |name: &str, args: Value| {
            pipeline
                .execution_mode(&hoh_tools::ToolExecutionInput::new(
                    "m".into(),
                    name,
                    args,
                    live_signal(),
                ))
                .unwrap()
                .kind()
        };
        assert_eq!(mode("read", json!({ "file_path": "a.txt" })), "parallel");
        assert_eq!(
            mode("write", json!({ "file_path": "a.txt", "content": "x" })),
            "exclusive"
        );
        assert_eq!(
            mode(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "x", "new_string": "y" })
            ),
            "exclusive"
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn registers_prompt_sections_for_each_tool() {
        let setup = setup(json!({})).await;
        let prompt = setup
            .ctx
            .get::<hoh_system_prompt::SystemPromptKey>()
            .unwrap();
        let assembly = prompt
            .assemble(hoh_system_prompt::AssembleContext::default())
            .await
            .unwrap();
        let rendered = hoh_system_prompt::render_prompt(&assembly).unwrap();
        assert!(rendered.contains("Use the read tool"));
        assert!(rendered.contains("before overwriting it with write"));
        assert!(rendered.contains("before editing it"));
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn stays_pending_until_the_fs_service_exists() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<hoh_system_prompt::SystemPrompt>(json!({ "includeRuntimeContext": false }))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tool_executor_native::NativeToolExecutorPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        // No fs provider: the plugin waits on its injections and registers nothing.
        let _pending = ctx.plugin::<hoh_tool_fs::ToolFs>(json!({})).unwrap();
        let tools = ctx.get::<hoh_tools::Tools>().expect("tools service");
        assert_eq!(tools.schemas(None, None).unwrap().len(), 0);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn unregisters_everything_on_fiber_disposal() {
        let setup = setup(json!({})).await;
        // Each tool contributes BOTH a schema and a prompt section; disposal must withdraw both.
        assert_eq!(setup.tools.schemas(None, None).unwrap().len(), 3);
        let prompt = setup
            .ctx
            .get::<hoh_system_prompt::SystemPromptKey>()
            .unwrap();
        let names = |assembly: hoh_system_prompt::PromptAssembly| {
            let mut names: Vec<String> = assembly
                .sections
                .into_iter()
                .map(|section| section.name)
                .collect();
            names.sort();
            names
        };
        assert_eq!(
            names(
                prompt
                    .assemble(hoh_system_prompt::AssembleContext::default())
                    .await
                    .unwrap()
            ),
            [
                "deployment:persona-prefix",
                "deployment:persona-suffix",
                "harness:identity",
                "tool:edit",
                "tool:read",
                "tool:write"
            ]
        );
        setup.tool_fs.dispose().await;
        assert_eq!(setup.tools.schemas(None, None).unwrap().len(), 0);
        // Only the system-prompt plugin's own built-in sections remain.
        assert_eq!(
            names(
                prompt
                    .assemble(hoh_system_prompt::AssembleContext::default())
                    .await
                    .unwrap()
            ),
            [
                "deployment:persona-prefix",
                "deployment:persona-suffix",
                "harness:identity"
            ]
        );
        setup.kernel.shutdown().await;
    }
}

mod read_tool {
    use super::*;

    #[tokio::test]
    async fn formats_line_numbered_content_with_a_footer() {
        let setup = setup(json!({})).await;
        setup.fs.set("a.txt", "hello\nworld");
        let result = call(&setup.ctx, "read", json!({ "file_path": "a.txt" }), None).await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(
            result.value(),
            Some(&json!({
                "path": "/abs/a.txt",
                "offset": 1,
                "lines": [{ "number": 1, "text": "hello" }, { "number": 2, "text": "world" }],
                "totalLines": 2,
            }))
        );
        assert_eq!(
            result.text(),
            "<path>/abs/a.txt</path>\n<type>file</type>\n<content>\n1: hello\n2: world\n\n(End of file - total 2 lines)\n</content>"
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn names_the_exact_offset_in_the_out_of_range_refusal() {
        let setup = setup(json!({})).await;
        setup.fs.set("a.txt", "hello\nworld");
        for (offset, shown) in [
            (json!(1e20), "100000000000000000000"),
            (json!(3), "3"),
            (json!(9007199254740993_u64), "9007199254740992"),
        ] {
            let result = call(
                &setup.ctx,
                "read",
                json!({ "file_path": "a.txt", "offset": offset }),
                None,
            )
            .await;
            assert!(result.is_error(), "{offset}");
            assert_eq!(
                result.text(),
                format!("Error: offset {shown} is out of range for \"/abs/a.txt\" (2 lines)")
            );
            assert_eq!(
                error_info(&result).map(|(_, code)| code),
                Some("FS_NOT_FOUND".into())
            );
        }
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn returns_an_explicit_empty_canonical_line_window_for_an_empty_file() {
        let setup = setup(json!({})).await;
        setup.fs.set("empty.txt", "");
        let result = call(
            &setup.ctx,
            "read",
            json!({ "file_path": "empty.txt" }),
            None,
        )
        .await;
        assert!(!result.is_error());
        assert_eq!(
            result.value(),
            Some(&json!({ "path": "/abs/empty.txt", "offset": 1, "lines": [], "totalLines": 0 }))
        );
        assert!(result.text().contains("(End of file - total 0 lines)"));
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_non_positive_offset_via_arg_validation() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt", "offset": 0 }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("offset must be a positive integer"));
        assert_eq!(error_info(&result), invalid_args());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_fractional_offset_and_a_zero_or_negative_limit() {
        let setup = setup(json!({})).await;
        for args in [
            json!({ "file_path": "a.txt", "offset": 1.5 }),
            json!({ "file_path": "a.txt", "limit": 0 }),
            json!({ "file_path": "a.txt", "limit": -3 }),
        ] {
            let result = call(&setup.ctx, "read", args.clone(), None).await;
            assert!(result.is_error(), "{args}");
            assert!(
                result.text().contains("must be a positive integer"),
                "{}",
                result.text()
            );
            assert_eq!(error_info(&result), invalid_args());
        }
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_limit_above_the_cap() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt", "limit": 99999 }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("less than or equal to 2000"));
        assert_eq!(error_info(&result), invalid_args());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_blank_file_path() {
        let setup = setup(json!({})).await;
        let result = call(&setup.ctx, "read", json!({ "file_path": "   " }), None).await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("file_path must be a non-empty string")
        );
        assert_eq!(error_info(&result), invalid_args());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn records_observed_state_so_a_follow_up_edit_by_the_same_session_is_authorized() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("a.txt", "hello");
        assert!(
            !call(
                &setup.ctx,
                "read",
                json!({ "file_path": "a.txt" }),
                Some(session.clone())
            )
            .await
            .is_error()
        );
        let edited = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "hello", "new_string": "bye" }),
            Some(session),
        )
        .await;
        assert!(!edited.is_error(), "{}", edited.text());
        assert_eq!(
            setup.fs.edit_intents(),
            [Some(FsVersionGuard {
                version: "v1".into()
            })]
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn propagates_fs_not_found_for_an_absent_file() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "read",
            json!({ "file_path": "missing.txt" }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert_eq!(
            error_info(&result).map(|(_, code)| code),
            Some("FS_NOT_FOUND".into())
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_non_regular_target() {
        let setup = setup(json!({})).await;
        setup.fs.set("d", "");
        lock(&setup.fs.state).type_override = Some(FsType::Directory);
        let result = call(&setup.ctx, "read", json!({ "file_path": "d" }), None).await;
        assert!(result.is_error());
        assert_eq!(
            error_info(&result).map(|(_, code)| code),
            Some("FS_NOT_REGULAR_FILE".into())
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn streams_a_large_file_instead_of_reading_whole() {
        let setup = setup(json!({})).await;
        setup.fs.set("big.txt", "alpha\nbeta");
        lock(&setup.fs.state).fixed_size = Some(STREAM_MIN_SIZE as u64);
        let result = call(&setup.ctx, "read", json!({ "file_path": "big.txt" }), None).await;
        assert!(!result.is_error());
        assert!(result.text().contains("1: alpha"));
        assert!(setup.fs.stream_text_calls.load(Ordering::SeqCst) > 0);
        assert_eq!(setup.fs.read_text_calls.load(Ordering::SeqCst), 0);
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn streams_when_the_backend_reports_no_size() {
        let setup = setup(json!({})).await;
        setup.fs.set("a.txt", "alpha");
        lock(&setup.fs.state).hide_size = true;
        let result = call(&setup.ctx, "read", json!({ "file_path": "a.txt" }), None).await;
        assert!(!result.is_error());
        assert!(setup.fs.stream_text_calls.load(Ordering::SeqCst) > 0);
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn surfaces_a_byte_capped_read_as_a_truncated_footer() {
        let setup = setup(json!({})).await;
        // Many long lines so the window hits the byte cap before EOF.
        setup
            .fs
            .set("big.txt", &vec!["y".repeat(100); 2000].join("\n"));
        let result = call(&setup.ctx, "read", json!({ "file_path": "big.txt" }), None).await;
        assert!(!result.is_error());
        assert!(result.text().contains("Output capped."));
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn attaches_the_structured_window_as_presentation_meta_and_present_result_narrows_it() {
        let setup = setup(json!({})).await;
        setup.fs.set("a.ts", "const x = 1\nconst y = 2");
        let result = call(&setup.ctx, "read", json!({ "file_path": "a.ts" }), None).await;
        assert!(!result.is_error());
        // The extension drives the lang hint; the window rides on persisted meta.
        assert_eq!(
            result.meta(),
            Some(&json!({
                "path": "/abs/a.ts",
                "offset": 1,
                "lines": [{ "number": 1, "text": "const x = 1" }, { "number": 2, "text": "const y = 2" }],
                "totalLines": 2,
                "lang": "ts",
            }))
        );
        let definition = setup.tools.get("read", None, None).unwrap();
        let present = definition.present_result.as_ref().unwrap();
        let view = present(&json!({ "file_path": "a.ts" }), &to_result(&result));
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            json!({
                "card": "read",
                "path": "/abs/a.ts",
                "offset": 1,
                "lines": [{ "number": 1, "text": "const x = 1" }, { "number": 2, "text": "const y = 2" }],
                "totalLines": 2,
                "lang": "ts",
                "content": [{ "type": "text", "text": "1: const x = 1\n2: const y = 2\n\n(End of file - total 2 lines)" }],
            })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn omits_the_lang_hint_in_meta_for_an_extension_that_maps_to_no_language() {
        let setup = setup(json!({})).await;
        setup.fs.set("notes", "plain");
        let result = call(&setup.ctx, "read", json!({ "file_path": "notes" }), None).await;
        assert!(!result.is_error());
        assert_eq!(
            result.meta(),
            Some(
                &json!({ "path": "/abs/notes", "offset": 1, "lines": [{ "number": 1, "text": "plain" }], "totalLines": 1 })
            )
        );
        setup.kernel.shutdown().await;
    }
}

mod format_read_output_footer_variants {
    use super::*;

    fn base() -> FileReadOutcome {
        FileReadOutcome {
            offset: 1,
            lines: vec![FileTextLine {
                number: 1,
                text: "x".into(),
            }],
            total_lines: 1,
            truncated_by_bytes: false,
        }
    }

    #[test]
    fn reports_a_byte_capped_read() {
        let out = format_read_output(
            "/f",
            &FileReadOutcome {
                total_lines: 99,
                truncated_by_bytes: true,
                ..base()
            },
        );
        assert!(out.contains("(Output capped. Showing lines 1-1. Use offset=2 to continue.)"));
    }

    #[test]
    fn reports_a_more_remaining_page() {
        let out = format_read_output(
            "/f",
            &FileReadOutcome {
                total_lines: 99,
                ..base()
            },
        );
        assert!(out.contains("(Showing lines 1-1 of 99. Use offset=2 to continue.)"));
    }

    #[test]
    fn reports_end_of_file() {
        assert!(format_read_output("/f", &base()).contains("(End of file - total 1 lines)"));
    }

    #[test]
    fn renders_an_empty_file_as_just_the_footer() {
        let out = format_read_output(
            "/f",
            &FileReadOutcome {
                lines: vec![],
                total_lines: 0,
                ..base()
            },
        );
        assert!(out.contains("(End of file - total 0 lines)"));
        assert!(!out.contains(": "));
    }
}

mod write_tool {
    use super::*;

    #[tokio::test]
    async fn formats_a_create_result_and_uses_create_if_absent() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "hi" }),
            Some(agent()),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(
            result.value(),
            Some(
                &json!({ "path": "/abs/a.txt", "operation": "create", "before": null, "after": "hi" })
            )
        );
        assert!(result.text().contains("Created file"));
        assert_eq!(
            setup.fs.write_intents(),
            [Some(FsWriteIntent::CreateIfAbsent)]
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_blank_file_path() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "   ", "content": "hi" }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("file_path must be a non-empty string")
        );
        assert_eq!(error_info(&result), invalid_args());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn propagates_a_backend_fs_error_as_an_error_result_carrying_its_code_and_remedy() {
        let setup = setup(json!({})).await;
        setup.fs.reject_with("blocked", FsErrorCode::StaleVersion);
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "hi" }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert_eq!(error_info(&result), named("FsError", "FS_STALE_VERSION"));
        assert!(result.text().contains("re-read the file, then retry"));
        setup.kernel.shutdown().await;
    }
}

mod edit_tool {
    use super::*;

    #[tokio::test]
    async fn formats_a_single_replacement_success_after_a_read() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("a.txt", "a");
        call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt" }),
            Some(session.clone()),
        )
        .await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "a", "new_string": "b" }),
            Some(session),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(
            result.value(),
            Some(&json!({ "path": "/abs/a.txt", "before": "a", "after": "b" }))
        );
        assert_eq!(
            result.text(),
            "The file /abs/a.txt has been updated successfully."
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn formats_the_replace_all_success_message_distinctly() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("a.txt", "a a a");
        call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt" }),
            Some(session.clone()),
        )
        .await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "a", "new_string": "b", "replace_all": true }),
            Some(session),
        )
        .await;
        assert_eq!(
            result.text(),
            "The file /abs/a.txt has been updated. All occurrences were successfully replaced."
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_identical_old_and_new_strings() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "x", "new_string": "x" }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("must differ"));
        assert_eq!(error_info(&result), invalid_args());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_an_empty_old_string() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "", "new_string": "x" }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("old_string must be a non-empty string")
        );
        assert_eq!(error_info(&result), invalid_args());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_blank_file_path() {
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "  ", "old_string": "a", "new_string": "b" }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("file_path must be a non-empty string")
        );
        assert_eq!(error_info(&result), invalid_args());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn propagates_fs_not_observed_when_the_file_was_never_read() {
        let setup = setup(json!({})).await;
        setup.fs.set("a.txt", "hello");
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "a", "new_string": "b" }),
            Some(agent()),
        )
        .await;
        assert!(result.is_error());
        assert_eq!(
            error_info(&result).map(|(_, code)| code),
            Some("FS_NOT_OBSERVED".into())
        );
        setup.kernel.shutdown().await;
    }
}

fn present_call(setup: &Setup, name: &str, args: Value) -> Value {
    let definition = setup.tools.get(name, None, None).unwrap();
    let present = definition.present_call.as_ref().unwrap();
    serde_json::to_value(present(&args)).unwrap()
}

fn present_result(setup: &Setup, name: &str, args: Value, result: &hoh_tools::ToolResult) -> Value {
    let definition = setup.tools.get(name, None, None).unwrap();
    let present = definition.present_result.as_ref().unwrap();
    serde_json::to_value(present(&args, result)).unwrap()
}

fn text_result(
    blocks: Vec<ContentBlock>,
    is_error: bool,
    meta: Option<Value>,
) -> hoh_tools::ToolResult {
    hoh_tools::ToolResult {
        content: blocks,
        is_error,
        meta,
    }
}

mod tool_owned_presentation {
    use super::*;

    #[tokio::test]
    async fn read_generic_card_titled_by_file_with_the_read_window() {
        let setup = setup(json!({})).await;
        assert_eq!(
            present_call(
                &setup,
                "read",
                json!({ "file_path": "src/a.ts", "offset": 12, "limit": 40 })
            ),
            json!({ "card": "generic", "title": "Read src/a.ts (12 - 51)", "kind": "read",
                    "locations": [{ "path": "src/a.ts", "line": 12 }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_bare_title_and_line_1_location_when_offset_and_limit_are_unset() {
        let setup = setup(json!({})).await;
        assert_eq!(
            present_call(&setup, "read", json!({ "file_path": "a.txt" })),
            json!({ "card": "generic", "title": "Read a.txt", "kind": "read",
                    "locations": [{ "path": "a.txt", "line": 1 }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_completed_presentation_is_a_read_card_with_the_envelope_stripped() {
        let setup = setup(json!({})).await;
        let meta = json!({ "path": "/tmp/a.ts", "offset": 1, "lines": [{ "number": 1, "text": "hello" }], "totalLines": 1, "lang": "ts" });
        let envelope = "<path>/tmp/a.ts</path>\n<type>file</type>\n<content>\n1: hello\n\n(End of file - total 1 lines)\n</content>";
        assert_eq!(
            present_result(
                &setup,
                "read",
                json!({ "file_path": "a.ts" }),
                &text_result(
                    vec![ContentBlock::text(envelope)],
                    false,
                    Some(meta.clone())
                ),
            ),
            json!({
                "card": "read", "path": "/tmp/a.ts", "offset": 1,
                "lines": [{ "number": 1, "text": "hello" }], "totalLines": 1, "lang": "ts",
                "content": [{ "type": "text", "text": "1: hello\n\n(End of file - total 1 lines)" }],
            })
        );
        // A window whose extension maps to no language omits `lang` from the card.
        assert_eq!(
            present_result(
                &setup,
                "read",
                json!({ "file_path": "notes" }),
                &text_result(
                    vec![ContentBlock::text(
                        "<path>/tmp/notes</path>\n<type>file</type>\n<content>\nbody\n</content>"
                    )],
                    false,
                    Some(
                        json!({ "path": "/tmp/notes", "offset": 1, "lines": [{ "number": 1, "text": "body" }], "totalLines": 1 })
                    ),
                ),
            ),
            json!({
                "card": "read", "path": "/tmp/notes", "offset": 1,
                "lines": [{ "number": 1, "text": "body" }], "totalLines": 1,
                "content": [{ "type": "text", "text": "body" }],
            })
        );
        // Malformed envelope text with valid meta still declines (the fallback text is unavailable).
        assert_eq!(
            present_result(
                &setup,
                "read",
                json!({ "file_path": "a.ts" }),
                &text_result(
                    vec![ContentBlock::text("malformed replay")],
                    false,
                    Some(meta)
                ),
            ),
            Value::Null
        );
        // Valid envelope but absent/malformed meta declines to the generic fallback.
        let bare = "<path>/tmp/a.ts</path>\n<type>file</type>\n<content>\n1: hello\n</content>";
        assert_eq!(
            present_result(
                &setup,
                "read",
                json!({ "file_path": "a.ts" }),
                &text_result(vec![ContentBlock::text(bare)], false, None)
            ),
            Value::Null
        );
        assert_eq!(
            present_result(
                &setup,
                "read",
                json!({ "file_path": "a.ts" }),
                &text_result(
                    vec![ContentBlock::text(bare)],
                    false,
                    Some(json!({ "path": "/tmp/a.ts", "lines": "nope", "totalLines": 1 })),
                ),
            ),
            Value::Null
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_completed_presentation_declines_errors_and_non_single_text_content() {
        let setup = setup(json!({})).await;
        let envelope = "<path>/tmp/a.txt</path>\n<type>file</type>\n<content>\nbody\n</content>";
        let meta = json!({ "path": "/tmp/a.txt", "offset": 1, "lines": [{ "number": 1, "text": "body" }], "totalLines": 1 });
        let args = json!({ "file_path": "a.txt" });
        assert_eq!(
            present_result(
                &setup,
                "read",
                args.clone(),
                &text_result(vec![ContentBlock::text(envelope)], true, Some(meta.clone()))
            ),
            Value::Null
        );
        assert_eq!(
            present_result(
                &setup,
                "read",
                args.clone(),
                &text_result(
                    vec![ContentBlock::text(envelope), ContentBlock::text("second")],
                    false,
                    Some(meta.clone())
                ),
            ),
            Value::Null
        );
        let reasoning: ContentBlock =
            serde_json::from_value(json!({ "type": "reasoning", "text": envelope })).unwrap();
        assert_eq!(
            present_result(
                &setup,
                "read",
                args,
                &text_result(vec![reasoning], false, Some(meta))
            ),
            Value::Null
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_from_line_n_window_when_only_offset_is_set() {
        let setup = setup(json!({})).await;
        assert_eq!(
            present_call(&setup, "read", json!({ "file_path": "a.txt", "offset": 5 })),
            json!({ "card": "generic", "title": "Read a.txt (from line 5)", "kind": "read",
                    "locations": [{ "path": "a.txt", "line": 5 }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_diff_card_new_file_style_with_location() {
        let setup = setup(json!({})).await;
        assert_eq!(
            present_call(
                &setup,
                "write",
                json!({ "file_path": "out.txt", "content": "hello" })
            ),
            json!({ "card": "diff", "title": "Write out.txt",
                    "diffs": [{ "path": "out.txt", "oldText": null, "newText": "hello" }],
                    "locations": [{ "path": "out.txt" }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_a_limit_with_no_offset_windows_from_line_1() {
        let setup = setup(json!({})).await;
        assert_eq!(
            present_call(&setup, "read", json!({ "file_path": "a.txt", "limit": 10 })),
            json!({ "card": "generic", "title": "Read a.txt (1 - 10)", "kind": "read",
                    "locations": [{ "path": "a.txt", "line": 1 }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_an_empty_old_string_maps_to_old_text_null() {
        // presentCall runs on replay of raw logged args, which parse_edit_args does not gate — an
        // empty old_string must still produce a valid diff (oldText null).
        let setup = setup(json!({})).await;
        assert_eq!(
            present_call(
                &setup,
                "edit",
                json!({ "file_path": "a.txt", "old_string": "", "new_string": "seed" })
            ),
            json!({ "card": "diff", "title": "Edit a.txt",
                    "diffs": [{ "path": "a.txt", "oldText": null, "newText": "seed" }],
                    "locations": [{ "path": "a.txt" }] })
        );
        setup.kernel.shutdown().await;
    }
}

mod result_time_contextual_diff {
    use super::*;

    const WITH_CONTEXT: &str = "a\nb\nc\nOLD\nd\ne\nf\n";
    const HUNK_OLD: &str = "a\nb\nc\nOLD\nd\ne\nf";
    const HUNK_NEW: &str = "a\nb\nc\nNEW\nd\ne\nf";

    #[tokio::test]
    async fn returns_usable_process_paths_when_a_provider_uses_a_different_display_label() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("note.txt", "initial");
        lock(&setup.fs.state).display_prefix = Some("pretty://".into());
        lock(&setup.fs.state).process_path = Some("/execution/current/note.txt".into());
        let read = call(
            &setup.ctx,
            "read",
            json!({ "file_path": "note.txt" }),
            Some(session.clone()),
        )
        .await;
        let write = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "note.txt", "content": "written" }),
            Some(session.clone()),
        )
        .await;
        let edit = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "note.txt", "old_string": "written", "new_string": "edited" }),
            Some(session),
        )
        .await;
        for outcome in [&read, &write, &edit] {
            assert!(!outcome.is_error(), "{}", outcome.text());
            assert_eq!(
                outcome.value().unwrap()["path"],
                "/execution/current/note.txt"
            );
            assert_eq!(
                outcome.meta().unwrap()["path"],
                "/execution/current/note.txt"
            );
        }
        assert_eq!(setup.fs.get("note.txt").as_deref(), Some("edited"));
        assert_eq!(
            write.meta().unwrap()["diffs"][0]["path"],
            "/execution/current/note.txt"
        );
        assert_eq!(
            edit.meta().unwrap()["diffs"][0]["path"],
            "/execution/current/note.txt"
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_execute_attaches_the_applied_hunk_as_meta_diffs() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("a.txt", WITH_CONTEXT);
        call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt" }),
            Some(session.clone()),
        )
        .await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "OLD", "new_string": "NEW" }),
            Some(session),
        )
        .await;
        assert!(!result.is_error());
        assert_eq!(
            result.meta(),
            Some(&json!({
                "path": "/abs/a.txt",
                "diffs": [{ "path": "/abs/a.txt", "oldText": HUNK_OLD, "newText": HUNK_NEW }],
            }))
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_present_result_turns_the_meta_into_a_diff_result_card() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("a.txt", WITH_CONTEXT);
        call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt" }),
            Some(session.clone()),
        )
        .await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "OLD", "new_string": "NEW" }),
            Some(session),
        )
        .await;
        assert_eq!(
            present_result(
                &setup,
                "edit",
                json!({ "file_path": "a.txt", "old_string": "OLD", "new_string": "NEW" }),
                &to_result(&result),
            ),
            json!({ "card": "diff", "title": "Edit a.txt",
                    "diffs": [{ "path": "/abs/a.txt", "oldText": HUNK_OLD, "newText": HUNK_NEW }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_overwrite_attaches_a_contextual_hunk_and_present_result_renders_a_diff_card() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("a.txt", WITH_CONTEXT);
        call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt" }),
            Some(session.clone()),
        )
        .await;
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "a\nb\nc\nNEW\nd\ne\nf\n" }),
            Some(session),
        )
        .await;
        assert!(!result.is_error());
        assert_eq!(
            result.meta(),
            Some(&json!({ "operation": "update", "path": "/abs/a.txt",
                          "diffs": [{ "path": "/abs/a.txt", "oldText": HUNK_OLD, "newText": HUNK_NEW }] }))
        );
        assert_eq!(
            present_result(
                &setup,
                "write",
                json!({ "file_path": "a.txt", "content": "x" }),
                &to_result(&result)
            ),
            json!({ "card": "diff", "title": "Write a.txt",
                    "diffs": [{ "path": "/abs/a.txt", "oldText": HUNK_OLD, "newText": HUNK_NEW }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_create_empty_applied_diff_falls_back_to_the_whole_file_diff_card() {
        // A create has no prior content, yet the completed replacement view must remain a diff
        // instead of clobbering the pending new-file diff with text.
        let setup = setup(json!({})).await;
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "new.txt", "content": "fresh\n" }),
            Some(agent()),
        )
        .await;
        assert!(!result.is_error());
        assert_eq!(
            result.meta(),
            Some(&json!({ "operation": "create", "path": "/abs/new.txt", "diffs": [] }))
        );
        assert_eq!(
            present_result(
                &setup,
                "write",
                json!({ "file_path": "new.txt", "content": "fresh\n" }),
                &to_result(&result),
            ),
            json!({ "card": "diff", "title": "Write new.txt",
                    "diffs": [{ "path": "/abs/new.txt", "oldText": null, "newText": "fresh\n" }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_overwrite_with_identical_content_falls_back_to_a_whole_file_diff() {
        let setup = setup(json!({})).await;
        let session = agent();
        setup.fs.set("a.txt", "same\n");
        call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt" }),
            Some(session.clone()),
        )
        .await;
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "same\n" }),
            Some(session),
        )
        .await;
        assert!(!result.is_error());
        // The operation lets a consumer tell this unchanged overwrite from a create with the same
        // empty hunk list.
        assert_eq!(
            result.meta(),
            Some(&json!({ "operation": "update", "path": "/abs/a.txt", "diffs": [] }))
        );
        assert_eq!(
            present_result(
                &setup,
                "write",
                json!({ "file_path": "a.txt", "content": "same\n" }),
                &to_result(&result),
            ),
            json!({ "card": "diff", "title": "Write a.txt",
                    "diffs": [{ "path": "/abs/a.txt", "oldText": null, "newText": "same\n" }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn present_result_returns_none_on_an_error_result() {
        let setup = setup(json!({})).await;
        let error = text_result(vec![ContentBlock::text("Error: boom")], true, None);
        assert_eq!(
            present_result(
                &setup,
                "edit",
                json!({ "file_path": "a.txt", "old_string": "x", "new_string": "y" }),
                &error
            ),
            Value::Null
        );
        assert_eq!(
            present_result(
                &setup,
                "write",
                json!({ "file_path": "a.txt", "content": "y" }),
                &error
            ),
            Value::Null
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_present_result_returns_none_on_malformed_meta() {
        // edit has no whole-file fallback (only a literal replacement), so a malformed meta yields
        // the generic "updated successfully" rendering.
        let setup = setup(json!({})).await;
        let bad = text_result(
            vec![ContentBlock::text("ok")],
            false,
            Some(json!({ "diffs": "nope" })),
        );
        assert_eq!(
            present_result(
                &setup,
                "edit",
                json!({ "file_path": "a.txt", "old_string": "x", "new_string": "y" }),
                &bad
            ),
            Value::Null
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_present_result_falls_back_to_a_whole_file_diff_on_malformed_meta() {
        // write always renders a diff card so the completed update can't clobber the pending diff
        // with the model-facing text; a malformed meta falls back to the args-derived whole-file
        // diff, same as a create.
        let setup = setup(json!({})).await;
        let bad = text_result(
            vec![ContentBlock::text("ok")],
            false,
            Some(json!({ "diffs": "nope" })),
        );
        assert_eq!(
            present_result(
                &setup,
                "write",
                json!({ "file_path": "a.txt", "content": "y" }),
                &bad
            ),
            json!({ "card": "diff", "title": "Write a.txt",
                    "diffs": [{ "path": "a.txt", "oldText": null, "newText": "y" }] })
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_retains_its_recorded_argument_fallback_without_usable_result_metadata() {
        let setup = setup(json!({})).await;
        for meta in [None, Some(Value::Null), Some(json!([]))] {
            assert_eq!(
                present_result(
                    &setup,
                    "write",
                    json!({ "file_path": "old.txt", "content": "old result" }),
                    &text_result(vec![], false, meta.clone()),
                ),
                json!({ "card": "diff", "title": "Write old.txt",
                        "diffs": [{ "path": "old.txt", "oldText": null, "newText": "old result" }] }),
                "{meta:?}"
            );
        }
        setup.kernel.shutdown().await;
    }
}

mod read_caps_are_plugin_config {
    use super::*;

    #[tokio::test]
    async fn a_configured_read_limit_is_both_the_default_and_the_cap_and_the_schema_names_it() {
        let setup = setup(json!({ "readLimit": 2 })).await;
        setup.fs.set("a.txt", "one\ntwo\nthree\nfour");
        let result = call(&setup.ctx, "read", json!({ "file_path": "a.txt" }), None).await;
        assert!(
            result
                .text()
                .contains("(Showing lines 1-2 of 4. Use offset=3 to continue.)")
        );
        let over = call(
            &setup.ctx,
            "read",
            json!({ "file_path": "a.txt", "limit": 3 }),
            None,
        )
        .await;
        assert!(over.is_error());
        assert!(over.text().contains("less than or equal to 2"));
        assert_eq!(error_info(&over), invalid_args());
        let schemas = setup.tools.schemas(None, None).unwrap();
        let read = schemas.iter().find(|schema| schema.name == "read").unwrap();
        assert!(
            serde_json::to_string(read)
                .unwrap()
                .contains("Defaults to 2.")
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_configured_read_max_line_length_truncates_lines_at_the_configured_length() {
        let setup = setup(json!({ "readMaxLineLength": 4 })).await;
        setup.fs.set("a.txt", "abcdefgh");
        let result = call(&setup.ctx, "read", json!({ "file_path": "a.txt" }), None).await;
        assert!(
            result
                .text()
                .contains("1: abcd... (line truncated to 4 chars)")
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_configured_read_max_bytes_caps_the_window_at_the_configured_bytes() {
        let setup = setup(json!({ "readMaxBytes": 9 })).await;
        setup.fs.set("a.txt", "aaaa\nbbbb\ncccc");
        let result = call(&setup.ctx, "read", json!({ "file_path": "a.txt" }), None).await;
        assert!(!result.is_error());
        assert_eq!(result.value().unwrap()["totalLines"], 3);
        assert!(result.text().contains("Output capped."));
        assert!(!result.text().contains("cccc"));
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_configured_read_stream_min_size_routes_smaller_files_to_the_streaming_path() {
        let setup = setup(json!({ "readStreamMinSize": 5 })).await;
        setup.fs.set("a.txt", "alpha\nbeta");
        let result = call(&setup.ctx, "read", json!({ "file_path": "a.txt" }), None).await;
        assert!(!result.is_error());
        assert!(setup.fs.stream_text_calls.load(Ordering::SeqCst) > 0);
        assert_eq!(setup.fs.read_text_calls.load(Ordering::SeqCst), 0);
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_non_positive_or_fractional_cap_at_load() {
        for (name, config) in [
            ("readLimit", json!({ "readLimit": 0 })),
            ("readLimit", json!({ "readLimit": 2.5 })),
            ("readMaxLineLength", json!({ "readMaxLineLength": -1 })),
            // NaN is not a JSON value; null is the nearest hand-edited form and fails parsing too.
            ("readStreamMinSize", json!({ "readStreamMinSize": 0 })),
        ] {
            let error = match try_setup(config.clone()).await {
                Ok(_) => panic!("{config} must fail at load"),
                Err(error) => error.to_string(),
            };
            assert!(
                error.contains(&format!("tool-fs: {name} must be a positive integer")),
                "{config}: {error}"
            );
        }
    }
}

mod sandbox_escalation_api {
    use super::*;
    use hoh_sandbox::SandboxMode;
    use hoh_user_approval::ApprovalOutcome;

    async fn setup_confining(approval: Option<ApprovalOutcome>) -> (Setup, Option<Arc<Answerer>>) {
        let answerer = approval.map(Answerer::new);
        let setup = try_setup_with(
            Options {
                confining: Some(SandboxMode::WorkspaceWrite),
                sandbox_policy: true,
                approval: answerer.clone(),
            },
            json!({}),
        )
        .await
        .expect("confining setup");
        (setup, answerer)
    }

    fn stamped(setup: &Setup) -> Vec<(String, String, Option<String>)> {
        lock(&setup.fs.state)
            .stamped
            .iter()
            .map(|policy| {
                let policy = policy.as_ref().expect("a confining call is stamped");
                (
                    policy.mode.as_str().to_owned(),
                    policy.workspace_root.clone(),
                    policy.session_id.as_ref().map(ToString::to_string),
                )
            })
            .collect()
    }

    fn session_id(agent: &Arc<dyn Agent>) -> Option<String> {
        Some(agent.session().id().to_string())
    }

    fn properties(setup: &Setup, name: &str) -> Value {
        let schema = setup
            .tools
            .schemas(None, None)
            .unwrap()
            .into_iter()
            .find(|schema| schema.name == name)
            .unwrap();
        serde_json::to_value(&schema.parameters).unwrap()["properties"].clone()
    }

    #[tokio::test]
    async fn fails_load_when_a_confining_filesystem_has_no_shared_sandbox_policy_resolver() {
        let error = match try_setup_with(
            Options {
                confining: Some(SandboxMode::WorkspaceWrite),
                ..Options::default()
            },
            json!({}),
        )
        .await
        {
            Ok(_) => panic!("must fail at load"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains(
                "tool-fs: the mounted filesystem confines but ctx.sandboxPolicy is missing"
            ),
            "{error}"
        );
    }

    #[tokio::test]
    async fn advertises_no_escalation_fields_under_a_non_confining_backend() {
        let setup = setup(json!({})).await;
        for name in ["write", "edit"] {
            let props = properties(&setup, name);
            assert!(props.get("sandbox_permissions").is_none());
            assert!(props.get("justification").is_none());
        }
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn advertises_the_closed_target_vocabulary_on_write_and_edit_under_a_confining_backend() {
        let (setup, _) = setup_confining(None).await;
        for name in ["write", "edit"] {
            let props = properties(&setup, name);
            assert_eq!(
                props["sandbox_permissions"]["enum"],
                json!(["workspace-write", "danger-full-access"])
            );
            assert!(props.get("justification").is_some());
        }
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_plain_write_stamps_the_default_mode_with_the_calling_session_root() {
        let (setup, _) = setup_confining(None).await;
        let agent = agent();
        call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x" }),
            Some(agent.clone()),
        )
        .await;
        assert_eq!(
            stamped(&setup),
            [(
                "workspace-write".into(),
                "/session-project".into(),
                session_id(&agent)
            )]
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_standing_session_override_folds_onto_the_stamp() {
        let (setup, _) = setup_confining(None).await;
        let agent = agent();
        setup.set_standing_mode(&agent, SandboxMode::ReadOnly);
        call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x" }),
            Some(agent.clone()),
        )
        .await;
        assert_eq!(
            stamped(&setup),
            [(
                "read-only".into(),
                "/session-project".into(),
                session_id(&agent)
            )]
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_denied_write_maps_to_the_shared_marker_plus_the_escalation_hint() {
        let (setup, _) = setup_confining(None).await;
        setup.fs.reject_with("denied", FsErrorCode::SandboxDenied);
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x" }),
            Some(agent()),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("[sandbox: file access denied under workspace-write mode]"),
            "{} {:?}",
            result.text(),
            error_info(&result)
        );
        assert!(
            result
                .text()
                .contains("retry this exact operation once with sandbox_permissions")
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn keeps_read_only_denials_when_an_ordinary_write_includes_a_blank_reason() {
        let (setup, answerer) = setup_confining(Some(ApprovalOutcome::AllowedOnce)).await;
        let agent = agent();
        setup.set_standing_mode(&agent, SandboxMode::ReadOnly);
        setup.fs.reject_with("denied", FsErrorCode::SandboxDenied);
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "justification": "" }),
            Some(agent),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("[sandbox: file access denied under read-only mode]"),
            "{} {:?}",
            result.text(),
            error_info(&result)
        );
        assert_eq!(stamped(&setup)[0].0, "read-only");
        assert!(lock(&setup.fs.state).files.is_empty());
        assert_eq!(answerer.unwrap().asked.load(Ordering::SeqCst), 0);
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_non_sandbox_denied_provider_error_passes_through_unchanged() {
        let (setup, _) = setup_confining(None).await;
        setup.fs.reject_with("boom", FsErrorCode::IoError);
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x" }),
            Some(agent()),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("boom"));
        assert!(!result.text().contains("[sandbox:"));
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn an_approved_escalation_stamps_the_granted_mode_onto_that_write() {
        let (setup, _) = setup_confining(Some(ApprovalOutcome::AllowedOnce)).await;
        let agent = agent();
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "sandbox_permissions": "danger-full-access", "justification": "the test needs it" }),
            Some(agent.clone()),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(
            stamped(&setup),
            [(
                "danger-full-access".into(),
                "/session-project".into(),
                session_id(&agent)
            )]
        );
        setup.kernel.shutdown().await;
    }

    /// The real `approval` service end to end: a read-only Session asks to write under
    /// workspace-write, the user answers allowed-once, the write succeeds and the Session carries the
    /// `approval/asked` and `approval/decided` audit pair.
    #[tokio::test]
    async fn a_read_only_session_writes_after_the_real_approval_service_allows_once() {
        let (setup, answerer) = setup_confining(Some(ApprovalOutcome::AllowedOnce)).await;
        let answerer = answerer.expect("the user is mounted");
        let agent = agent();
        setup.set_standing_mode(&agent, SandboxMode::ReadOnly);
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "sandbox_permissions": "workspace-write", "justification": "the file is part of the task" }),
            Some(agent.clone()),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert!(result.text().contains("Created file"), "{}", result.text());
        assert_eq!(answerer.asked.load(Ordering::SeqCst), 1);
        assert_eq!(
            stamped(&setup),
            [(
                "workspace-write".into(),
                "/session-project".into(),
                session_id(&agent)
            )]
        );
        let audit: Vec<(String, Value)> = agent
            .session()
            .snapshot_events(None, None)
            .iter()
            .filter(|event| event.event_type().starts_with("approval/"))
            .map(|event| (event.event_type().to_owned(), event.data().clone()))
            .collect();
        assert_eq!(audit.len(), 2, "{audit:?}");
        assert_eq!(audit[0].0, "approval/asked");
        assert_eq!(audit[0].1["toolName"], "write");
        assert_eq!(
            audit[0].1["reason"],
            "escalate sandbox to workspace-write: the file is part of the task"
        );
        assert_eq!(audit[1].0, "approval/decided");
        assert_eq!(audit[1].1["outcome"], "allowed-once");
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_rejection_by_the_real_approval_service_keeps_the_write_denied() {
        let (setup, answerer) = setup_confining(Some(ApprovalOutcome::Rejected)).await;
        let agent = agent();
        setup.set_standing_mode(&agent, SandboxMode::ReadOnly);
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "sandbox_permissions": "workspace-write", "justification": "needed" }),
            Some(agent),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result.text().contains("the user rejected escalating"),
            "{}",
            result.text()
        );
        assert_eq!(answerer.unwrap().asked.load(Ordering::SeqCst), 1);
        assert!(lock(&setup.fs.state).files.is_empty());
        setup.kernel.shutdown().await;
    }

    async fn writes_under_repeated_mode(mode: SandboxMode) {
        let (setup, _) = setup_confining(None).await;
        let agent = agent();
        setup.set_standing_mode(&agent, mode);
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "sandbox_permissions": mode.as_str(), "justification": "use the current permissions" }),
            Some(agent.clone()),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(
            stamped(&setup),
            [(
                mode.as_str().into(),
                "/session-project".into(),
                session_id(&agent)
            )]
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn writes_under_repeated_workspace_write_without_approval() {
        writes_under_repeated_mode(SandboxMode::WorkspaceWrite).await;
    }

    #[tokio::test]
    async fn writes_under_repeated_danger_full_access_without_approval() {
        writes_under_repeated_mode(SandboxMode::DangerFullAccess).await;
    }

    async fn writes_and_edits_with_unused_reasons(mode: SandboxMode) {
        let (setup, answerer) = setup_confining(Some(ApprovalOutcome::AllowedOnce)).await;
        let agent = agent();
        setup.set_standing_mode(&agent, mode);
        for (index, justification) in [Some(""), None, Some(" \t\n")].into_iter().enumerate() {
            for repeat_mode in [true, false] {
                let file_path = format!("unused-reason-{index}-{repeat_mode}.txt");
                let mut args = json!({ "file_path": file_path });
                if repeat_mode {
                    args["sandbox_permissions"] = json!(mode.as_str());
                }
                if let Some(justification) = justification {
                    args["justification"] = json!(justification);
                }
                let mut write = args.clone();
                write["content"] = json!("before");
                let written = call(&setup.ctx, "write", write, Some(agent.clone())).await;
                assert!(!written.is_error(), "{}", written.text());
                let mut edit = args.clone();
                edit["old_string"] = json!("before");
                edit["new_string"] = json!("after");
                let edited = call(&setup.ctx, "edit", edit, Some(agent.clone())).await;
                assert!(!edited.is_error(), "{}", edited.text());
                assert_eq!(setup.fs.get(&file_path).as_deref(), Some("after"));
            }
        }
        let stamps = lock(&setup.fs.state).stamped.clone();
        assert_eq!(stamps.len(), 12);
        assert!(stamps.iter().all(|policy| {
            policy.as_ref().is_some_and(|policy| {
                policy.mode == mode && policy.workspace_root == "/session-project"
            })
        }));
        assert_eq!(answerer.unwrap().asked.load(Ordering::SeqCst), 0);
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn writes_and_edits_under_workspace_write_with_absent_or_blank_unused_reasons() {
        writes_and_edits_with_unused_reasons(SandboxMode::WorkspaceWrite).await;
    }

    #[tokio::test]
    async fn writes_and_edits_under_danger_full_access_with_absent_or_blank_unused_reasons() {
        writes_and_edits_with_unused_reasons(SandboxMode::DangerFullAccess).await;
    }

    async fn rejects_missing_or_blank_widening_reasons(tool: &str) {
        let (setup, answerer) = setup_confining(Some(ApprovalOutcome::AllowedOnce)).await;
        let agent = agent();
        setup.set_standing_mode(&agent, SandboxMode::ReadOnly);
        for justification in [None, Some(""), Some(" \t\n")] {
            let mut args = json!({
                "file_path": "denied.txt", "content": "x", "old_string": "x", "new_string": "y",
                "sandbox_permissions": "workspace-write",
            });
            if let Some(justification) = justification {
                args["justification"] = json!(justification);
            }
            let result = call(&setup.ctx, tool, args, Some(agent.clone())).await;
            assert!(result.is_error());
            assert!(
                result.text().contains(if justification.is_none() {
                    "requires a justification"
                } else {
                    "expected a non-empty sentence"
                }),
                "{}",
                result.text()
            );
        }
        assert!(lock(&setup.fs.state).stamped.is_empty());
        assert!(lock(&setup.fs.state).files.is_empty());
        assert_eq!(answerer.unwrap().asked.load(Ordering::SeqCst), 0);
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_missing_or_blank_widening_reasons_before_write_mutates() {
        rejects_missing_or_blank_widening_reasons("write").await;
    }

    #[tokio::test]
    async fn rejects_missing_or_blank_widening_reasons_before_edit_mutates() {
        rejects_missing_or_blank_widening_reasons("edit").await;
    }

    #[tokio::test]
    async fn a_rejected_escalation_fails_closed_with_its_own_text_and_never_mutates() {
        let (setup, _) = setup_confining(Some(ApprovalOutcome::Rejected)).await;
        let result = call(
            &setup.ctx,
            "edit",
            json!({ "file_path": "a.txt", "old_string": "x", "new_string": "y", "sandbox_permissions": "danger-full-access", "justification": "the test needs it" }),
            Some(agent()),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("the user rejected escalating this operation to \"danger-full-access\"")
        );
        assert!(lock(&setup.fs.state).stamped.is_empty());
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn escalation_without_an_approval_service_fails_closed() {
        let (setup, _) = setup_confining(None).await;
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "sandbox_permissions": "danger-full-access", "justification": "why" }),
            Some(agent()),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("no approval service is composed"));
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn escalation_with_an_approval_service_but_no_agent_fails_closed() {
        let (setup, _) = setup_confining(Some(ApprovalOutcome::AllowedOnce)).await;
        let result = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "sandbox_permissions": "danger-full-access", "justification": "why" }),
            None,
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("no agent to route it through"));
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_the_escalation_argument_pairing_one_field_without_the_other() {
        let (setup, _) = setup_confining(None).await;
        let missing = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "sandbox_permissions": "danger-full-access" }),
            Some(agent()),
        )
        .await;
        assert!(missing.is_error());
        assert!(
            missing
                .text()
                .contains("sandbox_permissions requires a justification")
        );
        let orphan = call(
            &setup.ctx,
            "write",
            json!({ "file_path": "a.txt", "content": "x", "justification": "why" }),
            Some(agent()),
        )
        .await;
        assert!(orphan.is_error());
        assert!(
            orphan
                .text()
                .contains("justification is only valid together with sandbox_permissions")
        );
        setup.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_unadvertised_escalation_fields_under_a_non_confining_backend() {
        for fields in [
            json!({ "sandbox_permissions": "workspace-write", "justification": "why" }),
            json!({ "sandbox_permissions": "workspace-write" }),
            json!({ "justification": "why" }),
        ] {
            let setup = setup(json!({})).await;
            let mut args = json!({ "file_path": "a.txt", "content": "x" });
            for (key, value) in fields.as_object().unwrap() {
                args[key] = value.clone();
            }
            let result = call(&setup.ctx, "write", args, Some(agent())).await;
            assert!(result.is_error(), "{fields}");
            assert!(
                result.text().contains(
                    "invalid arguments: sandbox_permissions and justification are not available in this composition"
                ),
                "{}",
                result.text()
            );
            assert_eq!(error_info(&result), invalid_args());
            setup.kernel.shutdown().await;
        }
    }
}
