use hoh_commands::builtin::{
    BUILTIN_COMMAND_TOKENS, BuiltinCommandName, builtin_command_name, resolve_command,
};
use hoh_commands::{CommandDefinitionId, CommandDescriptor};

fn descriptor(name: &str, builtin: Option<BuiltinCommandName>) -> CommandDescriptor {
    CommandDescriptor {
        definition_id: builtin.map(|value| CommandDefinitionId::from(value.definition_id())),
        name: name.to_owned(),
        description: name.to_owned(),
        input: None,
    }
}

#[test]
fn identifies_builtin_commands_only_by_definition_id() {
    for name in BuiltinCommandName::ALL {
        assert_eq!(
            builtin_command_name(&descriptor("renamed", Some(name))),
            Some(name)
        );
    }
    assert_eq!(builtin_command_name(&descriptor("compact", None)), None);
}

#[test]
fn resolves_exact_names_before_localized_builtin_aliases() {
    let commands = [
        descriptor("反馈", None),
        descriptor("renamed-feedback", Some(BuiltinCommandName::Feedback)),
    ];
    assert_eq!(resolve_command("反馈", &commands).unwrap().name, "反馈");
    assert_eq!(
        resolve_command(BUILTIN_COMMAND_TOKENS.en.feedback, &commands)
            .unwrap()
            .name,
        "renamed-feedback"
    );
    assert!(resolve_command("missing", &commands).is_none());
}
