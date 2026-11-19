//! packages/sdk/server/src/query-input.ts.
use crate::command_helpers::{
    BuiltinCommandName, CommandDescriptor, builtin_command_name, resolve_command,
};
use hoh_util_values::{is_js_white_space, js_trim};

pub fn query_command(text: &str, commands: &[CommandDescriptor]) -> Option<String> {
    let trimmed = js_trim(text);
    let after_slash = trimmed.strip_prefix('/')?;
    let separator = trimmed
        .char_indices()
        .find(|(_, c)| is_js_white_space(*c))
        .map(|(i, _)| i);
    let name = separator.map_or(after_slash, |i| &trimmed[1..i]);
    let descriptor = resolve_command(name, commands)?;
    let builtin = builtin_command_name(descriptor)?;
    let raw = separator.map_or("", |i| &trimmed[i..]);
    let input = match raw.chars().next() {
        Some(c) if !matches!(c, ' ' | '\t' | '\r' | '\n') => format!(" {}", &raw[c.len_utf8()..]),
        _ => raw.to_owned(),
    };
    match builtin {
        BuiltinCommandName::Compact if separator.is_none() => Some(format!("/{}", descriptor.name)),
        BuiltinCommandName::Permission if separator.is_some() => {
            Some(format!("/{}{input}", descriptor.name))
        }
        BuiltinCommandName::Plan | BuiltinCommandName::Goal => {
            Some(format!("/{}{input}", descriptor.name))
        }
        _ => None,
    }
}
