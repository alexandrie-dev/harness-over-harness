//! Private Bash/Zsh lifecycle records. Output text never proves shell idle.
use crate::lock;
use hoh_error::BoxError;
use hoh_subprocess::{
    SubprocessTerminalActivity, SubprocessTerminalActivityState, SubprocessTerminalSpawnSpec,
};
use indexmap::IndexMap;
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Mutex,
};
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
#[derive(Default)]
struct State {
    revision: u64,
    observed: String,
    invalidated: Option<String>,
}
pub struct ShellActivity {
    pub directory: PathBuf,
    pub argv: Vec<String>,
    pub env: IndexMap<String, String>,
    state: Mutex<State>,
}
impl ShellActivity {
    pub fn new(directory: PathBuf, argv: Vec<String>, env: IndexMap<String, String>) -> Self {
        Self {
            directory,
            argv,
            env,
            state: Mutex::default(),
        }
    }
    fn read(&self) -> String {
        crate::read_utf8(self.directory.join("state")).unwrap_or_default()
    }
    pub fn invalidate(&self) {
        let record = self.read();
        let mut state = lock(&self.state);
        state.invalidated = Some(record);
        state.revision += 1;
    }
    pub fn inspect(&self, pid: u32) -> SubprocessTerminalActivity {
        let record = self.read();
        let mut state = lock(&self.state);
        if record != state.observed {
            state.observed = record.clone();
            state.revision += 1;
        }
        let fields = record
            .strip_suffix('\n')
            .unwrap_or(&record)
            .split(':')
            .collect::<Vec<_>>();
        let status = if state.invalidated.as_deref() == Some(&record)
            || fields.len() != 3
            || fields[0] != pid.to_string()
            || fields[..2]
                .iter()
                .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
        {
            SubprocessTerminalActivityState::Unknown
        } else {
            match fields[2] {
                "idle" => SubprocessTerminalActivityState::Idle,
                "busy" => SubprocessTerminalActivityState::Busy,
                _ => SubprocessTerminalActivityState::Unknown,
            }
        };
        SubprocessTerminalActivity {
            state: status,
            revision: state.revision,
        }
    }
    pub fn dispose(&self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn write_new(path: &std::path::Path, body: &str) -> Result<(), BoxError> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            crate::linux_execve::system_error(error, "open", Some(&path.to_string_lossy()))
        })?
        .write_all(body.as_bytes())
        .map_err(|error| crate::linux_execve::system_error(error, "write", None))?;
    Ok(())
}
pub fn prepare_shell_activity(
    spec: &SubprocessTerminalSpawnSpec,
    env: IndexMap<String, String>,
    platform: &str,
) -> Result<Option<ShellActivity>, BoxError> {
    prepare_with(
        spec,
        env,
        platform,
        || {
            Ok(crate::shim::normalize_temporary_path(
                &tempfile::Builder::new()
                    .prefix("dsh-shell-")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir_in(crate::shim::temporary_directory())?
                    .keep(),
            ))
        },
        write_new,
    )
}
fn prepare_with(
    spec: &SubprocessTerminalSpawnSpec,
    env: IndexMap<String, String>,
    platform: &str,
    allocate: impl FnOnce() -> Result<PathBuf, BoxError>,
    write: impl Fn(&std::path::Path, &str) -> Result<(), BoxError>,
) -> Result<Option<ShellActivity>, BoxError> {
    if spec.shell_activity != Some(true)
        || platform == "win32"
        || spec.argv.len() != 2
        || spec.argv[1] != "-i"
    {
        return Ok(None);
    }
    let shell = hoh_util_workspace_path::node_path::basename(&spec.argv[0]);
    if shell != "bash" && shell != "zsh" {
        return Ok(None);
    }
    let directory = allocate()?;
    let state = quote(&directory.join("state").to_string_lossy());
    let guards = quote(&directory.join("guards").to_string_lossy());
    let result = (|| {
        if shell == "bash" {
            let rc = directory.join("bashrc");
            let body=[
                "[[ ! -r ~/.bashrc ]] || builtin source ~/.bashrc".into(),
                r#"if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )) && [[ ! $(declare -p PROMPT_COMMAND PS0 2>/dev/null) =~ declare\ -[^[:space:]]*r ]]; then"#.into(),
                "  __dsh_shell_pid=$BASHPID; __dsh_shell_sequence=0".into(),
                "  __dsh_shell_idle() {".into(),"    local result=$?".into(),
                r#"    if [[ $BASHPID == "$__dsh_shell_pid" ]]; then"#.into(),
                "      (( ++__dsh_shell_sequence ))".into(),"      local activity=idle".into(),
                format!("      builtin trap -p >| {guards}"),format!("      [[ ! -s {guards} ]] || activity=unknown"),
                format!(r#"      builtin printf '%s:%s:%s\n' "$BASHPID" "$__dsh_shell_sequence" "$activity" >| {state}"#),
                "    fi".into(),r#"    return "$result""#.into(),"  }".into(),
                r#"  if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a "* ]]; then"#.into(),
                "    PROMPT_COMMAND+=(__dsh_shell_idle)".into(),"  else".into(),
                r#"    PROMPT_COMMAND="${PROMPT_COMMAND}"$'\n'"__dsh_shell_idle""#.into(),"  fi".into(),
                format!("  PS0+={}",quote(&format!(r#"$(builtin printf '%s:%s:busy' "$__dsh_shell_pid" "$__dsh_shell_sequence" >| {state})"#))),
                "fi".into(),String::new(),
            ].join("\n");
            write(&rc, &body)?;
            Ok(Some(ShellActivity::new(
                directory.clone(),
                vec![
                    spec.argv[0].clone(),
                    "--rcfile".into(),
                    rc.to_string_lossy().into_owned(),
                    "-i".into(),
                ],
                env,
            )))
        } else {
            let body=[env.get("ZDOTDIR").map_or_else(||"unset ZDOTDIR".into(),|value|format!("ZDOTDIR={}",quote(value))),
                r#"[[ ! -r ${ZDOTDIR:-$HOME}/.zshenv ]] || builtin source "${ZDOTDIR:-$HOME}/.zshenv""#.into(),
                "typeset -g __dsh_shell_pid=$$ __dsh_shell_sequence=0".into(),"__dsh_shell_activity() {".into(),
                "  (( ZSH_SUBSHELL == 0 && $$ == __dsh_shell_pid )) || return".into(),"  (( ++__dsh_shell_sequence ))".into(),
                format!(r#"  builtin printf '%s:%s:%s\n' "$$" "$__dsh_shell_sequence" "$1" >| {state}"#),"  return 0".into(),"}".into(),"__dsh_shell_idle() {".into(),
                format!("  {{ builtin trap; zle -F; }} >| {guards}"),
                "  if [[ $CONTEXT != start || -n $BUFFER ]]; then __dsh_shell_activity busy".into(),
                format!("  elif [[ -s {guards} || -n ${{(k)functions[(I)TRAP*]}} ]]; then __dsh_shell_activity unknown"),
                "  else __dsh_shell_activity idle; fi".into(),"}".into(),"__dsh_shell_busy() { __dsh_shell_activity busy }".into(),"__dsh_shell_init() {".into(),
                "  autoload -Uz add-zle-hook-widget add-zsh-hook".into(),"  add-zle-hook-widget line-init __dsh_shell_idle".into(),"  add-zle-hook-widget line-finish __dsh_shell_busy".into(),"  add-zsh-hook preexec __dsh_shell_busy".into(),"  precmd_functions=(${precmd_functions:#__dsh_shell_init})".into(),"}".into(),"typeset -ga precmd_functions".into(),"precmd_functions+=(__dsh_shell_init)".into(),String::new(),
            ].join("\n");
            write(&directory.join(".zshenv"), &body)?;
            let mut env = env;
            env.insert("ZDOTDIR".into(), directory.to_string_lossy().into_owned());
            Ok(Some(ShellActivity::new(
                directory.clone(),
                spec.argv.clone(),
                env,
            )))
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&directory);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_the_allocated_private_directory_when_writing_its_startup_file_fails() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("allocated");
        let spec = SubprocessTerminalSpawnSpec {
            argv: vec!["/bin/zsh".into(), "-i".into()],
            cwd: "/tmp".into(),
            env: None,
            rows: 24,
            cols: 80,
            terminal_type: "dumb".into(),
            grace_ms: 100.0,
            signal: None,
            shell_activity: Some(true),
        };
        let error = prepare_with(
            &spec,
            Default::default(),
            "linux",
            || {
                std::fs::create_dir(&directory)?;
                Ok(directory.clone())
            },
            |_, _| Err(crate::error("startup write failed")),
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "startup write failed");
        assert!(!directory.exists());
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    }
}
