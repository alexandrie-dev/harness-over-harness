//! Registry tests for `@deepseek-ai/dsh-shell-env`: built-in facts, contributor ownership and
//! validation, collection ordering, effect-scoped disposal, and the explicit disposer contract.

mod common;

use std::{path::PathBuf, sync::Arc};

use common::*;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, Inject, Plugin, Service};
use hoh_loader::{ProfileContext, ProfileContextKey};
use hoh_shell_env::{
    BashEnvContributor, BashEnvVariable, BashEnvVariableInfo, Config, InstallAnchor,
    InstallAnchorKey, ShellEnv, ShellEnvPlugin, ShellEnvRegistry,
};
use indexmap::IndexMap;
use serde_json::json;

fn home_config() -> Config {
    Config {
        dsh_home: Some("./test-dsh-home".to_owned()),
    }
}

fn resolved_home() -> String {
    std::path::absolute("./test-dsh-home")
        .unwrap()
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

fn registry(env: &Env) -> ShellEnvRegistry {
    ShellEnvRegistry::new(&env.ctx, &home_config()).unwrap()
}

fn contributor(
    name: &str,
    variables: &[(&str, &str)],
    resolve: impl Fn(&hoh_tools::ToolExecution) -> IndexMap<String, String> + Send + Sync + 'static,
) -> BashEnvContributor {
    BashEnvContributor {
        name: name.to_owned(),
        variables: variables
            .iter()
            .map(|(key, description)| {
                (
                    (*key).to_owned(),
                    BashEnvVariable {
                        description: (*description).to_owned(),
                    },
                )
            })
            .collect(),
        resolve: Arc::new(resolve),
    }
}

fn values(entries: &[(&str, &str)]) -> IndexMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn register_error(
    registry: &ShellEnvRegistry,
    env: &Env,
    contributor: BashEnvContributor,
) -> String {
    registry
        .register(&env.ctx, contributor)
        .err()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn collects_unconditional_shell_facts_and_the_current_agent_session_id() {
    let env = setup().await;
    let registry = registry(&env);

    // `DSH_NODE_EXECUTABLE` is omitted: the single-executable case of the source (PORT_NOTES).
    let anonymous = registry.collect(&*env.execution(None).await).unwrap();
    assert_eq!(
        anonymous,
        values(&[("DSH_HOME", &resolved_home()), ("DSH_SHELL", "1")])
    );
    let with_session = registry
        .collect(&*env.execution(Some("session-a")).await)
        .unwrap();
    assert_eq!(
        with_session,
        values(&[
            ("DSH_HOME", &resolved_home()),
            ("DSH_SESSION_ID", "session-a"),
            ("DSH_SHELL", "1"),
        ])
    );
}

#[tokio::test]
async fn omits_the_node_executable_inside_a_single_executable_application() {
    let env = setup().await;
    let collected = ShellEnvRegistry::new(&env.ctx, &Config::default())
        .unwrap()
        .collect(&*env.execution(None).await)
        .unwrap();
    assert!(!collected.contains_key("DSH_NODE_EXECUTABLE"));
}

#[tokio::test]
async fn collects_the_launcher_provided_profile_name_directory_and_installation_directory_when_a_profile_context_exists()
 {
    let env = setup().await;
    env.ctx
        .provide::<ProfileContextKey>(Arc::new(ProfileContext {
            name: "web".to_owned(),
            dir: PathBuf::from("/profiles/web"),
            home: PathBuf::from("/home"),
        }))
        .await
        .unwrap();
    // The installation anchor is a file in the installation root, not next to the binary.
    env.ctx
        .provide::<InstallAnchorKey>(Arc::new(InstallAnchor(PathBuf::from("/dsh/package.json"))))
        .await
        .unwrap();
    let registry = registry(&env);
    let collected = registry.collect(&*env.execution(None).await).unwrap();
    assert_eq!(collected["DSH_PROFILE"], "web");
    assert_eq!(collected["DSH_PROFILE_DIR"], "/profiles/web");
    assert_eq!(collected["DSH_INSTALL_DIR"], "/dsh");
    let binary_dir = std::env::current_exe().unwrap();
    assert_ne!(
        collected["DSH_INSTALL_DIR"],
        binary_dir.parent().unwrap().to_string_lossy()
    );
    for key in ["DSH_PROFILE", "DSH_INSTALL_DIR", "DSH_NODE_EXECUTABLE"] {
        let error = register_error(
            &registry,
            &env,
            contributor(&format!("{key}-claimer"), &[(key, "Reserved key.")], |_| {
                IndexMap::new()
            }),
        );
        assert!(
            error.contains(&format!("reserved key \"{key}\"")),
            "{error}"
        );
    }
}

// SHIM(hoh-loader): without a provided anchor the executable's directory stands in for it.
#[tokio::test]
async fn falls_back_to_the_executable_directory_while_no_install_anchor_is_provided() {
    let env = setup().await;
    env.ctx
        .provide::<ProfileContextKey>(Arc::new(ProfileContext {
            name: "web".to_owned(),
            dir: PathBuf::from("/profiles/web"),
            home: PathBuf::from("/home"),
        }))
        .await
        .unwrap();
    let collected = registry(&env).collect(&*env.execution(None).await).unwrap();
    let binary_dir = std::env::current_exe().unwrap();
    assert_eq!(
        collected["DSH_INSTALL_DIR"],
        binary_dir.parent().unwrap().to_string_lossy()
    );
}

#[tokio::test]
async fn collects_declared_contributor_variables_and_omits_unavailable_values() {
    let env = setup().await;
    let registry = registry(&env);
    registry
        .register(
            &env.ctx,
            contributor(
                "optional-session-fact",
                &[("DSH_SESSION_OPTIONAL", "Optional session-scoped test fact.")],
                |exec| match &exec.agent {
                    None => IndexMap::new(),
                    Some(agent) => {
                        values(&[("DSH_SESSION_OPTIONAL", agent.session().id().as_str())])
                    }
                },
            ),
        )
        .unwrap();
    registry
        .register(
            &env.ctx,
            contributor(
                "always-available-fact",
                &[("DSH_ALWAYS_AVAILABLE", "Always-available test fact.")],
                |_| values(&[("DSH_ALWAYS_AVAILABLE", "yes")]),
            ),
        )
        .unwrap();

    assert!(
        !registry
            .collect(&*env.execution(None).await)
            .unwrap()
            .contains_key("DSH_SESSION_OPTIONAL")
    );
    assert_eq!(
        registry.collect(&*env.execution(None).await).unwrap()["DSH_ALWAYS_AVAILABLE"],
        "yes"
    );
    assert_eq!(
        registry
            .collect(&*env.execution(Some("session-b")).await)
            .unwrap()["DSH_SESSION_OPTIONAL"],
        "session-b"
    );
    assert_eq!(
        registry.list(),
        vec![
            BashEnvVariableInfo {
                contributor: "always-available-fact".to_owned(),
                description: "Always-available test fact.".to_owned(),
                key: "DSH_ALWAYS_AVAILABLE".to_owned(),
            },
            BashEnvVariableInfo {
                contributor: "optional-session-fact".to_owned(),
                description: "Optional session-scoped test fact.".to_owned(),
                key: "DSH_SESSION_OPTIONAL".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn rejects_duplicate_variable_ownership_at_registration_time() {
    let env = setup().await;
    let registry = registry(&env);
    registry
        .register(
            &env.ctx,
            contributor("first", &[("DSH_SHARED", "First owner.")], |_| {
                values(&[("DSH_SHARED", "first")])
            }),
        )
        .unwrap();
    let error = register_error(
        &registry,
        &env,
        contributor("second", &[("DSH_SHARED", "Second owner.")], |_| {
            values(&[("DSH_SHARED", "second")])
        }),
    );
    assert!(
        error.contains("DSH_SHARED") && error.contains("first") && error.contains("second"),
        "{error}"
    );
}

#[tokio::test]
async fn rejects_duplicate_contributor_names_and_malformed_declarations() {
    let env = setup().await;
    let registry = registry(&env);
    registry
        .register(
            &env.ctx,
            contributor("declared", &[("DSH_DECLARED", "Declared fact.")], |_| {
                IndexMap::new()
            }),
        )
        .unwrap();

    let cases = [
        (
            contributor("declared", &[("DSH_ANOTHER", "Another fact.")], |_| {
                IndexMap::new()
            }),
            "already registered",
        ),
        (
            contributor(" ", &[("DSH_BLANK_NAME", "Blank owner.")], |_| {
                IndexMap::new()
            }),
            "name must be non-empty",
        ),
        (
            contributor("invalid-key", &[("dsh_invalid", "Invalid key.")], |_| {
                IndexMap::new()
            }),
            "invalid key",
        ),
        (
            contributor("reserved-key", &[("DSH_HOME", "Reserved key.")], |_| {
                IndexMap::new()
            }),
            "reserved key",
        ),
        (
            contributor(
                "blank-description",
                &[("DSH_BLANK_DESCRIPTION", " ")],
                |_| IndexMap::new(),
            ),
            "must describe",
        ),
    ];
    for (candidate, expected) in cases {
        let error = register_error(&registry, &env, candidate);
        assert!(error.contains(expected), "{error}");
    }
}

fn contribution_error(error: BoxError) -> HarnessError {
    HarnessError::of(error.as_ref())
        .cloned()
        .expect("a ShellEnvError")
}

#[tokio::test]
async fn rejects_undeclared_variables_returned_by_a_contributor() {
    let env = setup().await;
    let registry = registry(&env);
    registry
        .register(
            &env.ctx,
            contributor(
                "drifted-provider",
                &[("DSH_DECLARED", "Declared fact.")],
                |_| values(&[("DSH_UNDECLARED", "bad")]),
            ),
        )
        .unwrap();

    let exec = env.execution(None).await;
    let error = registry.collect(&exec).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("drifted-provider") && message.contains("DSH_UNDECLARED"),
        "{message}"
    );
    let error = contribution_error(registry.collect(&exec).unwrap_err());
    assert_eq!(error.name(), "ShellEnvError");
    assert_eq!(error.code(), "SHELL_ENV_CONTRIBUTION_INVALID");
}

#[derive(Inject)]
struct TempDeps {
    shell_env: Service<ShellEnv>,
}

struct TemporaryPlugin;

impl Plugin for TemporaryPlugin {
    const NAME: &'static str = "temporary-plugin";
    type Config = hoh_kernel::NoConfig;
    type Deps = TempDeps;
    async fn apply(
        ctx: Context,
        deps: TempDeps,
        _config: hoh_kernel::Cfg<hoh_kernel::NoConfig>,
    ) -> Result<(), BoxError> {
        deps.shell_env.register(
            &ctx,
            contributor("temporary", &[("DSH_TEMPORARY", "Temporary fact.")], |_| {
                values(&[("DSH_TEMPORARY", "present")])
            }),
        )?;
        Ok(())
    }
}

#[tokio::test]
async fn removes_an_effect_scoped_contributor_when_its_plugin_is_disposed() {
    let env = setup().await;
    env.ctx
        .plugin::<ShellEnvPlugin>(json!({ "dshHome": "./test-dsh-home" }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let registry = env.ctx.get::<ShellEnv>().unwrap();
    let fiber = env.ctx.plugin::<TemporaryPlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();

    let exec = env.execution(None).await;
    assert_eq!(registry.collect(&exec).unwrap()["DSH_TEMPORARY"], "present");
    fiber.dispose().await;
    assert!(
        !registry
            .collect(&exec)
            .unwrap()
            .contains_key("DSH_TEMPORARY")
    );
}

#[tokio::test]
async fn returns_an_explicit_contributor_disposer() {
    let env = setup().await;
    let registry = registry(&env);
    let dispose = registry
        .register(
            &env.ctx,
            contributor(
                "explicit-disposal",
                &[("DSH_EXPLICIT_DISPOSAL", "Explicitly disposed fact.")],
                |_| values(&[("DSH_EXPLICIT_DISPOSAL", "present")]),
            ),
        )
        .unwrap();
    let exec = env.execution(None).await;
    assert_eq!(
        registry.collect(&exec).unwrap()["DSH_EXPLICIT_DISPOSAL"],
        "present"
    );
    dispose.dispose().await;
    assert!(
        !registry
            .collect(&exec)
            .unwrap()
            .contains_key("DSH_EXPLICIT_DISPOSAL")
    );
}

#[tokio::test]
async fn the_plugin_registers_the_service_with_no_contributors_on_load() {
    let env = setup().await;
    env.ctx
        .plugin::<ShellEnvPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(env.ctx.get::<ShellEnv>().unwrap().list().is_empty());
}
