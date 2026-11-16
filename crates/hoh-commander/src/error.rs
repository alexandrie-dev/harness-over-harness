use std::error::Error;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
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

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
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
/// Action failures retain their original error and are distinguishable from exits.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error(transparent)]
    Commander(CommanderError),
    #[error(transparent)]
    Other(Box<dyn Error + Send + Sync>),
}
impl ParseError {
    pub fn commander(&self) -> Option<&CommanderError> {
        match self {
            Self::Commander(error) => Some(error),
            Self::Other(_) => None,
        }
    }
    pub fn other(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Other(Box::new(error))
    }
}
impl From<CommanderError> for ParseError {
    fn from(error: CommanderError) -> Self {
        Self::Commander(error)
    }
}
