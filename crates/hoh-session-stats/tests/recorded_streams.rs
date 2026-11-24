use hoh_session_stats::{
    ActivityStatus, McpServerStatus, PlanSource, ProviderKind, SessionStats, StatsEvent,
    SubagentKind, TokenUsage, explicit_codex_exec, is_codex_mcp_tool,
};
use serde_json::{Value, json};

const LIVE_CLAUDE_FIXTURES: [(&str, &str); 6] = [
    ("one-turn", include_str!("fixtures/claude-one-turn.jsonl")),
    ("bash", include_str!("fixtures/claude-bash.jsonl")),
    (
        "two-agents",
        include_str!("fixtures/claude-two-agents.jsonl"),
    ),
    ("skill", include_str!("fixtures/claude-skill.jsonl")),
    ("todowrite", include_str!("fixtures/claude-todowrite.jsonl")),
    (
        "background",
        include_str!("fixtures/claude-background.jsonl"),
    ),
];

fn values(stream: &str) -> Vec<Value> {
    stream
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("fixture line is JSON"))
        .collect()
}

fn fold_claude(stream: &str) -> SessionStats {
    let mut stats = SessionStats::new();
    for value in values(stream) {
        stats.apply_claude_stream_value(&value);
    }
    stats
}

fn usage_from_result(value: &Value) -> TokenUsage {
    let usage = &value["usage"];
    TokenUsage {
        input_tokens: usage["input_tokens"].as_u64(),
        output_tokens: usage["output_tokens"].as_u64(),
        cache_read_tokens: usage["cache_read_input_tokens"].as_u64(),
        cache_write_tokens: usage["cache_creation_input_tokens"].as_u64(),
        reasoning_tokens: usage["output_tokens_details"]["thinking_tokens"].as_u64(),
    }
}

fn sum(left: &TokenUsage, right: &TokenUsage) -> TokenUsage {
    let add = |left: Option<u64>, right: Option<u64>| {
        Some(left.expect("recorded result field") + right.expect("recorded result field"))
    };
    TokenUsage {
        input_tokens: add(left.input_tokens, right.input_tokens),
        output_tokens: add(left.output_tokens, right.output_tokens),
        cache_read_tokens: add(left.cache_read_tokens, right.cache_read_tokens),
        cache_write_tokens: add(left.cache_write_tokens, right.cache_write_tokens),
        reasoning_tokens: add(left.reasoning_tokens, right.reasoning_tokens),
    }
}

#[test]
fn six_live_claude_streams_match_every_result_usage_field() {
    for (name, stream) in LIVE_CLAUDE_FIXTURES {
        for forbidden_path in ["/home/save", "/mnt/toshiba", "/tmp/claude-1000"] {
            assert!(
                !stream.contains(forbidden_path),
                "{name} leaked path prefix {forbidden_path}"
            );
        }
        let parsed = values(stream);
        let reconstructed_deltas: String = parsed
            .iter()
            .filter_map(|value| value.get("event")?.get("delta")?.get("text")?.as_str())
            .collect();
        for forbidden_path in ["/home/save", "/mnt/toshiba", "/tmp/claude-1000"] {
            assert!(
                !reconstructed_deltas.contains(forbidden_path),
                "{name} leaked reconstructed path prefix {forbidden_path}"
            );
        }
        let results: Vec<_> = parsed
            .iter()
            .filter(|value| value["type"] == "result")
            .collect();
        assert!(!results.is_empty(), "{name} has no live result");
        let expected = results
            .iter()
            .map(|value| usage_from_result(value))
            .reduce(|left, right| sum(&left, &right))
            .expect("at least one result");
        let stats = fold_claude(stream);
        assert_eq!(
            stats.snapshot().tokens.conversation.as_ref(),
            Some(&expected),
            "{name} conversation usage differs from recorded result.usage"
        );
        assert_eq!(
            stats.snapshot().tokens.turn.as_ref(),
            results
                .last()
                .map(|value| usage_from_result(value))
                .as_ref(),
            "{name} last-turn usage differs from the final recorded result.usage"
        );
        assert_eq!(stats.snapshot().provider, Some(ProviderKind::Claude));
        assert!(
            stats.snapshot().cost_usd.is_some(),
            "{name} lost Claude cost"
        );
    }
}

#[test]
fn recorded_claude_streams_cover_requested_features() {
    let bash = fold_claude(LIVE_CLAUDE_FIXTURES[1].1);
    assert_eq!(bash.snapshot().tool_call_count, Some(1));

    let agents = fold_claude(LIVE_CLAUDE_FIXTURES[2].1);
    let subagents = agents
        .snapshot()
        .subagents
        .as_ref()
        .expect("subagents observed");
    assert_eq!(subagents.len(), 2);
    assert!(
        subagents
            .iter()
            .all(|agent| agent.kind == SubagentKind::Claude)
    );
    assert!(
        subagents
            .iter()
            .all(|agent| agent.status == ActivityStatus::Finished)
    );

    let skill = fold_claude(LIVE_CLAUDE_FIXTURES[3].1);
    let invoked = skill
        .snapshot()
        .skills
        .invoked
        .as_ref()
        .expect("Skill call");
    assert_eq!(invoked.len(), 1);
    assert_eq!(invoked[0].name, "hoh43-fixture:fixture-skill");
    assert!(
        skill
            .snapshot()
            .skills
            .available
            .as_ref()
            .is_some_and(|skills| skills.contains(&"hoh43-fixture:fixture-skill".to_owned()))
    );
    assert!(
        skill
            .snapshot()
            .mcp_servers
            .as_ref()
            .is_some_and(|servers| {
                servers
                    .iter()
                    .any(|server| server.status == McpServerStatus::Connected)
            })
    );

    let todo = fold_claude(LIVE_CLAUDE_FIXTURES[4].1);
    let plan = todo.snapshot().plan.as_ref().expect("TodoWrite plan");
    assert_eq!(plan.source, PlanSource::TodoWrite);
    assert!(plan.text.contains("inspect fixture"));
    assert!(plan.text.contains("verify totals"));

    let background = fold_claude(LIVE_CLAUDE_FIXTURES[5].1);
    let jobs = background
        .snapshot()
        .background_jobs
        .as_ref()
        .expect("background task");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, ActivityStatus::Finished);
}

#[test]
fn goal_is_first_request_and_plan_is_latest_explicit_tool_only() {
    let mut stats = SessionStats::new();
    assert!(stats.apply(StatsEvent::UserPrompt {
        text: "  Ship the panel  ".to_owned(),
    }));
    assert!(!stats.apply(StatsEvent::UserPrompt {
        text: "A later request".to_owned(),
    }));
    assert_eq!(stats.snapshot().goal.as_deref(), Some("Ship the panel"));
    assert!(stats.snapshot().plan.is_none());

    stats.apply(StatsEvent::ToolCall {
        id: "todo".to_owned(),
        name: "TodoWrite".to_owned(),
        input: json!({"todos":[{"content":"first","status":"in_progress"}]}),
        parent_tool_use_id: None,
    });
    assert_eq!(
        stats.snapshot().plan.as_ref().map(|plan| plan.source),
        Some(PlanSource::TodoWrite)
    );
    stats.apply(StatsEvent::ToolCall {
        id: "exit".to_owned(),
        name: "ExitPlanMode".to_owned(),
        input: json!({"plan":"Use the reducer, then wire it."}),
        parent_tool_use_id: None,
    });
    let plan = stats.snapshot().plan.as_ref().expect("latest plan");
    assert_eq!(plan.source, PlanSource::ExitPlanMode);
    assert_eq!(plan.text, "Use the reducer, then wire it.");

    stats.apply(StatsEvent::ToolCall {
        id: "nested-todo".to_owned(),
        name: "TodoWrite".to_owned(),
        input: json!({"todos":[{"content":"child-only plan","status":"in_progress"}]}),
        parent_tool_use_id: Some("agent-call".to_owned()),
    });
    let plan = stats.snapshot().plan.as_ref().expect("root plan remains");
    assert_eq!(plan.source, PlanSource::ExitPlanMode);
    assert_eq!(plan.text, "Use the reducer, then wire it.");
}

#[test]
fn nested_user_prompt_cannot_become_the_conversation_goal() {
    let mut stats = SessionStats::new();
    stats.apply_claude_stream_value(&json!({
        "type":"user",
        "parent_tool_use_id":"agent-call",
        "message":{"content":"child prompt"}
    }));
    stats.apply_claude_stream_value(&json!({
        "type":"user",
        "parent_tool_use_id":null,
        "message":{"content":"root prompt"}
    }));
    assert_eq!(stats.snapshot().goal.as_deref(), Some("root prompt"));
}

#[test]
fn revision_advances_on_changes_but_not_replay_or_ignored_events() {
    let mut stats = SessionStats::new();
    assert_eq!(stats.snapshot().revision, 0);
    let event = StatsEvent::UserPrompt {
        text: "Goal".to_owned(),
    };
    assert!(stats.apply(event.clone()));
    assert_eq!(stats.snapshot().revision, 1);
    assert!(!stats.apply(event));
    assert_eq!(stats.snapshot().revision, 1);

    let tool = StatsEvent::ToolCall {
        id: "call".to_owned(),
        name: "Bash".to_owned(),
        input: json!({"command":"printf ok"}),
        parent_tool_use_id: None,
    };
    assert!(stats.apply(tool.clone()));
    let revision = stats.snapshot().revision;
    assert!(!stats.apply(tool));
    assert_eq!(stats.snapshot().revision, revision);
}

#[test]
fn background_snapshot_replaces_provisional_id_and_marks_disappearance_unknown() {
    let mut stats = SessionStats::new();
    stats.apply(StatsEvent::ToolCall {
        id: "bash-call".to_owned(),
        name: "Bash".to_owned(),
        input: json!({"command":"sleep 1","run_in_background":true}),
        parent_tool_use_id: None,
    });
    stats.apply(StatsEvent::SystemTask {
        id: "task-id".to_owned(),
        tool_use_id: None,
        label: Some("provisional".to_owned()),
        status: ActivityStatus::Running,
    });
    stats.apply(StatsEvent::SystemTask {
        id: "task-id".to_owned(),
        tool_use_id: Some("bash-call".to_owned()),
        label: Some("stable".to_owned()),
        status: ActivityStatus::Running,
    });
    assert_eq!(stats.snapshot().background_jobs.as_ref().unwrap().len(), 1);
    stats.apply(StatsEvent::BackgroundTasksSnapshot {
        active_task_ids: Vec::new(),
    });
    let job = &stats.snapshot().background_jobs.as_ref().unwrap()[0];
    assert_eq!(job.id, "bash-call");
    assert_eq!(job.status, ActivityStatus::Unknown);
}

#[test]
fn claude_model_ledger_reset_starts_a_new_segment_without_losing_history() {
    let mut stats = SessionStats::new();
    stats.apply(StatsEvent::ClaudeResult {
        id: "one".to_owned(),
        ledger_id: Some("process".to_owned()),
        usage: TokenUsage {
            input_tokens: Some(2),
            output_tokens: Some(3),
            cache_read_tokens: Some(5),
            cache_write_tokens: Some(7),
            reasoning_tokens: Some(1),
        },
        model_usage: Some(TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(50),
            cache_read_tokens: Some(200),
            cache_write_tokens: Some(300),
            reasoning_tokens: Some(10),
        }),
        total_cost_usd: Some(1.25),
        duration_ms: Some(10),
    });
    stats.apply(StatsEvent::ClaudeResult {
        id: "two".to_owned(),
        ledger_id: Some("process".to_owned()),
        usage: TokenUsage {
            input_tokens: Some(11),
            output_tokens: Some(13),
            cache_read_tokens: Some(17),
            cache_write_tokens: Some(19),
            reasoning_tokens: Some(2),
        },
        // A lower ledger is a provider/process reset; its new segment is added once.
        model_usage: Some(TokenUsage {
            input_tokens: Some(40),
            output_tokens: Some(20),
            cache_read_tokens: Some(80),
            cache_write_tokens: Some(120),
            reasoning_tokens: Some(4),
        }),
        total_cost_usd: Some(0.5),
        duration_ms: Some(15),
    });
    assert_eq!(
        stats.snapshot().tokens.conversation,
        Some(TokenUsage {
            input_tokens: Some(13),
            output_tokens: Some(16),
            cache_read_tokens: Some(22),
            cache_write_tokens: Some(26),
            reasoning_tokens: Some(3),
        })
    );
    assert_eq!(
        stats.snapshot().tokens.provider_total,
        Some(TokenUsage {
            input_tokens: Some(140),
            output_tokens: Some(70),
            cache_read_tokens: Some(280),
            cache_write_tokens: Some(420),
            reasoning_tokens: Some(14),
        })
    );
    assert_eq!(stats.snapshot().cost_usd, Some(1.75));
}

#[test]
fn claude_cumulative_ledgers_contribute_only_growth() {
    let mut stats = SessionStats::new();
    for (id, main, ledger, cost) in [("one", 5, 100, 1.0), ("two", 7, 140, 1.4)] {
        stats.apply(StatsEvent::ClaudeResult {
            id: id.to_owned(),
            ledger_id: Some("one-process".to_owned()),
            usage: usage(main, main),
            model_usage: Some(usage(ledger, ledger)),
            total_cost_usd: Some(cost),
            duration_ms: None,
        });
    }
    assert_eq!(stats.snapshot().tokens.conversation, Some(usage(12, 12)));
    assert_eq!(
        stats.snapshot().tokens.provider_total,
        Some(usage(140, 140))
    );
    assert!((stats.snapshot().cost_usd.unwrap() - 1.4).abs() < f64::EPSILON);
}

#[test]
fn documented_synthetic_codex_stream_uses_total_and_delta_or_last() {
    let stream = include_str!("fixtures/codex-token-usage-synthetic.jsonl");
    let parsed = values(stream);
    assert_eq!(parsed[0]["_fixture"]["synthetic"], true);
    assert_eq!(parsed[0]["_fixture"]["live"], false);
    let mut stats = SessionStats::new();
    for value in &parsed {
        stats.apply_codex_notification(value);
    }
    let expected = TokenUsage {
        input_tokens: Some(150),
        output_tokens: Some(35),
        cache_read_tokens: Some(90),
        cache_write_tokens: Some(15),
        reasoning_tokens: Some(9),
    };
    assert_eq!(stats.snapshot().tokens.conversation, Some(expected.clone()));
    assert_eq!(
        stats.snapshot().tokens.provider_total,
        Some(expected.clone())
    );
    assert_eq!(stats.snapshot().tokens.turn, Some(expected));
    assert_eq!(stats.snapshot().provider, Some(ProviderKind::Codex));
    assert_eq!(stats.snapshot().turn_count, Some(1));
    assert_eq!(stats.snapshot().cost_usd, None);
}

#[test]
fn codex_counter_regression_falls_back_to_last_without_fabricating_cost() {
    let mut stats = SessionStats::new();
    stats.apply(StatsEvent::CodexTurnStarted {
        id: "turn".to_owned(),
    });
    stats.apply(StatsEvent::CodexTokenUsageUpdated {
        turn_id: "turn".to_owned(),
        total: usage(100, 20),
        last: usage(10, 2),
    });
    stats.apply(StatsEvent::CodexTokenUsageUpdated {
        turn_id: "turn".to_owned(),
        total: usage(5, 1),
        last: usage(3, 1),
    });
    stats.apply(StatsEvent::CodexTurnFinished {
        id: "turn".to_owned(),
    });
    assert_eq!(
        stats.snapshot().tokens.turn,
        Some(TokenUsage {
            input_tokens: Some(13),
            output_tokens: Some(3),
            cache_read_tokens: Some(13),
            cache_write_tokens: Some(13),
            reasoning_tokens: Some(3),
        })
    );
    assert_eq!(stats.snapshot().tokens.conversation, Some(usage(5, 1)));
    assert!(stats.snapshot().cost_usd.is_none());
}

#[test]
fn codex_optional_cache_write_does_not_force_last_for_every_counter() {
    let mut stats = SessionStats::new();
    stats.apply(StatsEvent::CodexTurnStarted {
        id: "turn".to_owned(),
    });
    let mut first = usage(100, 20);
    first.cache_write_tokens = None;
    let mut first_last = usage(10, 2);
    first_last.cache_write_tokens = None;
    stats.apply(StatsEvent::CodexTokenUsageUpdated {
        turn_id: "turn".to_owned(),
        total: first,
        last: first_last,
    });
    let second = usage(150, 35);
    stats.apply(StatsEvent::CodexTokenUsageUpdated {
        turn_id: "turn".to_owned(),
        total: second,
        // Deliberately larger than the real delta: falling back would overcount.
        last: usage(90, 25),
    });
    stats.apply(StatsEvent::CodexTurnFinished {
        id: "turn".to_owned(),
    });
    assert_eq!(
        stats.snapshot().tokens.turn,
        Some(TokenUsage {
            input_tokens: Some(60),
            output_tokens: Some(17),
            cache_read_tokens: Some(60),
            cache_write_tokens: None,
            reasoning_tokens: Some(17),
        })
    );
}

fn usage(input: u64, output: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_tokens: Some(input),
        cache_write_tokens: Some(input),
        reasoning_tokens: Some(output),
    }
}

#[test]
fn codex_launch_detection_accepts_only_explicit_argv_and_exact_mcp_server() {
    for accepted in [
        json!({"command":"codex exec --json 'say hello'"}),
        json!({"command":"/usr/local/bin/codex exec --full-auto task"}),
        json!({"command":["codex","exec","task"]}),
    ] {
        assert!(explicit_codex_exec(&accepted), "should accept {accepted}");
    }
    for rejected in [
        json!({"command":"echo codex exec task"}),
        json!({"command":"codex exec task && echo done"}),
        json!({"command":"bash -c 'codex exec task'"}),
        json!({"command":"codex-exec task"}),
        json!({"command":"codex $SUBCOMMAND task"}),
        json!({"command":["wrapper","codex","exec"]}),
    ] {
        assert!(!explicit_codex_exec(&rejected), "should reject {rejected}");
    }
    assert!(is_codex_mcp_tool("mcp__codex__exec"));
    assert!(is_codex_mcp_tool("mcp__CODEX__review"));
    assert!(!is_codex_mcp_tool("mcp__my_codex__exec"));
    assert!(!is_codex_mcp_tool("codex__exec"));
}

#[test]
fn detected_codex_launch_has_required_marker_and_invisible_steps_note() {
    let mut stats = SessionStats::new();
    stats.apply(StatsEvent::ToolCall {
        id: "codex-call".to_owned(),
        name: "Bash".to_owned(),
        input: json!({"command":"codex exec task"}),
        parent_tool_use_id: None,
    });
    let subagent = &stats.snapshot().subagents.as_ref().unwrap()[0];
    assert_eq!(subagent.kind, SubagentKind::Codex);
    assert_eq!(subagent.name, "Codex");
    assert_eq!(subagent.note.as_deref(), Some("внутренние шаги не видны"));
}

#[test]
fn unknown_values_serialize_as_absent_not_zeroes() {
    let value = serde_json::to_value(SessionStats::new().snapshot()).unwrap();
    assert!(value.get("costUsd").is_none());
    assert!(value.get("turnCount").is_none());
    assert!(value.get("toolCallCount").is_none());
    assert!(value.get("subagents").is_none());
    assert!(value["tokens"].get("conversation").is_none());
}
