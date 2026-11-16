use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommanderError {
    pub exit_code: i32,
    pub code: String,
    pub message: String,
}

impl CommanderError {
    pub fn new(exit_code: i32, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for CommanderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for CommanderError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidArgumentError {
    pub exit_code: i32,
    pub code: String,
    pub message: String,
}
impl InvalidArgumentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            exit_code: 1,
            code: "commander.invalidArgument".into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for InvalidArgumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for InvalidArgumentError {}

/// Action failures retain their original error and are distinguishable from exits.
#[derive(Debug)]
pub enum ParseError {
    Commander(CommanderError),
    Other(Box<dyn Error>),
}
impl ParseError {
    pub fn commander(&self) -> Option<&CommanderError> {
        match self {
            Self::Commander(error) => Some(error),
            Self::Other(_) => None,
        }
    }
    pub fn other(error: impl Error + 'static) -> Self {
        Self::Other(Box::new(error))
    }
}
impl From<CommanderError> for ParseError {
    fn from(error: CommanderError) -> Self {
        Self::Commander(error)
    }
}
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Commander(e) => e.fmt(f),
            Self::Other(e) => e.fmt(f),
        }
    }
}
impl Error for ParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(match self {
            Self::Commander(e) => e,
            Self::Other(e) => e.as_ref(),
        })
    }
}
