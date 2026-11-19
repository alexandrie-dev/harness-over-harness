//! Human-question capability and durable timed-question projection.
//!
//! Port of `packages/interaction/user-questions`. The UI/provider remains a
//! waterfall contribution; this crate owns validation, foreground waits,
//! continued replies, and reconstruction from the immutable Session log.

pub mod projection;
mod service;
pub mod timed_wait;
pub mod types;

pub use projection::{
    ASK_USER_QUESTION_TOOL, TIMED_WAIT_PARAMETER, UserQuestionFold, UserQuestionProjectionState,
    apply_user_question_event, fold_user_questions, is_timed_ask_user_question_schema,
    questions_of, user_question_projection_definition,
};
pub use service::{
    AskUserQuestionRequest, UserQuestionError, UserQuestionService, UserQuestions,
    UserQuestionsPlugin,
};
pub use timed_wait::{TimedQuestionClaim, TimedQuestionWait, WaitFrame};
pub use types::*;
