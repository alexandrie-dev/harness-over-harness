use crate::suggest_similar::suggest_similar;
use crate::{Argument, CommanderError, InvalidArgumentError, Opt, ParseError};
use indexmap::IndexMap;
use serde_json::Value;
use std::{
    io::{self, Write},
    rc::Rc,
};

pub type Options = IndexMap<String, Value>;
type Action = Rc<dyn Fn(&[Option<Value>], &Options, &mut Command) -> Result<(), ParseError>>;
type ErrorHandler = Rc<dyn Fn(&str, i32, &str) -> CommanderError>;
type Writer = Rc<dyn Fn(&str)>;

/// Output callbacks are shared with commands registered afterwards, as in commander.
#[derive(Clone)]
pub struct Output {
    pub write_out: Writer,
    pub write_err: Writer,
}
impl Output {
    pub fn new(out: impl Fn(&str) + 'static, err: impl Fn(&str) + 'static) -> Self {
        Self {
            write_out: Rc::new(out),
            write_err: Rc::new(err),
        }
    }
}
impl Default for Output {
    fn default() -> Self {
        Self::new(
            |s| {
                let _ = io::stdout().write_all(s.as_bytes());
            },
            |s| {
                let _ = io::stderr().write_all(s.as_bytes());
            },
        )
    }
}

/// The supported explicit argument convention is `{ from: 'user' }`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseOptions;
impl ParseOptions {
    pub const USER: Self = Self;
}

#[derive(Clone)]
struct ExitPolicy {
    output: Output,
    override_exit: bool,
    exit: Option<Rc<dyn Fn(i32)>>,
    error_handler: Option<ErrorHandler>,
}
impl ExitPolicy {
    fn exit(&self, status: i32, code: &str, message: &str) -> CommanderError {
        if !self.override_exit
            && let Some(exit) = &self.exit
        {
            exit(status);
        }
        CommanderError::new(status, code, message)
    }
    fn error(&self, message: &str, status: i32, code: &str) -> CommanderError {
        if let Some(handler) = &self.error_handler {
            return handler(message, status, code);
        }
        (self.output.write_err)(&format!("{message}\n"));
        self.exit(status, code, message)
    }
}

#[derive(Clone)]
struct Ancestor {
    missing: Vec<String>,
    policy: ExitPolicy,
    long_flags: Vec<String>,
    positional: bool,
    digit: bool,
}

pub struct Command {
    pub commands: Vec<Command>,
    pub options: Vec<Opt>,
    pub registered_arguments: Vec<Argument>,
    pub args: Vec<String>,
    pub raw_args: Vec<String>,
    /// None represents JavaScript undefined, including omitted optional arguments.
    pub processed_args: Vec<Option<Value>>,
    pub(crate) command_name: String,
    pub(crate) command_description: String,
    pub(crate) explicit_usage: Option<String>,
    pub(crate) parent_names: Vec<String>,
    pub(crate) help_option_value: Option<Opt>,
    pub(crate) help_command_enabled: Option<bool>,
    pub(crate) help_command_signature: String,
    pub(crate) help_command_description: String,
    pub(crate) help_texts: Vec<(String, String)>,
    pub(crate) inherited_before: Vec<String>,
    pub(crate) inherited_after: Vec<String>,
    allow_unknown: bool,
    allow_excess: bool,
    positional: bool,
    pass_through: bool,
    combine_optional: bool,
    action_handler: Option<Action>,
    values: Options,
    saved_values: Option<Options>,
    seen_options: indexmap::IndexSet<String>,
    policy: ExitPolicy,
    definition_error: Option<String>,
}

impl Default for Command {
    fn default() -> Self {
        Self::new()
    }
}
impl Command {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            options: Vec::new(),
            registered_arguments: Vec::new(),
            args: Vec::new(),
            raw_args: Vec::new(),
            processed_args: Vec::new(),
            command_name: String::new(),
            command_description: String::new(),
            explicit_usage: None,
            parent_names: Vec::new(),
            help_option_value: Some(Opt::new("-h, --help", "display help for command")),
            help_command_enabled: None,
            help_command_signature: "help [command]".into(),
            help_command_description: "display help for command".into(),
            help_texts: Vec::new(),
            inherited_before: Vec::new(),
            inherited_after: Vec::new(),
            allow_unknown: false,
            allow_excess: false,
            positional: false,
            pass_through: false,
            seen_options: indexmap::IndexSet::new(),
            combine_optional: true,
            action_handler: None,
            values: Options::new(),
            saved_values: None,
            policy: ExitPolicy {
                output: Output::default(),
                override_exit: false,
                exit: None,
                error_handler: None,
            },
            definition_error: None,
        }
    }
    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.command_name = name.into();
        self.refresh_children();
        self
    }
    pub fn get_name(&self) -> &str {
        &self.command_name
    }
    pub fn description(&mut self, text: impl Into<String>) -> &mut Self {
        self.command_description = text.into();
        self
    }
    pub fn get_description(&self) -> &str {
        &self.command_description
    }
    pub fn usage(&mut self, text: impl Into<String>) -> &mut Self {
        self.explicit_usage = Some(text.into());
        self
    }
    pub fn get_usage(&self) -> String {
        if let Some(usage) = &self.explicit_usage
            && !usage.is_empty()
        {
            return usage.clone();
        }
        let mut terms = Vec::new();
        if !self.options.is_empty() || self.help_option_value.is_some() {
            terms.push("[options]".into());
        }
        if !self.commands.is_empty() {
            terms.push("[command]".into());
        }
        terms.extend(
            self.registered_arguments
                .iter()
                .map(Argument::human_readable),
        );
        terms.join(" ")
    }
    pub fn option(&mut self, flags: &str, description: &str) -> &mut Self {
        self.add_option(Opt::new(flags, description))
    }
    pub fn option_with_default(
        &mut self,
        flags: &str,
        description: &str,
        default: impl Into<Value>,
    ) -> &mut Self {
        self.add_option(Opt::new(flags, description).default(default))
    }
    pub fn option_with_parser<F>(&mut self, flags: &str, description: &str, parser: F) -> &mut Self
    where
        F: Fn(&str, Option<&Value>) -> Result<Value, InvalidArgumentError> + 'static,
    {
        self.add_option(Opt::new(flags, description).arg_parser(parser))
    }
    pub fn option_with_parser_and_default<F>(
        &mut self,
        flags: &str,
        description: &str,
        parser: F,
        default: impl Into<Value>,
    ) -> &mut Self
    where
        F: Fn(&str, Option<&Value>) -> Result<Value, InvalidArgumentError> + 'static,
    {
        self.add_option(
            Opt::new(flags, description)
                .arg_parser(parser)
                .default(default),
        )
    }
    pub fn required_option(&mut self, flags: &str, description: &str) -> &mut Self {
        self.add_option(Opt::new(flags, description).make_option_mandatory(true))
    }
    pub fn required_option_with_parser<F>(
        &mut self,
        flags: &str,
        description: &str,
        parser: F,
    ) -> &mut Self
    where
        F: Fn(&str, Option<&Value>) -> Result<Value, InvalidArgumentError> + 'static,
    {
        self.add_option(
            Opt::new(flags, description)
                .make_option_mandatory(true)
                .arg_parser(parser),
        )
    }
    pub fn add_option(&mut self, option: Opt) -> &mut Self {
        if let Some(error) = &option.definition_error {
            self.definition_error = Some(error.clone());
        }
        let conflict = option
            .long
            .as_ref()
            .filter(|f| self.find_option(f).is_some())
            .or_else(|| {
                option
                    .short
                    .as_ref()
                    .filter(|f| self.find_option(f).is_some())
            });
        if let Some(flag) = conflict
            && let Some(other) = self.find_option(flag)
        {
            let command = if self.command_name.is_empty() {
                String::new()
            } else {
                format!(" to command '{}'", self.command_name)
            };
            self.definition_error = Some(format!(
                "Cannot add option '{}'{} due to conflicting flag '{}'\n-  already used by option '{}'",
                option.flags, command, flag, self.options[other].flags
            ));
        }
        if let Some(default) = &option.default_value {
            self.values.insert(option.attribute_name(), default.clone());
        }
        self.options.push(option);
        self
    }
    pub fn version(&mut self, version: &str) -> &mut Self {
        self.version_with_flags(version, "-V, --version", "output the version number")
    }
    pub fn version_with_flags(
        &mut self,
        version: &str,
        flags: &str,
        description: &str,
    ) -> &mut Self {
        let mut option = Opt::new(flags, description);
        option.version = Some(version.into());
        self.add_option(option)
    }
    pub fn argument(&mut self, name: &str, description: &str) -> &mut Self {
        self.add_argument(Argument::new(name, description))
    }
    pub fn add_argument(&mut self, argument: Argument) -> &mut Self {
        if self.registered_arguments.last().is_some_and(|a| a.variadic) {
            let name = self
                .registered_arguments
                .last()
                .map(Argument::name)
                .unwrap_or_default();
            self.definition_error =
                Some(format!("only the last argument can be variadic '{name}'"));
        }
        if argument.required
            && argument.default_value.is_some()
            && argument.parser.is_none()
            && argument.arg_choices.is_none()
        {
            self.definition_error = Some(format!(
                "a default value for a required argument is never used: '{}'",
                argument.name()
            ));
        }
        self.registered_arguments.push(argument);
        self
    }
    /// Returns the newly registered child, matching commander's command().
    pub fn command(&mut self, signature: &str) -> &mut Command {
        let mut parts = signature.split_whitespace();
        let name = parts.next().unwrap_or_default();
        let mut child = Self::new();
        child.command_name = name.into();
        child.policy = self.policy.clone();
        // Replacing .error on a parent is not inherited by commander.
        child.policy.error_handler = None;
        child.help_option_value = self.help_option_value.clone();
        child.help_command_signature = self.help_command_signature.clone();
        child.help_command_description = self.help_command_description.clone();
        child.allow_excess = self.allow_excess;
        child.positional = self.positional;
        child.combine_optional = self.combine_optional;
        for part in parts {
            child.argument(part, "");
        }
        if self.commands.iter().any(|c| c.command_name == name) {
            self.definition_error = Some(format!(
                "cannot add command '{name}' as already have command '{name}'"
            ));
        }
        self.commands.push(child);
        self.refresh_children();
        // A child was pushed immediately above, so the vector is nonempty.
        self.commands.last_mut().expect("just registered a command")
    }
    fn refresh_children(&mut self) {
        for child in &mut self.commands {
            child.parent_names = self.parent_names.clone();
            child.parent_names.push(self.command_name.clone());
            child.inherited_before = self.inherited_before.clone();
            child.inherited_before.extend(
                self.help_texts
                    .iter()
                    .filter(|(p, _)| p == "beforeAll")
                    .map(|(_, t)| t.clone()),
            );
            child.inherited_after = self
                .help_texts
                .iter()
                .filter(|(p, _)| p == "afterAll")
                .map(|(_, t)| t.clone())
                .collect();
            child.inherited_after.extend(self.inherited_after.clone());
            child.refresh_children();
        }
    }
    pub fn action<F>(&mut self, action: F) -> &mut Self
    where
        F: Fn(&[Option<Value>], &Options, &mut Command) -> Result<(), ParseError> + 'static,
    {
        self.action_handler = Some(Rc::new(action));
        self
    }
    pub fn has_action(&self) -> bool {
        self.action_handler.is_some()
    }
    pub fn has_action_recursive(&self) -> bool {
        self.has_action() || self.commands.iter().any(Self::has_action_recursive)
    }
    pub fn help_option(&mut self, flags: &str, description: &str) -> &mut Self {
        self.help_option_value = Some(Opt::new(flags, description));
        self
    }
    pub fn help_option_enabled(&mut self, enable: bool) -> &mut Self {
        if enable {
            if self.help_option_value.is_none() {
                self.help_option_value = Some(Opt::new("-h, --help", "display help for command"));
            }
        } else {
            self.help_option_value = None;
        }
        self
    }
    pub fn help_command(&mut self, enable: bool) -> &mut Self {
        self.help_command_enabled = Some(enable);
        self
    }
    pub fn help_command_with_description(
        &mut self,
        signature: &str,
        description: &str,
    ) -> &mut Self {
        self.help_command_enabled = Some(true);
        self.help_command_signature = signature.into();
        self.help_command_description = description.into();
        self
    }
    pub(crate) fn has_help_command(&self) -> bool {
        self.help_command_enabled.unwrap_or(
            !self.commands.is_empty()
                && !self.has_action()
                && !self.commands.iter().any(|c| c.command_name == "help"),
        )
    }
    pub fn add_help_text(&mut self, position: &str, text: impl Into<String>) -> &mut Self {
        if !["before", "after", "beforeAll", "afterAll"].contains(&position) {
            self.definition_error = Some("Unexpected value for position to addHelpText.\nExpecting one of 'beforeAll', 'before', 'after', 'afterAll'".into());
        }
        self.help_texts.push((position.into(), text.into()));
        self.refresh_children();
        self
    }
    pub fn allow_unknown_option(&mut self, allow: bool) -> &mut Self {
        self.allow_unknown = allow;
        self
    }
    pub fn allow_excess_arguments(&mut self, allow: bool) -> &mut Self {
        self.allow_excess = allow;
        self
    }
    pub fn enable_positional_options(&mut self, enable: bool) -> &mut Self {
        self.positional = enable;
        self
    }
    pub fn pass_through_options(&mut self, enable: bool) -> &mut Self {
        self.pass_through = enable;
        self
    }
    pub fn combine_flag_and_optional_value(&mut self, combine: bool) -> &mut Self {
        self.combine_optional = combine;
        self
    }
    pub fn exit_override(&mut self) -> &mut Self {
        self.policy.override_exit = true;
        self
    }
    pub fn exit_callback(&mut self, callback: impl Fn(i32) + 'static) -> &mut Self {
        self.policy.exit = Some(Rc::new(callback));
        self
    }
    pub fn configure_output(&mut self, output: Output) -> &mut Self {
        self.policy.output = output;
        self
    }
    /// Equivalent to replacing `program.error`, as headless does in JSON mode.
    pub fn error_handler(
        &mut self,
        handler: impl Fn(&str, i32, &str) -> CommanderError + 'static,
    ) -> &mut Self {
        self.policy.error_handler = Some(Rc::new(handler));
        self
    }
    pub fn opts(&self) -> &Options {
        &self.values
    }
    pub fn error(&self, message: &str) -> CommanderError {
        self.error_with_options(message, 1, "commander.error")
    }
    pub fn error_with_options(&self, message: &str, exit_code: i32, code: &str) -> CommanderError {
        self.policy.error(
            message,
            if exit_code == 0 { 1 } else { exit_code },
            if code.is_empty() {
                "commander.error"
            } else {
                code
            },
        )
    }
    pub fn help(&self) -> CommanderError {
        self.help_with_error(false)
    }
    pub fn help_with_error(&self, error: bool) -> CommanderError {
        self.output_help(error);
        self.policy
            .exit(i32::from(error), "commander.help", "(outputHelp)")
    }
    pub fn parse<I, S>(&mut self, args: I, _options: ParseOptions) -> Result<&mut Self, ParseError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.prepare();
        if self.command_name.is_empty() {
            self.command_name = "program".into();
        }
        self.refresh_children();
        let args: Vec<_> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
        self.raw_args = args.clone();
        self.parse_command(Vec::new(), args, &[])?;
        Ok(self)
    }
    fn prepare(&mut self) {
        if let Some(values) = &self.saved_values {
            self.values = values.clone();
        } else {
            for option in &self.options {
                if option.negate
                    && option.default_value.is_none()
                    && !self.values.contains_key(&option.attribute_name())
                {
                    let positive = option
                        .long
                        .as_deref()
                        .unwrap_or_default()
                        .replacen("--no-", "--", 1);
                    if self.find_option(&positive).is_none() {
                        self.values
                            .insert(option.attribute_name(), Value::Bool(true));
                    }
                }
            }
            self.saved_values = Some(self.values.clone());
        }
        self.args.clear();
        self.processed_args.clear();
        self.raw_args.clear();
        self.seen_options.clear();
    }
    fn find_option(&self, flag: &str) -> Option<usize> {
        self.options.iter().position(|o| o.is(flag))
    }
    pub(crate) fn write_help(&self, text: &str, error: bool) {
        let writer = if error {
            &self.policy.output.write_err
        } else {
            &self.policy.output.write_out
        };
        writer(&crate::help::strip_ansi(text));
    }
    fn ancestor(&self) -> Ancestor {
        Ancestor {
            missing: self
                .options
                .iter()
                .filter(|o| o.mandatory && !self.values.contains_key(&o.attribute_name()))
                .map(|o| o.flags.clone())
                .collect(),
            policy: self.policy.clone(),
            long_flags: self
                .visible_options()
                .iter()
                .filter_map(|o| o.long.clone())
                .collect(),
            positional: self.positional,
            digit: self.has_digit_option(),
        }
    }
    fn has_digit_option(&self) -> bool {
        self.options
            .iter()
            .filter_map(|o| o.short.as_deref())
            .any(|s| s.len() == 2 && s.as_bytes()[1].is_ascii_digit())
    }
    fn emit_option(
        &mut self,
        index: usize,
        raw: Option<&str>,
        optional_missing: bool,
    ) -> Result<(), CommanderError> {
        let option = &self.options[index];
        if let Some(version) = &option.version {
            (self.policy.output.write_out)(&format!("{version}\n"));
            return Err(self.policy.exit(0, "commander.version", version));
        }
        let key = option.attribute_name();
        let previous = self.values.get(&key);
        let shown = raw.unwrap_or(if option.optional && optional_missing {
            "null"
        } else {
            "undefined"
        });
        let mut value = raw
            .map(|s| Value::String(s.into()))
            .or_else(|| option.preset_arg.clone());
        let skip_parser =
            raw.is_none() && option.optional && optional_missing && option.preset_arg.is_none();
        if !skip_parser
            && (option.parser.is_some() || option.arg_choices.is_some() || option.variadic)
            && value.as_ref() != Some(&Value::Null)
        {
            let parsed = if let Some(parser) = &option.parser {
                let raw_value = value
                    .as_ref()
                    .map(crate::help::js_string)
                    .unwrap_or_else(|| "undefined".into());
                parser(&raw_value, previous)
            } else if option.arg_choices.as_ref().is_some_and(|choices| {
                !value
                    .as_ref()
                    .and_then(Value::as_str)
                    .is_some_and(|s| choices.iter().any(|c| c == s))
            }) {
                Err(InvalidArgumentError::new(format!(
                    "Allowed choices are {}.",
                    option.arg_choices.as_deref().unwrap_or_default().join(", ")
                )))
            } else if option.variadic {
                Ok(crate::collect(
                    value.clone().unwrap_or(Value::Null),
                    previous,
                    !self.seen_options.contains(&key),
                ))
            } else {
                Ok(value.clone().unwrap_or(Value::Null))
            };
            value = Some(parsed.map_err(|error| {
                self.policy.error(
                    &format!(
                        "error: option '{}' argument '{}' is invalid. {}",
                        option.flags, shown, error.message
                    ),
                    error.exit_code,
                    &error.code,
                )
            })?);
        }
        let value = match value {
            Some(Value::Null) | None => {
                if option.negate {
                    Value::Bool(false)
                } else if option.is_boolean() || option.optional {
                    Value::Bool(true)
                } else {
                    Value::String(String::new())
                }
            }
            Some(value) => value,
        };
        self.seen_options.insert(key.clone());
        self.values.insert(key, value);
        Ok(())
    }
    fn parse_options(
        &mut self,
        args: &[String],
        ancestors: &[Ancestor],
    ) -> Result<(Vec<String>, Vec<String>), CommanderError> {
        let mut operands = Vec::new();
        let mut unknown = Vec::new();
        let mut unknown_dest = false;
        let mut active_variadic = None;
        let mut active_group = None;
        let mut i = 0;
        let digit = self.has_digit_option() || ancestors.iter().any(|a| a.digit);
        while i < args.len() || active_group.is_some() {
            let arg = if let Some(group) = active_group.take() {
                group
            } else {
                let arg = args[i].clone();
                i += 1;
                arg
            };
            if arg == "--" {
                let dest = if unknown_dest {
                    unknown.push(arg);
                    &mut unknown
                } else {
                    &mut operands
                };
                dest.extend_from_slice(&args[i..]);
                break;
            }
            if let Some(option) = active_variadic
                && (!maybe_option(&arg) || (!digit && negative_number(&arg)))
            {
                self.emit_option(option, Some(&arg), false)?;
                continue;
            }
            active_variadic = None;
            if maybe_option(&arg)
                && let Some(index) = self.find_option(&arg)
            {
                let option = &self.options[index];
                let variadic = option.variadic;
                let value = if option.required {
                    let Some(value) = args.get(i) else {
                        return Err(self.policy.error(
                            &format!("error: option '{}' argument missing", option.flags),
                            1,
                            "commander.optionMissingArgument",
                        ));
                    };
                    i += 1;
                    Some(value.as_str())
                } else if option.optional
                    && args
                        .get(i)
                        .is_some_and(|v| !maybe_option(v) || (!digit && negative_number(v)))
                {
                    let value = &args[i];
                    i += 1;
                    Some(value.as_str())
                } else {
                    None
                };
                self.emit_option(index, value, value.is_none())?;
                if variadic {
                    active_variadic = Some(index);
                }
                continue;
            }
            if arg.len() > 2
                && arg.starts_with('-')
                && !arg.starts_with("--")
                && let Some(first) = arg[1..].chars().next()
            {
                let end = 1 + first.len_utf8();
                if let Some(index) = self.find_option(&format!("-{first}")) {
                    let option = &self.options[index];
                    if option.required || (option.optional && self.combine_optional) {
                        self.emit_option(index, Some(&arg[end..]), false)?;
                    } else {
                        self.emit_option(index, None, false)?;
                        active_group = Some(format!("-{}", &arg[end..]));
                    }
                    continue;
                }
            }
            if arg.starts_with("--")
                && let Some((flag, value)) = arg.split_once('=')
                && let Some(index) = self.find_option(flag)
                && (self.options[index].required || self.options[index].optional)
            {
                self.emit_option(index, Some(value), false)?;
                continue;
            }
            if !unknown_dest
                && maybe_option(&arg)
                && !(self.commands.is_empty() && !digit && negative_number(&arg))
            {
                unknown_dest = true;
            }
            if (self.positional || self.pass_through) && operands.is_empty() && unknown.is_empty() {
                if self.commands.iter().any(|c| c.command_name == arg) {
                    operands.push(arg);
                    unknown.extend_from_slice(&args[i..]);
                    break;
                }
                if self.has_help_command()
                    && self.help_command_signature.split_whitespace().next() == Some(arg.as_str())
                {
                    operands.push(arg);
                    operands.extend_from_slice(&args[i..]);
                    break;
                }
            }
            let dest = if unknown_dest {
                &mut unknown
            } else {
                &mut operands
            };
            dest.push(arg);
            if self.pass_through {
                dest.extend_from_slice(&args[i..]);
                break;
            }
        }
        Ok((operands, unknown))
    }
    fn parse_command(
        &mut self,
        mut operands: Vec<String>,
        unknown: Vec<String>,
        ancestors: &[Ancestor],
    ) -> Result<(), ParseError> {
        if let Some(error) = &self.definition_error {
            return Err(ParseError::other(io::Error::other(error.clone())));
        }
        if self.pass_through && ancestors.last().is_some_and(|p| !p.positional) {
            return Err(ParseError::other(io::Error::other(format!(
                "passThroughOptions cannot be used for '{}' without turning on enablePositionalOptions for parent command(s)",
                self.command_name
            ))));
        }
        let (more_operands, unknown) = self.parse_options(&unknown, ancestors)?;
        operands.extend(more_operands);
        self.args = operands.iter().chain(&unknown).cloned().collect();
        if let Some(index) = operands
            .first()
            .and_then(|name| self.commands.iter().position(|c| c.command_name == *name))
        {
            let mut ancestry = ancestors.to_vec();
            ancestry.push(self.ancestor());
            let child = &mut self.commands[index];
            child.prepare();
            return child.parse_command(operands[1..].to_vec(), unknown, &ancestry);
        }
        if self.has_help_command()
            && operands.first().map(String::as_str)
                == self.help_command_signature.split_whitespace().next()
        {
            if let Some(name) = operands.get(1) {
                if let Some(child) = self.commands.iter().find(|c| c.command_name == *name) {
                    return Err(child.help().into());
                }
                return Err(self.help_with_error(true).into());
            }
            return Err(self.help().into());
        }
        if !self.commands.is_empty() && self.args.is_empty() && !self.has_action() {
            return Err(self.help_with_error(true).into());
        }
        if self
            .help_option_value
            .as_ref()
            .is_some_and(|help| unknown.iter().any(|s| help.is(s)))
        {
            self.output_help(false);
            return Err(self
                .policy
                .exit(0, "commander.helpDisplayed", "(outputHelp)")
                .into());
        }
        for (missing, policy) in std::iter::once((self.ancestor().missing, self.policy.clone()))
            .chain(
                ancestors
                    .iter()
                    .rev()
                    .map(|a| (a.missing.clone(), a.policy.clone())),
            )
        {
            if let Some(flags) = missing.first() {
                return Err(policy
                    .error(
                        &format!("error: required option '{flags}' not specified"),
                        1,
                        "commander.missingMandatoryOptionValue",
                    )
                    .into());
            }
        }
        if !self.has_action() && !operands.is_empty() && !self.commands.is_empty() {
            let mut names: Vec<_> = self
                .commands
                .iter()
                .map(|c| c.command_name.clone())
                .collect();
            if self.has_help_command() {
                names.push(
                    self.help_command_signature
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .into(),
                );
            }
            let suggestion = suggest_similar(&self.args[0], &names);
            return Err(self
                .policy
                .error(
                    &format!("error: unknown command '{}'{}", self.args[0], suggestion),
                    1,
                    "commander.unknownCommand",
                )
                .into());
        }
        if !self.allow_unknown
            && let Some(flag) = unknown.first()
        {
            let mut candidates: Vec<_> = self
                .visible_options()
                .iter()
                .filter_map(|o| o.long.clone())
                .collect();
            for ancestor in ancestors.iter().rev() {
                if ancestor.positional {
                    break;
                }
                candidates.extend(ancestor.long_flags.clone());
            }
            let suggestion = if flag.starts_with("--") {
                suggest_similar(flag, &candidates)
            } else {
                String::new()
            };
            return Err(self
                .policy
                .error(
                    &format!("error: unknown option '{flag}'{suggestion}"),
                    1,
                    "commander.unknownOption",
                )
                .into());
        }
        if !self.commands.is_empty() && !self.has_action() && operands.is_empty() {
            return Err(self.help_with_error(true).into());
        }
        self.process_arguments()?;
        if let Some(action) = self.action_handler.clone() {
            action(&self.processed_args.clone(), &self.values.clone(), self)?;
        }
        Ok(())
    }
    fn process_arguments(&mut self) -> Result<(), CommanderError> {
        for (i, arg) in self.registered_arguments.iter().enumerate() {
            if arg.required && self.args.get(i).is_none() {
                return Err(self.policy.error(
                    &format!("error: missing required argument '{}'", arg.name()),
                    1,
                    "commander.missingArgument",
                ));
            }
        }
        if !self.allow_excess
            && !self.registered_arguments.last().is_some_and(|a| a.variadic)
            && self.args.len() > self.registered_arguments.len()
        {
            let n = self.registered_arguments.len();
            let s = if n == 1 { "" } else { "s" };
            let sub = if self.parent_names.is_empty() {
                String::new()
            } else {
                format!(" for '{}'", self.command_name)
            };
            return Err(self.policy.error(
                &format!(
                    "error: too many arguments{sub}. Expected {n} argument{s} but got {}: {}.",
                    self.args.len(),
                    self.args.join(", ")
                ),
                1,
                "commander.excessArguments",
            ));
        }
        let mut processed = Vec::new();
        for (i, arg) in self.registered_arguments.iter().enumerate() {
            let mut value = arg.default_value.clone();
            let raw: Vec<_> = if arg.variadic {
                self.args.get(i..).unwrap_or_default().to_vec()
            } else {
                self.args.get(i).cloned().into_iter().collect()
            };
            if arg.variadic && raw.is_empty() && value.is_none() {
                value = Some(Value::Array(Vec::new()));
            }
            for (index, raw) in raw.into_iter().enumerate() {
                value = Some(arg.parse_value(&raw, value.as_ref(), index == 0).map_err(|e| self.policy.error(&format!("error: command-argument value '{raw}' is invalid for argument '{}'. {}", arg.name(), e.message), e.exit_code, &e.code))?);
            }
            processed.push(value);
        }
        self.processed_args = processed;
        Ok(())
    }
}

fn maybe_option(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-')
}
fn negative_number(arg: &str) -> bool {
    let Some(mut s) = arg.strip_prefix('-') else {
        return false;
    };
    if let Some((mantissa, exponent)) = s.split_once('e') {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !exponent.bytes().all(|c| c.is_ascii_digit()) {
            return false;
        }
        s = mantissa;
    }
    if let Some((before, after)) = s.split_once('.') {
        before.bytes().all(|c| c.is_ascii_digit())
            && !after.is_empty()
            && after.bytes().all(|c| c.is_ascii_digit())
    } else {
        !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // command.parseOptions.test.js, direct split tests and POSIX matrix.
    #[test]
    fn when_parse_options_then_operands_and_unknown_follow_commander_sinks() {
        let cases: &[(&[&str], &[&str], &[&str])] = &[
            (&[], &[], &[]),
            (&["one", "two", "three"], &["one", "two", "three"], &[]),
            (&["sub", "one"], &["sub", "one"], &[]),
            (&["--global-flag"], &[], &[]),
            (&["--global-value", "foo"], &[], &[]),
            (&["--global-flag", "arg"], &["arg"], &[]),
            (&["arg", "--global-flag"], &["arg"], &[]),
            (&["sub", "--global-flag"], &["sub"], &[]),
            (&["--unknown"], &[], &["--unknown"]),
            (&["--unknown", "arg"], &[], &["--unknown", "arg"]),
            (&["arg", "--unknown"], &["arg"], &["--unknown"]),
            (&["--unknown", "--global-flag"], &[], &["--unknown"]),
            (&["sub", "--sub-flag"], &["sub"], &["--sub-flag"]),
            (&["--", "--global-flag"], &["--global-flag"], &[]),
            (&["--", "--unknown", "uuu"], &["--unknown", "uuu"], &[]),
            (
                &["--unknown1", "--", "--unknown2"],
                &[],
                &["--unknown1", "--", "--unknown2"],
            ),
        ];
        for (args, operands, unknown) in cases {
            let mut p = Command::new();
            p.option("--global-flag", "")
                .option("--global-value <value>", "")
                .command("sub [args...]")
                .option("--sub-flag", "");
            let args: Vec<_> = args.iter().map(|s| s.to_string()).collect();
            let (actual_operands, actual_unknown) = p.parse_options(&args, &[]).unwrap();
            assert_eq!(&actual_operands, operands);
            assert_eq!(&actual_unknown, unknown);
        }
    }
    #[test]
    fn when_short_groups_and_long_equals_then_known_removed_unknown_preserved() {
        for (arg, expected, unknown) in [
            ("-ab", json!({"aaa":true,"bbb":true}), false),
            ("-pq", json!({}), true),
            ("-cvalue", json!({"ccc":"value"}), false),
            ("-dvalue", json!({"ddd":"value"}), false),
            (
                "-abcvalue",
                json!({"aaa":true,"bbb":true,"ccc":"value"}),
                false,
            ),
            (
                "-abdvalue",
                json!({"aaa":true,"bbb":true,"ddd":"value"}),
                false,
            ),
            ("--ccc=value", json!({"ccc":"value"}), false),
            ("--rrr=value", json!({}), true),
        ] {
            let mut p = Command::new();
            p.option("-a,--aaa", "")
                .option("-b,--bbb", "")
                .option("-c,--ccc <value>", "")
                .option("-d,--ddd [value]", "");
            let (operands, actual_unknown) = p.parse_options(&[arg.into()], &[]).unwrap();
            assert!(operands.is_empty());
            assert_eq!(actual_unknown, if unknown { vec![arg] } else { vec![] });
            assert_eq!(
                p.opts()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<serde_json::Map<_, _>>(),
                *expected.as_object().unwrap()
            );
        }
        for combine in [true, false] {
            let mut p = Command::new();
            p.option("-b,--bbb", "")
                .option("-d,--ddd [value]", "")
                .combine_flag_and_optional_value(combine);
            p.parse_options(&["-db".into()], &[]).unwrap();
            assert_eq!(
                p.opts()["ddd"],
                if combine { json!("b") } else { json!(true) }
            );
            assert_eq!(p.opts().contains_key("bbb"), !combine);
        }
    }
}
