use crate::{InvalidArgumentError, ValueParser};
use serde_json::Value;
use std::rc::Rc;

#[derive(Clone)]
pub struct Opt {
    pub flags: String,
    pub description: String,
    pub short: Option<String>,
    pub long: Option<String>,
    pub required: bool,
    pub optional: bool,
    pub variadic: bool,
    pub negate: bool,
    pub mandatory: bool,
    pub hidden: bool,
    pub default_value: Option<Value>,
    pub default_value_description: Option<String>,
    pub preset_arg: Option<Value>,
    pub arg_choices: Option<Vec<String>>,
    pub(crate) parser: Option<ValueParser>,
    pub(crate) version: Option<String>,
    pub(crate) definition_error: Option<String>,
}

impl Opt {
    pub fn new(flags: impl Into<String>, description: impl Into<String>) -> Self {
        let flags = flags.into();
        let parts: Vec<_> = flags
            .split([' ', '|', ','])
            .filter(|s| !s.is_empty())
            .collect();
        let is_short =
            |s: &str| s.starts_with('-') && !s.starts_with("--") && s.encode_utf16().count() == 2;
        let is_long = |s: &str| s.starts_with("--") && s.len() > 2 && !s.starts_with("---");
        let mut i = 0;
        let mut short = None;
        let mut long = None;
        if parts.get(i).is_some_and(|s| is_short(s)) {
            short = Some(parts[i].to_string());
            i += 1;
        }
        if parts.get(i).is_some_and(|s| is_long(s)) {
            long = Some(parts[i].to_string());
            i += 1;
        }
        if short.is_none() && parts.get(i).is_some_and(|s| is_short(s)) {
            short = Some(parts[i].to_string());
            i += 1;
        }
        if short.is_none() && parts.get(i).is_some_and(|s| is_long(s)) {
            short = long.take();
            long = Some(parts[i].to_string());
            i += 1;
        }
        let definition_error = if let Some(flag) = parts.get(i).filter(|s| s.starts_with('-')) {
            let reason = if is_short(flag) {
                "too many short flags"
            } else if is_long(flag) {
                "too many long flags"
            } else if flag.starts_with('-')
                && !flag.starts_with("--")
                && flag.chars().nth(2).is_some_and(|c| c != '-')
            {
                "a short flag is a single dash and a single character\n  - either use a single dash and a single character (for a short flag)\n  - or use a double dash for a long option (and can have two, like '--ws, --workspace')"
            } else {
                "unrecognised flag format"
            };
            Some(format!(
                "option creation failed due to '{flag}' in option flags '{flags}'\n- {reason}"
            ))
        } else if short.is_none() && long.is_none() {
            Some(format!(
                "option creation failed due to no flags found in '{flags}'."
            ))
        } else {
            None
        };
        let variadic = (flags.ends_with("...>") || flags.ends_with("...]"))
            && flags
                .chars()
                .rev()
                .nth(4)
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        Self {
            required: flags.contains('<'),
            optional: flags.contains('['),
            variadic,
            negate: long.as_ref().is_some_and(|s| s.starts_with("--no-")),
            flags,
            description: description.into(),
            short,
            long,
            mandatory: false,
            hidden: false,
            default_value: None,
            default_value_description: None,
            preset_arg: None,
            arg_choices: None,
            parser: None,
            version: None,
            definition_error,
        }
    }
    pub fn name(&self) -> String {
        self.long
            .as_deref()
            .map(|s| s.trim_start_matches("--"))
            .or_else(|| self.short.as_deref().map(|s| s.trim_start_matches('-')))
            .unwrap_or_default()
            .into()
    }
    pub fn attribute_name(&self) -> String {
        let name = self.name();
        let name = if self.negate {
            name.strip_prefix("no-").unwrap_or(&name)
        } else {
            &name
        };
        let mut words = name.split('-');
        let mut result = words.next().unwrap_or_default().to_string();
        for word in words {
            let mut chars = word.chars();
            if let Some(c) = chars.next() {
                result.extend(c.to_uppercase());
                result.push_str(chars.as_str());
            }
        }
        result
    }
    pub fn is(&self, flag: &str) -> bool {
        self.short.as_deref() == Some(flag) || self.long.as_deref() == Some(flag)
    }
    pub fn is_boolean(&self) -> bool {
        !self.required && !self.optional && !self.negate
    }
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.default_value = Some(value.into());
        self
    }
    pub fn default_description(mut self, description: impl Into<String>) -> Self {
        self.default_value_description = Some(description.into());
        self
    }
    pub fn preset(mut self, value: impl Into<Value>) -> Self {
        self.preset_arg = Some(value.into());
        self
    }
    pub fn arg_parser<F>(mut self, parser: F) -> Self
    where
        F: Fn(&str, Option<&Value>) -> Result<Value, InvalidArgumentError> + 'static,
    {
        self.parser = Some(Rc::new(parser));
        self
    }
    pub fn choices<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.arg_choices = Some(values.into_iter().map(Into::into).collect());
        self.parser = None;
        self
    }
    pub fn make_option_mandatory(mut self, mandatory: bool) -> Self {
        self.mandatory = mandatory;
        self
    }
    pub fn hide_help(mut self, hide: bool) -> Self {
        self.hidden = hide;
        self
    }
}
