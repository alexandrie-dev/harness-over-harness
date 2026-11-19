use hoh_error::BoxError;
use hoh_subprocess::{SubprocessHandle, SubprocessRuntime, SubprocessSpawnSpec};

pub(crate) fn spawn_ripgrep_at(
    subprocess: &dyn SubprocessRuntime,
    mut spec: SubprocessSpawnSpec,
    executable: String,
) -> Result<SubprocessHandle, BoxError> {
    spec.argv
        .splice(0..0, [executable, "--no-config".to_owned()]);
    subprocess.spawn(spec)
}
