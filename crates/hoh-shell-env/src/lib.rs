//! Port of @deepseek-ai/dsh-shell-env (packages/shell/shell-env).
//!
//! Tool-independent shell environment plugin: owns the `shellEnv` registry of trusted, per-execution
//! `DSH_*` variables consumed by the model-facing shell tools. Built-in shell facts are owned by the
//! registry itself while plugins can register additional, enumerable facts with effect-scoped
//! disposal.

mod error;

use std::{
    cmp::Ordering,
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use hoh_error::BoxError;
use hoh_home_paths::{DSH_HOME_ENV, resolve_dsh_home};
use hoh_kernel::{Cfg, Context, Disposer, Plugin, Undo};
use hoh_loader::ProfileContextKey;
use hoh_shell::{DSH_ENV_PREFIX, DshEnvironment};
use hoh_tools::ToolExecution;
use hoh_util_values::js_trim;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use error::{SHELL_ENV_ERROR_CODES, ShellEnvError, ShellEnvErrorCode};

hoh_kernel::service!(pub ShellEnv: ShellEnvRegistry = "shellEnv");

/// The `installAnchor` of the launcher's profile context: the file that marks the installation
/// root (`<root>/package.json` in the source). `DSH_INSTALL_DIR` is its directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallAnchor(pub PathBuf);

// SHIM(hoh-loader): `ProfileContext` carries no `installAnchor`; the launcher provides this service
// next to `profileContext` until the loader's context has the field. Replace then.
hoh_kernel::service!(pub InstallAnchorKey: InstallAnchor = "installAnchor");

const DSH_SHELL_KEY: &str = "DSH_SHELL";
const DSH_SESSION_ID_KEY: &str = "DSH_SESSION_ID";
const DSH_PROFILE_KEY: &str = "DSH_PROFILE";
const DSH_PROFILE_DIR_KEY: &str = "DSH_PROFILE_DIR";
const DSH_INSTALL_DIR_KEY: &str = "DSH_INSTALL_DIR";
const DSH_NODE_EXECUTABLE_KEY: &str = "DSH_NODE_EXECUTABLE";
const RESERVED_BASH_ENV_KEYS: [&str; 7] = [
    DSH_HOME_ENV,
    DSH_SHELL_KEY,
    DSH_SESSION_ID_KEY,
    DSH_PROFILE_KEY,
    DSH_PROFILE_DIR_KEY,
    DSH_INSTALL_DIR_KEY,
    DSH_NODE_EXECUTABLE_KEY,
];

/// Plugin config (all optional — the built-in facts resolve without defaults).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
#[schemars(extend("default" = serde_json::json!({})))]
pub struct Config {
    /// DeepSeek Harness home directory exposed as `DSH_HOME`; defaults to `$DSH_HOME` or `~/.dsh`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsh_home: Option<String>,
}

/// Model-visible metadata for one managed `DSH_*` environment variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BashEnvVariable {
    /// Concise description of the environment fact represented by the variable.
    pub description: String,
}

/// Resolve one contributor's available values for one tool execution; returns a partial map
/// containing only keys declared in `variables`.
pub type ResolveFn = Arc<dyn Fn(&ToolExecution) -> IndexMap<String, String> + Send + Sync>;

/// A plugin contribution to the managed environment of each model shell call. Declared keys make
/// ownership conflicts detectable before the first command; `resolve` computes only the values
/// available for the current execution.
#[derive(Clone)]
pub struct BashEnvContributor {
    /// Stable contributor name used in diagnostics and duplicate detection.
    pub name: String,
    /// Complete set of `DSH_*` keys this contributor may return.
    pub variables: IndexMap<String, BashEnvVariable>,
    pub resolve: ResolveFn,
}

/// An enumerable declaration returned by [`ShellEnvRegistry::list`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BashEnvVariableInfo {
    /// Contributor that owns the variable.
    pub contributor: String,
    pub description: String,
    /// Declared `DSH_*` environment variable name.
    pub key: String,
}

#[derive(Default)]
struct State {
    contributors: IndexMap<String, Arc<BashEnvContributor>>,
    key_owners: IndexMap<String, String>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Registry (`shellEnv`) for trusted, per-execution `DSH_*` variables. The namespace is rebuilt for
/// every model shell call: ambient `DSH_*` values are discarded by the executor, then the registry's
/// current snapshot is injected.
pub struct ShellEnvRegistry {
    ctx: Context,
    dsh_home: String,
    state: Arc<Mutex<State>>,
}

fn path_text(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

/// `^[A-Z][A-Z0-9_]*$` (without the multiline flag).
fn is_valid_key_suffix(suffix: &str) -> bool {
    let mut chars = suffix.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Primary collation class of one ASCII character in the root collation: punctuation, then digits,
/// then letters (case-insensitive at the primary level).
fn collation_key(c: char) -> (u8, u32) {
    if c.is_ascii_alphabetic() {
        (2, u32::from(c.to_ascii_lowercase()))
    } else if c.is_ascii_digit() {
        (1, u32::from(c))
    } else {
        (0, u32::from(c))
    }
}

/// `String.prototype.localeCompare` for the ASCII names this registry sorts (contributor names and
/// `DSH_*` keys); see PORT_NOTES for what is not reproduced.
fn locale_compare(left: &str, right: &str) -> Ordering {
    let primary = left
        .chars()
        .map(collation_key)
        .cmp(right.chars().map(collation_key));
    // Tertiary level: a lowercase letter sorts before its uppercase form.
    primary.then_with(|| {
        left.chars()
            .map(|c| c.is_ascii_uppercase())
            .cmp(right.chars().map(|c| c.is_ascii_uppercase()))
    })
}

impl ShellEnvRegistry {
    /// Create the registry for `ctx`; `config` supplies the home directory of the built-in variables.
    pub fn new(ctx: &Context, config: &Config) -> Result<Self, BoxError> {
        Self::with_env(ctx, config, None)
    }

    fn with_env(
        ctx: &Context,
        config: &Config,
        env: Option<&HashMap<String, String>>,
    ) -> Result<Self, BoxError> {
        Ok(Self {
            ctx: ctx.clone(),
            dsh_home: path_text(resolve_dsh_home(config.dsh_home.as_deref(), env)?),
            state: Arc::default(),
        })
    }

    /// Register one environment contributor. Names and keys are unique; built-in keys are reserved.
    /// Registration is disposed with the calling plugin fiber.
    ///
    /// # Errors
    /// An `Error` with the source's message for an empty name, a duplicate name, an invalid or
    /// reserved or already owned key, or a blank description.
    pub fn register(
        &self,
        ctx: &Context,
        contributor: BashEnvContributor,
    ) -> Result<Disposer, BoxError> {
        let contributor = Arc::new(contributor);
        let state = self.state.clone();
        ctx.try_effect("bashEnv.register()", move |undos| -> Result<(), BoxError> {
            let name = &contributor.name;
            {
                let mut state = lock(&state);
                if js_trim(name).is_empty() {
                    return Err("bash env contributor name must be non-empty".into());
                }
                if state.contributors.contains_key(name) {
                    return Err(format!("bash env contributor \"{name}\" is already registered").into());
                }
                for (key, variable) in &contributor.variables {
                    let valid = key
                        .strip_prefix(DSH_ENV_PREFIX)
                        .is_some_and(is_valid_key_suffix);
                    if !valid {
                        return Err(format!(
                            "bash env contributor \"{name}\" declared invalid key \"{key}\""
                        )
                        .into());
                    }
                    if RESERVED_BASH_ENV_KEYS.contains(&key.as_str()) {
                        return Err(format!(
                            "bash env contributor \"{name}\" cannot own reserved key \"{key}\""
                        )
                        .into());
                    }
                    if js_trim(&variable.description).is_empty() {
                        return Err(
                            format!("bash env contributor \"{name}\" must describe \"{key}\"").into()
                        );
                    }
                    if let Some(owner) = state.key_owners.get(key) {
                        return Err(format!(
                            "bash env key \"{key}\" is already owned by contributor \"{owner}\"; contributor \"{name}\" cannot also own it"
                        )
                        .into());
                    }
                }
                state.contributors.insert(name.clone(), contributor.clone());
                for key in contributor.variables.keys() {
                    state.key_owners.insert(key.clone(), name.clone());
                }
            }
            let state = state.clone();
            let contributor = contributor.clone();
            undos.push(Undo::sync(move || {
                let mut state = lock(&state);
                state.contributors.shift_remove(&contributor.name);
                for key in contributor.variables.keys() {
                    state.key_owners.shift_remove(key);
                }
            }))?;
            Ok(())
        })
    }

    /// `dirname(profile.installAnchor)`. Until the launcher provides the anchor the directory of the
    /// running executable stands in for it, which is the installation root only for a layout where
    /// the binary sits there.
    fn install_dir(&self) -> String {
        if let Some(anchor) = self.ctx.get::<InstallAnchorKey>() {
            return hoh_util_workspace_path::node_path::dirname(&path_text(anchor.0.clone()));
        }
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
            .map(path_text)
            .unwrap_or_default()
    }

    /// Build the trusted `DSH_*` snapshot for one shell tool execution: built-ins and current
    /// contributions, sorted by name.
    ///
    /// # Errors
    /// `ShellEnvError` (`SHELL_ENV_CONTRIBUTION_INVALID`) when a contributor returns an undeclared key.
    pub fn collect(&self, execution: &ToolExecution) -> Result<DshEnvironment, BoxError> {
        let mut values: IndexMap<String, String> = IndexMap::new();
        values.insert(DSH_HOME_ENV.to_owned(), self.dsh_home.clone());
        values.insert(DSH_SHELL_KEY.to_owned(), "1".to_owned());
        // `DSH_NODE_EXECUTABLE` is a fact of a Node script run; this host is a single executable,
        // the case where the source omits it (PORT_NOTES).
        if let Some(agent) = &execution.agent {
            values.insert(
                DSH_SESSION_ID_KEY.to_owned(),
                agent.session().id().as_str().to_owned(),
            );
        }
        if let Some(profile) = self.ctx.get::<ProfileContextKey>() {
            values.insert(DSH_PROFILE_KEY.to_owned(), profile.name.clone());
            values.insert(
                DSH_PROFILE_DIR_KEY.to_owned(),
                path_text(profile.dir.clone()),
            );
            values.insert(DSH_INSTALL_DIR_KEY.to_owned(), self.install_dir());
        }

        let mut contributors: Vec<Arc<BashEnvContributor>> =
            lock(&self.state).contributors.values().cloned().collect();
        contributors.sort_by(|left, right| locale_compare(&left.name, &right.name));
        for contributor in contributors {
            let resolved = (contributor.resolve)(execution);
            for (key, value) in resolved {
                if !contributor.variables.contains_key(&key) {
                    return Err(Box::new(ShellEnvError::new(
                        format!(
                            "bash env contributor \"{}\" returned undeclared key \"{key}\"",
                            contributor.name
                        ),
                        ShellEnvErrorCode::ShellEnvContributionInvalid,
                    )));
                }
                values.insert(key, value);
            }
        }

        let mut entries: Vec<(String, String)> = values.into_iter().collect();
        entries.sort_by(|(left, _), (right, _)| locale_compare(left, right));
        Ok(entries.into_iter().collect())
    }

    /// Enumerate plugin-contributed variables without executing their resolvers, sorted by
    /// environment variable name.
    pub fn list(&self) -> Vec<BashEnvVariableInfo> {
        let mut infos: Vec<BashEnvVariableInfo> = lock(&self.state)
            .contributors
            .values()
            .flat_map(|contributor| {
                contributor
                    .variables
                    .iter()
                    .map(|(key, variable)| BashEnvVariableInfo {
                        contributor: contributor.name.clone(),
                        description: variable.description.clone(),
                        key: key.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        infos.sort_by(|left, right| locale_compare(&left.key, &right.key));
        infos
    }
}

/// The shell-env plugin: registers the `shellEnv` registry service.
pub struct ShellEnvPlugin;

impl Plugin for ShellEnvPlugin {
    const NAME: &'static str = "shell-env";
    type Config = Config;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), config: Cfg<Config>) -> Result<(), BoxError> {
        let registry = ShellEnvRegistry::new(&ctx, &config.get())?;
        ctx.provide::<ShellEnv>(Arc::new(registry)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with_env(env: &HashMap<String, String>) -> ShellEnvRegistry {
        let kernel = hoh_kernel::Kernel::start();
        ShellEnvRegistry::with_env(&kernel.root(), &Config::default(), Some(env)).unwrap()
    }

    #[tokio::test]
    async fn resolves_dsh_home_from_the_ambient_override_or_the_user_home_default() {
        let ambient = HashMap::from([("DSH_HOME".to_owned(), "./ambient-dsh-home".to_owned())]);
        assert_eq!(
            registry_with_env(&ambient).dsh_home,
            path_text(
                std::path::absolute("./ambient-dsh-home")
                    .map(|p| {
                        // `path.resolve` normalizes the leading `./`.
                        p.components().collect::<PathBuf>()
                    })
                    .unwrap()
            )
        );
        let fallback = registry_with_env(&HashMap::new());
        assert_eq!(
            fallback.dsh_home,
            path_text(resolve_dsh_home(None, Some(&HashMap::new())).unwrap())
        );
        assert!(fallback.dsh_home.ends_with(".dsh"));
    }

    #[test]
    fn locale_compare_orders_punctuation_before_digits_before_letters() {
        assert_eq!(locale_compare("DSH_A_B", "DSH_AB"), Ordering::Less);
        assert_eq!(locale_compare("DSH_A1", "DSH_AA"), Ordering::Less);
        assert_eq!(locale_compare("a", "A"), Ordering::Less);
    }
}
