//! The synchronous, in-process part of commander 15.0.0 used by DeepSeek Harness.
//!
//! `parse` accepts user arguments (without an executable or script name).
//! All exits are returned as errors; without `exit_override`, the explicitly
//! configured exit callback is also called. The library never exits the process.
mod argument;
mod command;
mod error;
mod help;
mod option;
mod suggest_similar;

pub use argument::Argument;
pub use command::{Command, Options, Output, ParseOptions};
pub use error::{CommanderError, InvalidArgumentError, ParseError};
pub use option::Opt;

use serde_json::Value;
use std::rc::Rc;

/// A missing previous value is JavaScript's `undefined`, distinct from JSON null.
pub type ValueParser = Rc<dyn Fn(&str, Option<&Value>) -> Result<Value, InvalidArgumentError>>;

pub(crate) fn collect(value: Value, previous: Option<&Value>, is_default: bool) -> Value {
    let mut values = if !is_default {
        previous
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    values.push(value);
    Value::Array(values)
}
