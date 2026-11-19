use hoh_error::HarnessError;

pub const SKILL_ERROR_CODES: [&str; 4] = [
    "SKILL_NOT_FOUND",
    "SKILL_NOT_MODEL_INVOCABLE",
    "SKILL_INVALID_DEFINITION",
    "SKILL_READ_FAILED",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillErrorCode {
    SkillNotFound,
    SkillNotModelInvocable,
    SkillInvalidDefinition,
    SkillReadFailed,
}
impl SkillErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SkillNotFound => SKILL_ERROR_CODES[0],
            Self::SkillNotModelInvocable => SKILL_ERROR_CODES[1],
            Self::SkillInvalidDefinition => SKILL_ERROR_CODES[2],
            Self::SkillReadFailed => SKILL_ERROR_CODES[3],
        }
    }
}
pub struct SkillError;
impl SkillError {
    pub const NAME: &'static str = "SkillError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: SkillErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
