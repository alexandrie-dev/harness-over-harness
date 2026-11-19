use hoh_sdk_jsonrpc_server::{
    command_helpers::{BuiltinCommandName, CommandDescriptor},
    query_input::query_command,
};
fn commands() -> Vec<CommandDescriptor> {
    use BuiltinCommandName::*;
    [
        ("compact", Compact),
        ("plan", Plan),
        ("goal", Goal),
        ("permission", Permission),
        ("feedback", Feedback),
        ("export", Export),
    ]
    .into_iter()
    .map(|(name, id)| CommandDescriptor {
        name: name.into(),
        definition_id: Some(id.definition_id().into()),
        description: name.into(),
    })
    .collect()
}
#[test]
fn recognizes_the_desktop_form_1() {
    assert_eq!(
        query_command("/compact", &commands()).as_deref(),
        Some("/compact")
    );
}
#[test]
fn recognizes_the_desktop_form_2() {
    assert_eq!(
        query_command(" \n/compact \t", &commands()).as_deref(),
        Some("/compact")
    );
}
#[test]
fn recognizes_the_desktop_form_3() {
    assert_eq!(
        query_command("/压缩", &commands()).as_deref(),
        Some("/compact")
    );
}
#[test]
fn recognizes_the_desktop_form_4() {
    assert_eq!(
        query_command("/permission workspace-write", &commands()).as_deref(),
        Some("/permission workspace-write")
    );
}
#[test]
fn recognizes_the_desktop_form_5() {
    assert_eq!(
        query_command(" \n/权限\tread-only \t", &commands()).as_deref(),
        Some("/permission\tread-only")
    );
}
#[test]
fn recognizes_the_desktop_form_6() {
    assert_eq!(
        query_command("/permission\ncustom-deployment-preset", &commands()).as_deref(),
        Some("/permission\ncustom-deployment-preset")
    );
}
#[test]
fn recognizes_the_desktop_form_7() {
    assert_eq!(
        query_command("/permission\rread-only", &commands()).as_deref(),
        Some("/permission\rread-only")
    );
}
#[test]
fn recognizes_the_desktop_form_8() {
    assert_eq!(
        query_command("/permission  missing extra", &commands()).as_deref(),
        Some("/permission  missing extra")
    );
}
#[test]
fn recognizes_the_desktop_form_9() {
    assert_eq!(
        query_command("/plan", &commands()).as_deref(),
        Some("/plan")
    );
}
#[test]
fn recognizes_the_desktop_form_10() {
    assert_eq!(
        query_command("/计划", &commands()).as_deref(),
        Some("/plan")
    );
}
#[test]
fn recognizes_the_desktop_form_11() {
    assert_eq!(
        query_command(" \n/plan off\t ", &commands()).as_deref(),
        Some("/plan off")
    );
}
#[test]
fn recognizes_the_desktop_form_12() {
    assert_eq!(
        query_command("/计划\nDesign the change", &commands()).as_deref(),
        Some("/plan\nDesign the change")
    );
}
#[test]
fn recognizes_the_desktop_form_13() {
    assert_eq!(
        query_command("/plan  off extra", &commands()).as_deref(),
        Some("/plan  off extra")
    );
}
#[test]
fn recognizes_the_desktop_form_14() {
    assert_eq!(
        query_command("/goal", &commands()).as_deref(),
        Some("/goal")
    );
}
#[test]
fn recognizes_the_desktop_form_15() {
    assert_eq!(
        query_command("/目标", &commands()).as_deref(),
        Some("/goal")
    );
}
#[test]
fn recognizes_the_desktop_form_16() {
    assert_eq!(
        query_command("/目标\nfinish the task", &commands()).as_deref(),
        Some("/goal\nfinish the task")
    );
}
#[test]
fn recognizes_the_desktop_form_17() {
    assert_eq!(
        query_command("/goal edit new objective", &commands()).as_deref(),
        Some("/goal edit new objective")
    );
}
#[test]
fn recognizes_the_desktop_form_18() {
    assert_eq!(
        query_command("/goal pause", &commands()).as_deref(),
        Some("/goal pause")
    );
}
#[test]
fn recognizes_the_desktop_form_19() {
    assert_eq!(
        query_command("/goal resume", &commands()).as_deref(),
        Some("/goal resume")
    );
}
#[test]
fn recognizes_the_desktop_form_20() {
    assert_eq!(
        query_command("/goal clear", &commands()).as_deref(),
        Some("/goal clear")
    );
}
#[test]
fn normalizes_only_the_first_unicode_separator_1() {
    assert_eq!(
        query_command("/plan　Inspect　the\nchange", &commands()).as_deref(),
        Some("/plan Inspect　the\nchange")
    );
}
#[test]
fn normalizes_only_the_first_unicode_separator_2() {
    assert_eq!(
        query_command("/计划 \t设计 方案", &commands()).as_deref(),
        Some("/plan \t设计 方案")
    );
}
#[test]
fn normalizes_only_the_first_unicode_separator_3() {
    assert_eq!(
        query_command("/goal Finish　the task", &commands()).as_deref(),
        Some("/goal Finish　the task")
    );
}
#[test]
fn normalizes_only_the_first_unicode_separator_4() {
    assert_eq!(
        query_command("/目标　 完成任务", &commands()).as_deref(),
        Some("/goal  完成任务")
    );
}
#[test]
fn normalizes_only_the_first_unicode_separator_5() {
    assert_eq!(
        query_command("/permission read-only", &commands()).as_deref(),
        Some("/permission read-only")
    );
}
#[test]
fn normalizes_only_the_first_unicode_separator_6() {
    assert_eq!(
        query_command("/权限 \tread-only", &commands()).as_deref(),
        Some("/permission \tread-only")
    );
}
#[test]
fn leaves_as_ordinary_text_1() {
    assert_eq!(query_command("ordinary text", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_2() {
    assert_eq!(query_command("/", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_3() {
    assert_eq!(query_command("/unknown", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_4() {
    assert_eq!(query_command("/usr/bin", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_5() {
    assert_eq!(query_command("/COMPACT", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_6() {
    assert_eq!(query_command("/模型", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_7() {
    assert_eq!(query_command("/文件", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_8() {
    assert_eq!(query_command("/compact extra", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_9() {
    assert_eq!(query_command("/压缩 extra", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_10() {
    assert_eq!(query_command("/export path", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_11() {
    assert_eq!(query_command("/model extra", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_12() {
    assert_eq!(query_command("/file path", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_13() {
    assert_eq!(query_command("/plans inspect", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_14() {
    assert_eq!(query_command("/goals finish", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_15() {
    assert_eq!(query_command("/反馈 problem", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_16() {
    assert_eq!(query_command("/permission", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_17() {
    assert_eq!(query_command(" \n/permission \t", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_18() {
    assert_eq!(query_command("/权限", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_19() {
    assert_eq!(query_command("/权限\n\t", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_20() {
    assert_eq!(query_command("/permissions read-only", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_21() {
    assert_eq!(query_command("/导出", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_22() {
    assert_eq!(query_command("/export", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_23() {
    assert_eq!(query_command("/model", &commands()), None);
}
#[test]
fn leaves_as_ordinary_text_24() {
    assert_eq!(query_command("/file", &commands()), None);
}

#[test]
fn does_not_identify_unrelated_scoped_replacements_as_builtins() {
    let replaced = vec![
        CommandDescriptor {
            name: "compact".into(),
            definition_id: None,
            description: "Scoped command".into(),
        },
        CommandDescriptor {
            name: "permission".into(),
            definition_id: None,
            description: "Scoped permission".into(),
        },
    ];
    for text in [
        "/compact",
        "/压缩",
        "/permission read-only",
        "/权限 read-only",
    ] {
        assert_eq!(query_command(text, &replaced), None);
    }
    assert_eq!(
        query_command(
            "/计划",
            &[CommandDescriptor {
                name: "plan".into(),
                definition_id: None,
                description: "Scoped plan".into()
            }]
        ),
        None
    );
}
#[test]
fn leaves_other_registered_commands_and_unavailable_supported_commands_unchanged() {
    assert_eq!(query_command("/feedback", &commands()), None);
    for text in [
        "/goal",
        "/plan",
        "/compact",
        "/export",
        "/permission read-only",
    ] {
        assert_eq!(query_command(text, &[]), None);
    }
    for (token, name, id, expected) in [
        ("/压缩", "prune", BuiltinCommandName::Compact, "/prune"),
        (
            "/权限 read-only",
            "access",
            BuiltinCommandName::Permission,
            "/access read-only",
        ),
        ("/计划", "planning", BuiltinCommandName::Plan, "/planning"),
    ] {
        assert_eq!(
            query_command(
                token,
                &[CommandDescriptor {
                    name: name.into(),
                    definition_id: Some(id.definition_id().into()),
                    description: "Renamed".into()
                }]
            )
            .as_deref(),
            Some(expected)
        );
    }
    assert_eq!(
        query_command(
            "/目标",
            &[CommandDescriptor {
                name: "goal".into(),
                definition_id: None,
                description: "Scoped".into()
            }]
        ),
        None
    );
}
#[test]
fn preserves_exact_name_priority_over_localized_aliases() {
    let mut descriptors = commands();
    descriptors.push(CommandDescriptor {
        name: "权限".into(),
        definition_id: None,
        description: "Another".into(),
    });
    assert_eq!(query_command("/权限 read-only", &descriptors), None);
}
