//! Kimi Code CLI sessions over ACP v1. Kimi owns its configuration, credentials and history.

mod session;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_external_harness::{
    AccessMode, ApprovalMode, AuthState, Discovery, ExternalHarness, ExternalHarnesses,
    HarnessEffort, HarnessFailure, HarnessModel, HarnessPolicy, HarnessResult, InteractionMode,
    NetworkAccess, ProcessSpec, SandboxMode, StartParams, StartResult, Started, VersionStatus,
    code,
};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use session::Peer;

const INSTALL: &str =
    "Kimi Code CLI (kimi) was not found. Install Kimi Code CLI and recheck the provider.";
const LOGIN: &str = "Run kimi login in a terminal, then recheck Kimi Code.";
const FULL_ACCESS: &str = "Choose Full access in the composer to use Kimi Code.";
const AUTH_REQUIRED: &str = "kimi-auth-required";

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// An absolute path or a program name on PATH. The default also checks the Kimi installer path.
    pub executable: Option<String>,
}

#[derive(Inject)]
pub struct Deps {
    harnesses: Service<ExternalHarnesses>,
}

pub struct HarnessKimiPlugin;

impl Plugin for HarnessKimiPlugin {
    const NAME: &'static str = "HarnessKimi";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let harness = KimiHarness::new(config.get().executable.as_deref().unwrap_or("kimi").into());
        deps.harnesses.register(&ctx, "kimi", Arc::new(harness))?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct KimiHarness {
    executable: PathBuf,
    catalog: Arc<RwLock<Vec<HarnessModel>>>,
}

impl KimiHarness {
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            catalog: Arc::default(),
        }
    }

    fn executable(&self) -> HarnessResult<PathBuf> {
        fn executable_file(path: &Path) -> bool {
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        }

        let selected = if self.executable.is_absolute() {
            executable_file(&self.executable).then(|| self.executable.clone())
        } else if self.executable.parent() == Some(Path::new("")) {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let home = std::env::var_os("HOH_WINDOW_HOME").or_else(|| std::env::var_os("HOME"));
            let standalone = home
                .filter(|_| self.executable == Path::new("kimi"))
                .map(|home| PathBuf::from(home).join(".kimi-code/bin/kimi"));
            std::env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(&self.executable))
                .chain(standalone.filter(|path| path.is_absolute()))
                .find(|path| executable_file(path))
        } else {
            None
        };
        selected.ok_or_else(|| HarnessFailure::new(code::NOT_INSTALLED, INSTALL))
    }

    fn peer(&self, cwd: &Path, cwd_identity: Option<[u64; 2]>) -> HarnessResult<Peer> {
        Peer::spawn(&ProcessSpec {
            program: self.executable()?,
            args: vec!["acp".into()],
            cwd: Some(cwd.to_owned()),
            cwd_identity,
            ..Default::default()
        })
    }

    async fn discovery(&self) -> (Discovery, VersionStatus) {
        let mut found = Discovery {
            installed: true,
            version: None,
            auth: AuthState::Unknown,
            models: Vec::new(),
            detail: None,
            failure: None,
        };
        let mut status = VersionStatus::Unknown;
        let result: HarnessResult<()> = async {
            let cwd = tempfile::tempdir().map_err(|_| {
                HarnessFailure::new(
                    code::IO,
                    "Could not create a temporary Kimi probe directory.",
                )
            })?;
            let mut peer = self.peer(cwd.path(), None)?;
            let result = async {
                let info = peer.initialize().await?;
                found.version = info
                    .pointer("/agentInfo/version")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                check_capabilities(&info)?;
                status = VersionStatus::Supported;
                // Kimi's terminal-auth method refreshes readiness on this connection. Opening a
                // session here would also run the user's SessionStart hooks before access was chosen.
                peer.call("authenticate", json!({"methodId":"login"}))
                    .await?;
                found.auth = AuthState::Authenticated;
                found.models = self.catalog.read().await.clone();
                Ok(())
            }
            .await;
            peer.close().await;
            result
        }
        .await;
        if let Err(error) = result {
            found.installed = error.code != code::NOT_INSTALLED;
            if error.code == AUTH_REQUIRED {
                found.auth = AuthState::Unauthenticated;
                self.catalog.write().await.clear();
            }
            if error.code == code::UNSUPPORTED_VERSION {
                status = VersionStatus::Unsupported;
            }
            found.detail = Some(error.message);
        }
        (found, status)
    }

    async fn start(&self, params: StartParams, access: AccessMode) -> HarnessResult<Started> {
        ensure_policy(&params, access)?;
        let cwd = Path::new(&params.cwd);
        if !cwd.is_absolute() {
            return Err(HarnessFailure::new(
                code::IO,
                "Kimi requires an absolute working directory.",
            ));
        }
        let mut peer = self.peer(cwd, params.cwd_identity)?;
        let info = peer.initialize().await?;
        check_capabilities(&info)?;
        let images =
            info.pointer("/agentCapabilities/promptCapabilities/image") == Some(&json!(true));

        let (native_id, mut config, resumed) = if let Some(resume) = params.resume {
            let id = resume.as_str();
            let config = peer.resume(id, cwd).await?;
            (id.to_owned(), config, true)
        } else {
            let config = peer
                .call("session/new", json!({"cwd":cwd,"mcpServers":[]}))
                .await?;
            let id = session_id(&config)?.to_owned();
            *self.catalog.write().await = models(&config);
            (id, config, false)
        };
        if let Some(model) = params.model.as_deref().filter(|model| *model != "default") {
            config = set_config(&mut peer, &native_id, &config, "model", model).await?;
        }
        if let Some(effort) = &params.effort {
            set_config(&mut peer, &native_id, &config, "thinking", effort).await?;
        }
        // Set this explicitly after both new and resume, since Kimi can restore its own mode.
        peer.call(
            "session/set_mode",
            json!({"sessionId":native_id,"modeId":"auto"}),
        )
        .await?;
        let (session, events) = peer.into_session(native_id.clone(), images);
        Ok(Started {
            result: StartResult {
                native_session_id: native_id.into(),
                resumed,
            },
            session: Arc::new(session),
            events,
        })
    }
}

impl ExternalHarness for KimiHarness {
    fn discover(&self) -> BoxFuture<'static, HarnessResult<Discovery>> {
        let harness = self.clone();
        Box::pin(async move { Ok(harness.discovery().await.0) })
    }

    fn discover_with_status(
        &self,
    ) -> BoxFuture<'static, HarnessResult<(Discovery, VersionStatus)>> {
        let harness = self.clone();
        Box::pin(async move { Ok(harness.discovery().await) })
    }

    fn start_or_resume(&self, params: StartParams) -> BoxFuture<'static, HarnessResult<Started>> {
        self.start_with_access(params, AccessMode::Confirm)
    }

    fn start_with_access(
        &self,
        params: StartParams,
        access: AccessMode,
    ) -> BoxFuture<'static, HarnessResult<Started>> {
        let harness = self.clone();
        Box::pin(async move { harness.start(params, access).await })
    }

    fn launch_flags(&self, _policy: &HarnessPolicy, access: AccessMode) -> Vec<String> {
        if access == AccessMode::Full {
            vec!["acp".into(), "mode=auto".into()]
        } else {
            Vec::new()
        }
    }

    fn accepts_effort(&self, effort: &str) -> bool {
        !effort.is_empty()
    }
}

fn ensure_policy(params: &StartParams, access: AccessMode) -> HarnessResult<()> {
    let failure = |message| HarnessFailure::new(code::POLICY_UNAVAILABLE, message);
    if access != AccessMode::Full {
        return Err(failure(FULL_ACCESS));
    }
    if params.policy.sandbox != SandboxMode::FullAccess
        || params.policy.network != NetworkAccess::Allowed
        || params.policy.approval == ApprovalMode::Never
    {
        return Err(failure(
            "Kimi Code cannot enforce a filesystem or network sandbox.",
        ));
    }
    if params.interaction_mode != InteractionMode::Default {
        return Err(failure("Kimi Code does not support Plan mode in HoH."));
    }
    Ok(())
}

fn check_capabilities(info: &Value) -> HarnessResult<()> {
    let capabilities = &info["agentCapabilities"]["sessionCapabilities"];
    if info["protocolVersion"] != 1 || !capabilities["resume"].is_object() {
        return Err(HarnessFailure::new(
            code::UNSUPPORTED_VERSION,
            "Update Kimi Code CLI to a release with ACP v1 session resume support.",
        ));
    }
    Ok(())
}

fn session_id(result: &Value) -> HarnessResult<&str> {
    result["sessionId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            HarnessFailure::new(code::PROTOCOL, "Kimi Code did not return a session ID.")
        })
}

fn option<'a>(config: &'a Value, id: &str) -> Option<&'a Value> {
    config["configOptions"]
        .as_array()?
        .iter()
        .find(|option| option["id"] == id)
}

async fn set_config(
    peer: &mut Peer,
    native_id: &str,
    config: &Value,
    id: &str,
    value: &str,
) -> HarnessResult<Value> {
    let setting = option(config, id);
    if setting.is_some_and(|setting| setting["currentValue"] == value) {
        return Ok(config.clone());
    }
    if !setting
        .and_then(|setting| setting["options"].as_array())
        .is_some_and(|options| options.iter().any(|option| option["value"] == value))
    {
        return Err(HarnessFailure::new(
            code::HARNESS,
            format!("Kimi Code does not offer the selected {id} option."),
        ));
    }
    peer.call(
        "session/set_config_option",
        json!({"sessionId":native_id,"configId":id,"value":value}),
    )
    .await
}

fn models(config: &Value) -> Vec<HarnessModel> {
    let Some(model) = option(config, "model") else {
        return Vec::new();
    };
    let thinking = option(config, "thinking");
    model["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = entry["value"].as_str().filter(|id| !id.is_empty())?;
            let is_default = model["currentValue"] == id;
            // ACP's thinking options describe only the currently selected model.
            let thinking = thinking.filter(|_| is_default);
            let effort_levels = thinking
                .and_then(|setting| setting["options"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    Some(HarnessEffort {
                        id: entry["value"].as_str()?.to_owned(),
                        description: entry["description"].as_str().map(str::to_owned),
                    })
                })
                .collect();
            Some(HarnessModel {
                id: id.to_owned(),
                display_name: entry["name"].as_str().map(str::to_owned),
                is_default,
                effort_levels,
                default_effort: thinking
                    .and_then(|setting| setting["currentValue"].as_str())
                    .map(str::to_owned),
            })
        })
        .collect()
}
