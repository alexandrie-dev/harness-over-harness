use crate::{InvalidArgumentError, ValueParser, collect};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct Argument {
    name: String,
    pub description: String,
    pub required: bool,
    pub variadic: bool,
    pub default_value: Option<Value>,
    pub default_value_description: Option<String>,
    pub arg_choices: Option<Vec<String>>,
    pub(crate) parser: Option<ValueParser>,
}
impl Argument {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name = name.into();
        let required = !name.starts_with('[');
        let name = if name.starts_with(['[', '<']) {
            name.get(1..name.len().saturating_sub(1))
                .unwrap_or_default()
        } else {
            &name
        };
        let variadic = name.ends_with("...");
        Self {
            name: name.strip_suffix("...").unwrap_or(name).into(),
            description: description.into(),
            required,
            variadic,
            default_value: None,
            default_value_description: None,
            arg_choices: None,
            parser: None,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.default_value = Some(value.into());
        self
    }
    pub fn default_description(mut self, description: impl Into<String>) -> Self {
        self.default_value_description = Some(description.into());
        self
    }
    pub fn arg_parser<F>(mut self, parser: F) -> Self
    where
        F: Fn(&str, Option<&Value>) -> Result<Value, InvalidArgumentError> + Send + Sync + 'static,
    {
        self.parser = Some(Arc::new(parser));
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
    pub fn arg_required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn arg_optional(mut self) -> Self {
        self.required = false;
        self
    }
    pub(crate) fn human_readable(&self) -> String {
        let name = format!("{}{}", self.name, if self.variadic { "..." } else { "" });
        if self.required {
            format!("<{name}>")
        } else {
            format!("[{name}]")
        }
    }
    pub(crate) fn parse_value(
        &self,
        raw: &str,
        previous: Option<&Value>,
        is_default: bool,
    ) -> Result<Value, InvalidArgumentError> {
        if let Some(parser) = &self.parser {
            return parser(raw, previous);
        }
        if let Some(choices) = &self.arg_choices
            && !choices.iter().any(|v| v == raw)
        {
            return Err(InvalidArgumentError::new(format!(
                "Allowed choices are {}.",
                choices.join(", ")
            )));
        }
        Ok(if self.variadic {
            collect(Value::String(raw.into()), previous, is_default)
        } else {
            Value::String(raw.into())
        })
    }
}
