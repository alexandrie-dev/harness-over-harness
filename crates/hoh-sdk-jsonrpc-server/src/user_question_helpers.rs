//! Only UserQuestionError from packages/interaction/user-questions/src/index.ts:46–51.
//! The service itself is not ported into the SDK server.
use hoh_error::HarnessError;

pub struct UserQuestionError;
impl UserQuestionError {
    pub const NAME: &'static str = "UserQuestionError";
    /// Source constructor accepts an extensible string code, not a closed union.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}
