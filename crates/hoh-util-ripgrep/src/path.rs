use hoh_error::BoxError;

#[derive(Debug, thiserror::Error)]
#[error("packaged ripgrep executable was not found at {0}")]
struct MissingExecutable(String);

pub(crate) struct Resolution<'a> {
    pub(crate) executable: &'a str,
    pub(crate) windows: bool,
    pub(crate) packaged: bool,
    pub(crate) electron: bool,
    pub(crate) dependency: Option<&'a str>,
}

pub(crate) fn sidecar_path(executable: &str, windows: bool) -> String {
    if !windows {
        return format!("{executable}-rg");
    }
    let split = executable
        .rfind(['/', '\\'])
        .map_or(0, |separator| separator + 1);
    let (directory, basename) = executable.split_at(split);
    let extension = basename
        .rfind('.')
        .filter(|extension| basename[..*extension].bytes().any(|byte| byte != b'.'));
    let name = extension.map_or(basename, |extension| &basename[..extension]);
    format!("{directory}{name}-rg.exe")
}

pub(crate) fn unpack_asar(path: &str) -> String {
    let slash = path.find(".asar/");
    let backslash = path.find(".asar\\");
    let position = match (slash, backslash) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    position.map_or_else(
        || path.to_owned(),
        |position| {
            let mut unpacked = path.to_owned();
            unpacked.insert_str(position + ".asar".len(), ".unpacked");
            unpacked
        },
    )
}

pub(crate) fn resolve_from(
    context: Resolution<'_>,
    exists: impl FnOnce(&str) -> bool,
) -> Result<String, BoxError> {
    let sidecar = sidecar_path(context.executable, context.windows);
    if context.packaged && exists(&sidecar) {
        return Ok(sidecar);
    }
    let dependency = context
        .dependency
        .ok_or_else(|| Box::new(MissingExecutable(sidecar)) as BoxError)?;
    Ok(if context.electron {
        unpack_asar(dependency)
    } else {
        dependency.to_owned()
    })
}
