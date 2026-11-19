use hoh_error::BoxError;
use hoh_subprocess::{SubprocessEnvironment, control::SUBPROCESS_CONTROL_ENV};

pub fn control_environment(
    mut env: SubprocessEnvironment,
    control: bool,
) -> Result<SubprocessEnvironment, BoxError> {
    if env
        .iter()
        .any(|(key, value)| key.to_uppercase() == SUBPROCESS_CONTROL_ENV && value.is_some())
    {
        return Err(crate::error(format!(
            "{SUBPROCESS_CONTROL_ENV} is reserved for subprocess control-channel setup"
        )));
    }
    if control {
        env.insert(SUBPROCESS_CONTROL_ENV.into(), Some("pipe".into()));
    }
    Ok(env)
}
