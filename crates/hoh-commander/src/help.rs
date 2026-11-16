use crate::{Argument, Command, Opt};
use serde_json::Value;

impl Command {
    pub(crate) fn visible_options(&self) -> Vec<Opt> {
        let mut options: Vec<_> = self.options.iter().filter(|o| !o.hidden).cloned().collect();
        if let Some(help) = &self.help_option_value
            && !help.hidden
        {
            let short_clash = help
                .short
                .as_ref()
                .is_some_and(|f| self.options.iter().any(|o| o.is(f)));
            let long_clash = help
                .long
                .as_ref()
                .is_some_and(|f| self.options.iter().any(|o| o.is(f)));
            if !short_clash && !long_clash {
                options.push(help.clone());
            } else if let Some(long) = help.long.as_ref().filter(|_| !long_clash) {
                options.push(Opt::new(long, &help.description));
            } else if let Some(short) = help.short.as_ref().filter(|_| !short_clash) {
                options.push(Opt::new(short, &help.description));
            }
        }
        options
    }

    /// Built-in help, excluding the custom text added with add_help_text.
    pub fn help_information(&self) -> String {
        let options = self.visible_options();
        let arguments = if self
            .registered_arguments
            .iter()
            .any(|a| !a.description.is_empty())
        {
            self.registered_arguments.as_slice()
        } else {
            &[]
        };
        let mut commands: Vec<_> = self
            .commands
            .iter()
            .map(|c| (subcommand_term(c), c.command_description.clone()))
            .collect();
        if self.has_help_command() {
            commands.push((
                self.help_command_signature.clone(),
                self.help_command_description.clone(),
            ));
        }
        let width = pad_width(&options, arguments, &commands);
        let mut path = self.parent_names.join(" ");
        if !path.is_empty() {
            path.push(' ');
        }
        let mut output = vec![
            format!("Usage: {path}{} {}", self.command_name, self.get_usage()),
            String::new(),
        ];
        if !self.command_description.is_empty() {
            output.push(box_wrap(&self.command_description, 80));
            output.push(String::new());
        }
        if !arguments.is_empty() {
            output.push("Arguments:".into());
            for arg in arguments {
                output.push(format_item(arg.name(), width, &argument_description(arg)));
            }
            output.push(String::new());
        }
        if !options.is_empty() {
            output.push("Options:".into());
            for opt in options {
                output.push(format_item(&opt.flags, width, &option_description(&opt)));
            }
            output.push(String::new());
        }
        if !commands.is_empty() {
            output.push("Commands:".into());
            for (term, description) in commands {
                output.push(format_item(&term, width, &description));
            }
            output.push(String::new());
        }
        strip_ansi(&output.join("\n"))
    }

    /// Writes each part separately, in commander's help-event order.
    pub fn output_help(&self, error: bool) {
        for text in &self.inherited_before {
            self.write_help_text(text, error);
        }
        self.write_help_position("beforeAll", error);
        self.write_help_position("before", error);
        self.write_help(&self.help_information(), error);
        self.write_help_position("after", error);
        self.write_help_position("afterAll", error);
        for text in &self.inherited_after {
            self.write_help_text(text, error);
        }
    }
    fn write_help_position(&self, position: &str, error: bool) {
        for (_, text) in self.help_texts.iter().filter(|(p, _)| p == position) {
            self.write_help_text(text, error);
        }
    }
    fn write_help_text(&self, text: &str, error: bool) {
        if !text.is_empty() {
            self.write_help(&format!("{text}\n"), error);
        }
    }
}

fn subcommand_term(cmd: &Command) -> String {
    let mut term = cmd.command_name.clone();
    if !cmd.options.is_empty() {
        term.push_str(" [options]");
    }
    for arg in &cmd.registered_arguments {
        term.push(' ');
        term.push_str(&arg.human_readable());
    }
    term
}

fn longest_option_term_length(options: &[Opt]) -> usize {
    options
        .iter()
        .map(|o| display_width(&o.flags))
        .max()
        .unwrap_or_default()
}
fn longest_argument_term_length(arguments: &[Argument]) -> usize {
    arguments
        .iter()
        .map(|a| display_width(a.name()))
        .max()
        .unwrap_or_default()
}
fn longest_subcommand_term_length(commands: &[(String, String)]) -> usize {
    commands
        .iter()
        .map(|(term, _)| display_width(term))
        .max()
        .unwrap_or_default()
}
fn pad_width(options: &[Opt], arguments: &[Argument], commands: &[(String, String)]) -> usize {
    longest_option_term_length(options)
        .max(longest_argument_term_length(arguments))
        .max(longest_subcommand_term_length(commands))
}

fn annotate(description: &str, extra: Vec<String>) -> String {
    if extra.is_empty() {
        return description.into();
    }
    let extra = format!("({})", extra.join(", "));
    if description.is_empty() {
        extra
    } else {
        format!("{description} {extra}")
    }
}
fn option_description(option: &Opt) -> String {
    let mut extra = Vec::new();
    if let Some(choices) = &option.arg_choices {
        extra.push(format!(
            "choices: {}",
            choices
                .iter()
                .map(|v| json_text(&Value::String(v.clone())))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(default) = &option.default_value
        && (option.required || option.optional || (option.is_boolean() && default.is_boolean()))
    {
        extra.push(format!(
            "default: {}",
            option
                .default_value_description
                .as_ref()
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| json_text(default))
        ));
    }
    if option.optional
        && let Some(preset) = &option.preset_arg
    {
        extra.push(format!("preset: {}", json_text(preset)));
    }
    annotate(&option.description, extra)
}
fn argument_description(argument: &Argument) -> String {
    let mut extra = Vec::new();
    if let Some(choices) = &argument.arg_choices {
        extra.push(format!(
            "choices: {}",
            choices
                .iter()
                .map(|v| json_text(&Value::String(v.clone())))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(default) = &argument.default_value {
        extra.push(format!(
            "default: {}",
            argument
                .default_value_description
                .as_ref()
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| json_text(default))
        ));
    }
    annotate(&argument.description, extra)
}

pub(crate) fn display_width(text: &str) -> usize {
    strip_ansi(text).encode_utf16().count()
}
fn preformatted(text: &str) -> bool {
    text.split('\n').skip(1).any(|s| {
        s.chars()
            .next()
            .is_some_and(|c| c.is_whitespace() && c != '\r' && c != '\n')
    })
}
fn format_item(term: &str, term_width: usize, description: &str) -> String {
    if description.is_empty() {
        return format!("  {term}");
    }
    let padding = " ".repeat(term_width.saturating_sub(display_width(term)));
    let remaining = 80_usize.saturating_sub(term_width + 4);
    let description = if remaining < 40 || preformatted(description) {
        description.to_string()
    } else {
        box_wrap(description, remaining).replace('\n', &format!("\n{}", " ".repeat(term_width + 2)))
    };
    format!("  {term}{padding}  {}", description.replace('\n', "\n  "))
}
fn box_wrap(text: &str, width: usize) -> String {
    if width < 40 {
        return text.into();
    }
    let mut lines = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut in_word = false;
        for (i, c) in line.char_indices() {
            if c.is_whitespace() {
                if in_word {
                    chunks.push(&line[start..i]);
                    start = i;
                    in_word = false;
                }
            } else {
                in_word = true;
            }
        }
        if in_word {
            chunks.push(&line[start..]);
        }
        if chunks.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = chunks[0].to_string();
        let mut used = display_width(&current);
        for chunk in &chunks[1..] {
            let visible = display_width(chunk);
            if used + visible <= width {
                current.push_str(chunk);
                used += visible;
            } else {
                lines.push(current);
                current = chunk.trim_start().into();
                used = display_width(&current);
            }
        }
        lines.push(current);
    }
    lines.join("\n")
}

// No colour is emitted. Existing terminal escape sequences in help are stripped.
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if c == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            result.push(c);
        }
    }
    result
}

pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => number_text(v),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js_string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}
fn number_text(value: &serde_json::Number) -> String {
    if value.as_f64() == Some(0.0) {
        return "0".into();
    }
    let raw = value.to_string();
    if let Some(raw) = raw.strip_suffix(".0") {
        return raw.into();
    }
    let Some((mantissa, exponent)) = raw.split_once(['e', 'E']) else {
        return raw;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return raw;
    };
    let (sign, mantissa) = if let Some(m) = mantissa.strip_prefix('-') {
        ("-", m)
    } else {
        ("", mantissa)
    };
    let digits = mantissa.replace('.', "");
    let digits = digits.trim_end_matches('0');
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    if point > 0 && point <= 21 {
        let point = point as usize;
        if point >= digits.len() {
            format!("{sign}{digits}{}", "0".repeat(point - digits.len()))
        } else {
            format!("{sign}{}.{}", &digits[..point], &digits[point..])
        }
    } else if point <= 0 && point > -6 {
        format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let tail = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        let e = point - 1;
        format!(
            "{sign}{}{tail}e{}{e}",
            &digits[..1],
            if e >= 0 { "+" } else { "" }
        )
    }
}

// JSON.stringify for defaults/choices without a dependency outside the task manifest.
pub(crate) fn json_text(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => js_string(value),
        Value::String(s) => {
            let mut result = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => result.push_str("\\\""),
                    '\\' => result.push_str("\\\\"),
                    '\n' => result.push_str("\\n"),
                    '\r' => result.push_str("\\r"),
                    '\t' => result.push_str("\\t"),
                    '\u{8}' => result.push_str("\\b"),
                    '\u{c}' => result.push_str("\\f"),
                    c if c < ' ' => result.push_str(&format!("\\u{:04x}", u32::from(c))),
                    c => result.push(c),
                }
            }
            result.push('"');
            result
        }
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(json_text).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by_key(|(key, _)| {
                key.parse::<u32>()
                    .ok()
                    .filter(|v| *v != u32::MAX && v.to_string() == **key)
                    .map_or((1, 0), |v| (0, v))
            });
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        json_text(&Value::String(key.clone())),
                        json_text(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // help.boxWrap.test.js: 14 cases, excluding configurable minWidthToWrap.
    #[test]
    fn when_box_wrap_then_preserve_whitespace_and_soft_wrap() {
        for (text, width, expected) in [
            ("", 50, ""),
            ("\n\n\n", 50, "\n\n\n"),
            ("\r\n\r\n\r\n", 50, "\n\n\n"),
            (" \t  ", 50, ""),
            ("  abc", 50, "  abc"),
            ("123   456", 50, "123   456"),
            ("123   456   ", 50, "123   456"),
            (
                "123 567 901 345 789 123 567 901 345 789 123 567 901",
                45,
                "123 567 901 345 789 123 567 901 345 789 123\n567 901",
            ),
        ] {
            assert_eq!(box_wrap(text, width), expected);
        }
        let long = "abc".repeat(20);
        assert_eq!(box_wrap(&long, 40), long);
        assert_eq!(box_wrap(&format!("xyz {long}"), 40), format!("xyz\n{long}"));
        let text = format!("{} 1 2 3", "X".repeat(39));
        assert_eq!(box_wrap(&text, 40), format!("{}\n1 2 3", "X".repeat(39)));
        assert_eq!(box_wrap(&text, 41), format!("{} 1\n2 3", "X".repeat(39)));
        let text = " x".repeat(8) + &" yy".repeat(6);
        assert_eq!(box_wrap(&text, 14), text);
        let underlined = "\u{1b}[4mXYZ\u{1b}[24m";
        let text = underlined.to_string() + &" ABC".repeat(19);
        assert_eq!(
            box_wrap(&text, 44),
            format!(
                "{underlined} {}ABC\n{}ABC",
                "ABC ".repeat(9),
                "ABC ".repeat(8)
            )
        );
    }
    // help.preformatted.test.js: all nine cases.
    #[test]
    fn when_preformatted_then_line_break_followed_by_whitespace() {
        for text in [
            "a b c",
            " a b c",
            "a\nb\nc",
            "a\r\nb\r\nc",
            "a\n\n",
            "a\r\n\r\n",
        ] {
            assert!(!preformatted(text));
        }
        for text in ["a\n  b", "a\r\n  b"] {
            assert!(preformatted(text));
        }
        let mut p = Command::new();
        p.option("-t, --time <HH:MM>", "select time\n\nTime can also be specified using special values:\n  \"dawn\" - From night to sunrise.\n");
        assert_eq!(
            p.help_information(),
            "Usage:  [options]\n\nOptions:\n  -t, --time <HH:MM>  select time\n  \n  Time can also be specified using special values:\n    \"dawn\" - From night to sunrise.\n  \n  -h, --help          display help for command\n"
        );
    }
    // help.optionDescription.test.js: nine cases, excluding env.
    #[test]
    fn when_option_description_then_defaults_choices_and_preset_annotated() {
        for (opt, expected) in [
            (Opt::new("-a", ""), ""),
            (Opt::new("-a", "description"), "description"),
            (
                Opt::new("-a", "description").default(true),
                "description (default: true)",
            ),
            (Opt::new("-a", "description").default("foo"), "description"),
            (
                Opt::new("--aa [value]", "description").preset("abc"),
                "description (preset: \"abc\")",
            ),
            (Opt::new("--bb", "description").preset("abc"), "description"),
            (
                Opt::new("-a <value>", "description")
                    .default("default value")
                    .default_description("custom"),
                "description (default: custom)",
            ),
            (
                Opt::new("-a <value>", "")
                    .default("default value")
                    .default_description("custom"),
                "(default: custom)",
            ),
            (
                Opt::new("-a <value>", "description").choices(["one", "two"]),
                "description (choices: \"one\", \"two\")",
            ),
        ] {
            assert_eq!(option_description(&opt), expected);
        }
    }
    // help.argumentDescription.test.js: all five cases.
    #[test]
    fn when_argument_description_then_default_annotated() {
        for (arg, expected) in [
            (Argument::new("[n]", ""), ""),
            (Argument::new("[n]", "description"), "description"),
            (
                Argument::new("[n]", "description").default("default"),
                "description (default: \"default\")",
            ),
            (
                Argument::new("[n]", "description")
                    .default("default value")
                    .default_description("custom"),
                "description (default: custom)",
            ),
            (
                Argument::new("[n]", "").default("default"),
                "(default: \"default\")",
            ),
        ] {
            assert_eq!(argument_description(&arg), expected);
        }
    }
    #[test]
    fn when_unicode_help_then_width_counts_utf16_units() {
        assert_eq!(display_width("привет"), 6);
        assert_eq!(display_width("😀"), 2);
        assert_eq!(display_width("\u{1b}[31mred\u{1b}[0m"), 3);
    }
    #[test]
    fn when_json_defaults_then_javascript_representation() {
        for (value, expected) in [
            (json!(-0.0), "0"),
            (json!(1.0), "1"),
            (json!(1e21), "1e+21"),
            (json!(1e20), "100000000000000000000"),
            (json!(1e-6), "0.000001"),
            (json!(1e-7), "1e-7"),
            (json!("a\n\t\"\\"), "\"a\\n\\t\\\"\\\\\""),
        ] {
            assert_eq!(json_text(&value), expected);
        }
        assert_eq!(
            json_text(&json!({"b":null,"2":true,"1":false})),
            "{\"1\":false,\"2\":true,\"b\":null}"
        );
    }

    // help.longest*TermLength.test.js, help.padWidth.test.js without globals.
    #[test]
    fn when_longest_terms_then_pad_width_uses_maximum_visible_term() {
        assert_eq!(longest_option_term_length(&[]), 0);
        let p = Command::new();
        assert_eq!(
            longest_option_term_length(&p.visible_options()),
            "-h, --help".len()
        );
        let mut p = Command::new();
        p.option("--longest <value>", "").option("-s", "");
        assert_eq!(
            longest_option_term_length(&p.visible_options()),
            "--longest <value>".len()
        );
        assert_eq!(longest_argument_term_length(&[]), 0);
        assert_eq!(
            longest_argument_term_length(&[Argument::new("<wonder>", "wonder description")]),
            "wonder".len()
        );
        assert_eq!(
            longest_argument_term_length(&[
                Argument::new("<alpha>", "x"),
                Argument::new("<longest>", "x"),
                Argument::new("<beta>", "x")
            ]),
            "longest".len()
        );
        assert_eq!(longest_subcommand_term_length(&[]), 0);
        for terms in [
            vec!["program"],
            vec!["program <file>"],
            vec!["short", "longest-command"],
            vec!["help [command]"],
        ] {
            let expected = terms.iter().map(|s| s.len()).max().unwrap();
            let terms = terms
                .into_iter()
                .map(|s| (s.into(), String::new()))
                .collect::<Vec<_>>();
            assert_eq!(longest_subcommand_term_length(&terms), expected);
        }
        for (options, arguments, commands, expected) in [
            (
                vec![Opt::new("-o", "")],
                vec![Argument::new(
                    "<veryLongThingBiggerThanOthers>",
                    "description",
                )],
                vec![("sub".into(), String::new())],
                "veryLongThingBiggerThanOthers".len(),
            ),
            (
                vec![Opt::new("--very-long-thing-bigger-than-others", "")],
                vec![Argument::new("<file>", "desc")],
                vec![("sub".into(), String::new())],
                "--very-long-thing-bigger-than-others".len(),
            ),
            (
                vec![Opt::new("-o", "")],
                vec![Argument::new("<file>", "desc")],
                vec![("very-long-thing-bigger-than-others".into(), String::new())],
                "very-long-thing-bigger-than-others".len(),
            ),
        ] {
            assert_eq!(pad_width(&options, &arguments, &commands), expected);
        }
    }
    // help.visibleOptions.test.js and help.optionTerm.test.js.
    #[test]
    fn when_visible_options_then_hidden_and_masked_help_flags_filtered() {
        let mut p = Command::new();
        assert_eq!(
            p.visible_options()
                .iter()
                .map(|o| o.flags.as_str())
                .collect::<Vec<_>>(),
            ["-h, --help"]
        );
        p.help_option_enabled(false);
        assert!(p.visible_options().is_empty());
        p.help_option_enabled(true);
        p.option("--example", "");
        assert_eq!(p.visible_options().len(), 2);
        p.add_option(Opt::new("--secret", "").hide_help(true));
        assert_eq!(p.visible_options().len(), 2);
        for (flags, expected) in [
            ("-h, --host", Some("--help")),
            ("--help", Some("-h")),
            ("-h, --help", None),
        ] {
            let mut p = Command::new();
            p.option(flags, "custom");
            let options = p.visible_options();
            assert_eq!(
                options
                    .iter()
                    .filter(|o| o.description == "display help for command")
                    .map(|o| o.flags.as_str())
                    .next(),
                expected
            );
        }
        for flags in ["-s", "--short", "-s,--short", "-s|--short"] {
            let opt = Opt::new(flags, "");
            assert_eq!(opt.flags, flags);
        }
    }
    #[test]
    fn when_subcommand_term_then_arguments_and_options_included() {
        let mut p = Command::new();
        p.name("program");
        assert_eq!(subcommand_term(&p), "program");
        p.option("-a,--all", "");
        assert_eq!(subcommand_term(&p), "program [options]");
        let mut p = Command::new();
        p.name("program").argument("<argument>", "");
        assert_eq!(subcommand_term(&p), "program <argument>");
    }
}
