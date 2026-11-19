//! User-facing permission presets over sandbox mode and approval policy.

pub mod client;
mod shim;
pub mod types;

pub use types::{PermissionCatalog, PermissionSelection, PresetOption};

use hoh_agent::{AgentCreated, Agents};
use hoh_error::BoxError;
use hoh_kernel::{
    Cfg, Contained, Context, Disposer, Inject, ListenerFailure, Plugin, Service, Undo, Volatile,
};
use hoh_sandbox::SandboxMode;
use hoh_session::{Session, SessionEvent, Sessions};
use hoh_session_projection::{
    ProjectionDefinition, ProjectionHandle, ProjectionWire, SessionProjections,
};
use hoh_shell::Shell;
use hoh_user_approval::{Approval, ApprovalPolicy, set_approval_policy};
use hoh_util_values::js_trim;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Returned when the effective knobs match no available preset.
pub const CUSTOM_PRESET: &str = "custom";
/// Canonical identity of the experimental per-call review preset.
pub const AUTO_PRESET: &str = "auto";

type Admit = Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>;
type ReadDefaultPreset = Arc<dyn Fn() -> Option<String> + Send + Sync>;
type ReadSandboxMode = Arc<dyn Fn() -> Option<SandboxMode> + Send + Sync>;
type ReadApprovalPolicy = Arc<dyn Fn() -> ApprovalPolicy + Send + Sync>;
#[cfg(test)]
type TestHook = Arc<dyn Fn() + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct GateState {
    owner: Option<std::thread::ThreadId>,
    depth: usize,
}

#[derive(Default)]
struct ReentrantGate {
    state: Mutex<GateState>,
    wake: Condvar,
}

impl ReentrantGate {
    fn enter(&self) -> ReentrantGateGuard<'_> {
        let current = std::thread::current().id();
        let mut state = lock(&self.state);
        while state.owner.is_some_and(|owner| owner != current) {
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.owner = Some(current);
        state.depth += 1;
        ReentrantGateGuard { gate: self }
    }
}

struct ReentrantGateGuard<'a> {
    gate: &'a ReentrantGate,
}

impl Drop for ReentrantGateGuard<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.gate.state);
        state.depth -= 1;
        if state.depth == 0 {
            state.owner = None;
            self.gate.wake.notify_one();
        }
    }
}

/// One preset's sandbox/approval bundle and optional client presentation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresetSpec {
    /// The `sandbox/mode` value the preset writes through.
    pub sandbox: SandboxMode,
    /// The `approval/policy` value the preset writes through.
    pub approval: ApprovalPolicy,
    /// Display label; the table key is used when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional user-facing explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_presets() -> IndexMap<String, PresetSpec> {
    IndexMap::from([
        (
            "workspace-write".into(),
            PresetSpec {
                sandbox: SandboxMode::WorkspaceWrite,
                approval: ApprovalPolicy::Ask,
                name: Some("workspace-write".into()),
                description: Some("Write inside the workspace and permitted temporary directories; wider retries require approval.".into()),
            },
        ),
        (
            "danger-full-access".into(),
            PresetSpec {
                sandbox: SandboxMode::DangerFullAccess,
                approval: ApprovalPolicy::Never,
                name: Some("danger-full-access".into()),
                description: Some("Full file access without approval prompts.".into()),
            },
        ),
    ])
}

/// Plugin config: preset table and live default for future sessions.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub presets: IndexMap<String, PresetSpec>,
    #[serde(default, skip_serializing_if = "default_preset_unset")]
    pub default_preset: Volatile<Option<String>>,
}

fn default_preset_unset(value: &Volatile<Option<String>>) -> bool {
    value.0.is_none()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            presets: default_presets(),
            default_preset: Volatile(None),
        }
    }
}

/// User setting resolved when a new session receives its initial permission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSettings {
    pub default_preset: String,
}

/// Projection state before composition defaults are applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnobState {
    pub preset: Option<String>,
    pub sandbox: Option<SandboxMode>,
    pub approval: Option<ApprovalPolicy>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionProjectionState {
    preset: Option<String>,
    sandbox: Option<SandboxMode>,
    approval: Option<ApprovalPolicy>,
    seeded: bool,
}

impl PermissionProjectionState {
    fn empty() -> Self {
        Self {
            preset: None,
            sandbox: None,
            approval: None,
            seeded: false,
        }
    }

    fn knobs(&self) -> KnobState {
        KnobState {
            preset: self.preset.clone(),
            sandbox: self.sandbox,
            approval: self.approval,
        }
    }
}

enum StateSource {
    Pending,
    Projection(ProjectionHandle<PermissionProjectionState>),
    #[cfg(test)]
    Direct,
}

struct Inner {
    presets: Arc<IndexMap<String, PresetSpec>>,
    inferred_default: String,
    read_default_preset: ReadDefaultPreset,
    read_sandbox_mode: ReadSandboxMode,
    read_approval_policy: ReadApprovalPolicy,
    auto_turn: ReentrantGate,
    auto_admit: Mutex<Option<Admit>>,
    auto_available: AtomicBool,
    state_source: Mutex<StateSource>,
    #[cfg(test)]
    before_current_projection_read: Mutex<Option<TestHook>>,
}

/// Owns configured presets, the fixed Auto hook, and the durable write path.
#[derive(Clone)]
pub struct PermissionPresetService {
    inner: Arc<Inner>,
}

hoh_kernel::service!(pub PermissionPresets: PermissionPresetService = "permissionPresets");

/// Payload-free process catalog invalidation.
pub struct CatalogChanged;
hoh_kernel::event!(
    CatalogChanged = "permission-presets/catalog-changed",
    Contained
);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct PermissionError(String);

impl PermissionError {
    fn message(value: impl Into<String>) -> BoxError {
        Box::new(Self(value.into()))
    }
}

fn auto_preset_spec() -> PresetSpec {
    PresetSpec {
        sandbox: SandboxMode::DangerFullAccess,
        approval: ApprovalPolicy::Ask,
        name: None,
        description: None,
    }
}

fn js_array_index(key: &str) -> Option<u32> {
    let index = key.parse::<u32>().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

fn order_presets_like_js(presets: IndexMap<String, PresetSpec>) -> IndexMap<String, PresetSpec> {
    let mut entries = presets.into_iter().collect::<Vec<_>>();
    entries.sort_by_key(|(key, _)| js_array_index(key).map_or((1, 0), |index| (0, index)));
    entries.into_iter().collect()
}

impl PermissionPresetService {
    fn build(
        presets: IndexMap<String, PresetSpec>,
        read_default_preset: ReadDefaultPreset,
        read_sandbox_mode: ReadSandboxMode,
        read_approval_policy: ReadApprovalPolicy,
        source: StateSource,
    ) -> Result<Self, BoxError> {
        let presets = order_presets_like_js(presets);
        if presets.contains_key(CUSTOM_PRESET) {
            return Err(PermissionError::message(
                "permission: \"custom\" is reserved for the derived not-a-preset state and cannot name a table entry",
            ));
        }
        if presets.contains_key(AUTO_PRESET) {
            return Err(PermissionError::message(
                "permission: \"auto\" is reserved and cannot name a configured preset",
            ));
        }
        if read_sandbox_mode().is_none() {
            return Err(PermissionError::message(
                "permission: the mounted bash executor does not confine (no sandboxMode) — presets bundle a sandbox mode, so composing this plugin over an unconfined executor is a misconfiguration",
            ));
        }
        let presets = Arc::new(presets);
        let inferred_default = derive_from(
            &presets,
            None,
            &read_sandbox_mode,
            &read_approval_policy,
            &KnobState {
                preset: None,
                sandbox: None,
                approval: None,
            },
        );
        let selected = read_default_preset().unwrap_or_else(|| inferred_default.clone());
        if selected == CUSTOM_PRESET {
            return Err(PermissionError::message(
                "permission: composed sandbox and approval defaults match no preset; configure defaultPreset explicitly",
            ));
        }
        if !presets.contains_key(&selected) {
            return Err(PermissionError::message(format!(
                "permission: unknown preset \"{selected}\" (known: {})",
                presets.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                presets,
                inferred_default,
                read_default_preset,
                read_sandbox_mode,
                read_approval_policy,
                auto_turn: ReentrantGate::default(),
                auto_admit: Mutex::new(None),
                auto_available: AtomicBool::new(false),
                state_source: Mutex::new(source),
                #[cfg(test)]
                before_current_projection_read: Mutex::new(None),
            }),
        })
    }

    /// Advertised preset names in JavaScript property order, followed by live Auto.
    pub fn names(&self) -> Vec<String> {
        let _turn = self.inner.auto_turn.enter();
        self.names_with_auto(self.auto_available())
    }

    /// Complete process-level catalog for current-session UI.
    /// Remote: permissionPresets.catalog
    pub fn catalog(&self) -> Result<PermissionCatalog, BoxError> {
        let _turn = self.inner.auto_turn.enter();
        let auto_available = self.auto_available();
        let names = self.names_with_auto(auto_available);
        Ok(PermissionCatalog {
            options: names
                .iter()
                .map(|name| self.option_of_with_auto(name, auto_available))
                .collect::<Result<_, _>>()?,
            default_options: self
                .inner
                .presets
                .keys()
                .map(|name| self.option_of_with_auto(name, auto_available))
                .collect::<Result<_, _>>()?,
            default_preset: self.default_preset()?,
        })
    }

    /// Publish Auto for the caller fiber's effect lifetime.
    pub fn register_auto(
        &self,
        caller: &Context,
        admit: impl Fn() -> Result<(), BoxError> + Send + Sync + 'static,
    ) -> Result<Disposer, BoxError> {
        let service = self.clone();
        let emit_ctx = caller.clone();
        caller.try_effect("permissionPresets.registerAuto()", move |undos| {
            {
                let _turn = service.inner.auto_turn.enter();
                let mut slot = lock(&service.inner.auto_admit);
                if slot.is_some() {
                    return Err(PermissionError::message(
                        "permission: preset \"auto\" is already registered",
                    ));
                }
                *slot = Some(Arc::new(admit));
                service.inner.auto_available.store(true, Ordering::Release);
            }
            service.emit_catalog_changed(&emit_ctx);
            let cleanup = service.clone();
            let cleanup_ctx = emit_ctx.clone();
            undos.push(Undo::sync(move || {
                {
                    let _turn = cleanup.inner.auto_turn.enter();
                    *lock(&cleanup.inner.auto_admit) = None;
                    cleanup.inner.auto_available.store(false, Ordering::Release);
                }
                cleanup.emit_catalog_changed(&cleanup_ctx);
            }))?;
            Ok(())
        })
    }

    /// The preset currently selected as the future-session default.
    pub fn default_preset(&self) -> Result<String, BoxError> {
        let selected = (self.inner.read_default_preset)()
            .unwrap_or_else(|| self.inner.inferred_default.clone());
        if !self.inner.presets.contains_key(&selected) {
            return Err(PermissionError::message(format!(
                "permission: unknown default preset \"{selected}\""
            )));
        }
        Ok(selected)
    }

    /// Resolve an available preset's knob bundle.
    pub fn resolve(&self, name: &str) -> Result<PresetSpec, BoxError> {
        let _turn = self.inner.auto_turn.enter();
        let auto_available = self.auto_available();
        self.spec_of_with_auto(name, auto_available).ok_or_else(|| {
            PermissionError::message(format!(
                "permission: unknown preset \"{name}\" (known: {})",
                self.names_with_auto(auto_available).join(", ")
            ))
        })
    }

    /// Build the client option for an available preset or `custom`.
    pub fn option_of(&self, name: &str) -> Result<PresetOption, BoxError> {
        let _turn = self.inner.auto_turn.enter();
        self.option_of_with_auto(name, self.auto_available())
    }

    fn option_of_with_auto(
        &self,
        name: &str,
        auto_available: bool,
    ) -> Result<PresetOption, BoxError> {
        if name == CUSTOM_PRESET {
            return Ok(PresetOption {
                value: CUSTOM_PRESET.into(),
                name: "Custom".into(),
                description: Some(
                    "Current sandbox and approval settings do not match a preset.".into(),
                ),
            });
        }
        let spec = self
            .spec_of_with_auto(name, auto_available)
            .ok_or_else(|| {
                PermissionError::message(format!(
                    "permission: unknown preset \"{name}\" (known: {})",
                    self.names_with_auto(auto_available).join(", ")
                ))
            })?;
        Ok(PresetOption {
            value: name.into(),
            name: spec.name.unwrap_or_else(|| name.into()),
            description: spec.description,
        })
    }

    /// Resolve the effective preset of one session.
    pub fn current(&self, session: &Session) -> Result<String, BoxError> {
        let auto_available = {
            let _turn = self.inner.auto_turn.enter();
            self.auto_available()
        };
        #[cfg(test)]
        self.run_before_current_projection_read();
        self.current_with_auto(session, auto_available)
    }

    /// Record a changed identity, then write only changed canonical knobs.
    pub fn set(&self, session: &Session, name: &str) -> Result<(), BoxError> {
        self.apply(session, name, |policy| set_approval_policy(session, policy))
    }

    fn apply(
        &self,
        session: &Session,
        name: &str,
        set_approval: impl FnOnce(ApprovalPolicy) -> Result<(), BoxError>,
    ) -> Result<(), BoxError> {
        if name == AUTO_PRESET {
            let unknown_auto = || {
                PermissionError::message(format!(
                    "permission: unknown preset \"auto\" (known: {})",
                    self.names_with_auto(false).join(", ")
                ))
            };
            let admit = {
                let _turn = self.inner.auto_turn.enter();
                lock(&self.inner.auto_admit).clone()
            };
            let Some(admit) = admit else {
                return Err(unknown_auto());
            };
            admit()?;
            return self.apply_resolved(session, name, auto_preset_spec(), true, set_approval);
        }
        let spec = self.resolve(name)?;
        let auto_available = self.auto_available();
        self.apply_resolved(session, name, spec, auto_available, set_approval)
    }

    fn apply_resolved(
        &self,
        session: &Session,
        name: &str,
        spec: PresetSpec,
        auto_available: bool,
        set_approval: impl FnOnce(ApprovalPolicy) -> Result<(), BoxError>,
    ) -> Result<(), BoxError> {
        let current = self.current_with_auto(session, auto_available)?;
        let knobs = self.permission_state(session)?.knobs();
        if current != name {
            session.append("permission/preset", json!({ "preset": name }), None)?;
        }
        let sandbox = knobs.sandbox.or_else(|| (self.inner.read_sandbox_mode)());
        if sandbox != Some(spec.sandbox) {
            shim::set_sandbox_mode(session, spec.sandbox)?;
        }
        let approval = knobs
            .approval
            .unwrap_or_else(|| (self.inner.read_approval_policy)());
        if approval != spec.approval {
            set_approval(spec.approval)?;
        }
        Ok(())
    }

    fn derive(&self, state: &KnobState) -> String {
        let _turn = self.inner.auto_turn.enter();
        self.derive_with_auto(state, self.auto_available())
    }

    fn derive_with_auto(&self, state: &KnobState, auto_available: bool) -> String {
        derive_from(
            &self.inner.presets,
            auto_available.then_some(auto_preset_spec()),
            &self.inner.read_sandbox_mode,
            &self.inner.read_approval_policy,
            state,
        )
    }

    fn permission_state(&self, session: &Session) -> Result<PermissionProjectionState, BoxError> {
        match &*lock(&self.inner.state_source) {
            StateSource::Pending => Err(PermissionError::message(
                "permission: presets are not ready; await plugin activation before creating or restoring a session",
            )),
            StateSource::Projection(handle) => Ok((*handle.state_of(session)?).clone()),
            #[cfg(test)]
            StateSource::Direct => fold_session(session),
        }
    }

    fn pin_initial_permission(&self, session: &Session) -> Result<(), BoxError> {
        let state = self.permission_state(session)?;
        let persisted_auto = state.preset.as_deref() == Some(AUTO_PRESET);
        if persisted_auto {
            let admit = {
                let _turn = self.inner.auto_turn.enter();
                lock(&self.inner.auto_admit).clone()
            }
            .ok_or_else(|| {
                PermissionError::message(
                    "permission: cannot restore preset \"auto\" without its active integration",
                )
            })?;
            admit()?;
        }
        if state.preset.is_none()
            && state.sandbox.is_none()
            && state.approval.is_none()
            && !state.seeded
        {
            let name = self.default_preset()?;
            let spec = self.resolve(&name)?;
            session.append("permission/preset", json!({ "preset": name }), None)?;
            shim::set_sandbox_mode(session, spec.sandbox)?;
            set_approval_policy(session, spec.approval)?;
            return Ok(());
        }
        let auto_available = persisted_auto || self.auto_available();
        let effective = self.derive_with_auto(&state.knobs(), auto_available);
        if state.preset.is_none() && effective != CUSTOM_PRESET {
            session.append("permission/preset", json!({ "preset": effective }), None)?;
        }
        if state.sandbox.is_none() {
            let mode = (self.inner.read_sandbox_mode)().ok_or_else(|| {
                PermissionError::message("permission: the mounted bash executor does not confine (no sandboxMode) — presets bundle a sandbox mode, so composing this plugin over an unconfined executor is a misconfiguration")
            })?;
            shim::set_sandbox_mode(session, mode)?;
        }
        if state.approval.is_none() {
            set_approval_policy(session, (self.inner.read_approval_policy)())?;
        }
        Ok(())
    }

    fn spec_of_with_auto(&self, name: &str, auto_available: bool) -> Option<PresetSpec> {
        self.inner
            .presets
            .get(name)
            .cloned()
            .or_else(|| (name == AUTO_PRESET && auto_available).then(auto_preset_spec))
    }

    fn names_with_auto(&self, auto_available: bool) -> Vec<String> {
        self.inner
            .presets
            .keys()
            .cloned()
            .chain(auto_available.then(|| AUTO_PRESET.to_owned()))
            .collect()
    }

    fn auto_available(&self) -> bool {
        self.inner.auto_available.load(Ordering::Acquire)
    }

    fn current_with_auto(
        &self,
        session: &Session,
        auto_available: bool,
    ) -> Result<String, BoxError> {
        Ok(self.derive_with_auto(&self.permission_state(session)?.knobs(), auto_available))
    }

    #[cfg(test)]
    fn run_before_current_projection_read(&self) {
        let hook = lock(&self.inner.before_current_projection_read).clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    fn emit_catalog_changed(&self, ctx: &Context) {
        let logger = ctx.logger();
        ctx.dispatch(CatalogChanged).invoke(move |failure| {
            let error = match failure {
                ListenerFailure::Threw(error) | ListenerFailure::Rejected(error) => error,
            };
            logger.warn(format_args!(
                "permission: catalog-changed listener failed: {error}"
            ));
        });
    }

    // SHIM(hoh-commands): replace this private handler with registry registration.
    #[allow(dead_code)]
    fn execute_permission_command(
        &self,
        session: &Session,
        raw_input: &str,
        set_approval: impl FnOnce(ApprovalPolicy) -> Result<(), BoxError>,
    ) -> Result<shim::CommandResult, BoxError> {
        let name = js_trim(raw_input);
        if name.is_empty() {
            return Ok(shim::CommandResult {
                kind: "success",
                text: format!(
                    "current preset {} (available: {})",
                    self.current(session)?,
                    self.names().join(", ")
                ),
            });
        }
        if !self.names().iter().any(|candidate| candidate == name) {
            return Ok(shim::CommandResult {
                kind: "error",
                text: format!(
                    "unknown preset \"{name}\" (available: {})",
                    self.names().join(", ")
                ),
            });
        }
        self.apply(session, name, set_approval)?;
        Ok(shim::CommandResult {
            kind: "success",
            text: format!("preset {name}"),
        })
    }
}

fn derive_from(
    presets: &IndexMap<String, PresetSpec>,
    auto: Option<PresetSpec>,
    read_sandbox_mode: &ReadSandboxMode,
    read_approval_policy: &ReadApprovalPolicy,
    state: &KnobState,
) -> String {
    let sandbox = state.sandbox.or_else(|| read_sandbox_mode());
    let approval = state.approval.unwrap_or_else(|| read_approval_policy());
    let matches = |spec: &PresetSpec| Some(spec.sandbox) == sandbox && spec.approval == approval;
    if let Some(selected) = state.preset.as_deref() {
        let spec = presets
            .get(selected)
            .cloned()
            .or_else(|| (selected == AUTO_PRESET).then(|| auto.clone()).flatten());
        if spec.as_ref().is_some_and(&matches) {
            return selected.into();
        }
        if selected == AUTO_PRESET
            && spec
                .as_ref()
                .is_some_and(|value| Some(value.sandbox) == sandbox)
            && approval == ApprovalPolicy::Never
        {
            return AUTO_PRESET.into();
        }
    }
    presets
        .iter()
        .find_map(|(name, spec)| matches(spec).then(|| name.clone()))
        .unwrap_or_else(|| CUSTOM_PRESET.into())
}

fn apply_permission_event(
    state: &Arc<PermissionProjectionState>,
    event: &SessionEvent,
) -> Result<Arc<PermissionProjectionState>, BoxError> {
    let next = match event.event_type() {
        "permission/preset" => PermissionProjectionState {
            preset: Some(required_string(event.data(), "preset")?),
            ..(**state).clone()
        },
        "sandbox/mode" => PermissionProjectionState {
            sandbox: Some(serde_json::from_value(event.data()["mode"].clone())?),
            ..(**state).clone()
        },
        "approval/policy" => PermissionProjectionState {
            approval: Some(serde_json::from_value(event.data()["policy"].clone())?),
            ..(**state).clone()
        },
        "session/end-seed" => PermissionProjectionState {
            seeded: true,
            ..(**state).clone()
        },
        _ => return Ok(state.clone()),
    };
    Ok(Arc::new(next))
}

fn required_string(data: &Value, key: &str) -> Result<String, BoxError> {
    data.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            PermissionError::message(format!("permission: event field {key} must be a string"))
        })
}

#[cfg(test)]
fn fold_session(session: &Session) -> Result<PermissionProjectionState, BoxError> {
    let mut state = Arc::new(PermissionProjectionState::empty());
    for event in session.snapshot_events(None, None).iter() {
        state = apply_permission_event(&state, event)?;
    }
    Ok((*state).clone())
}

fn state_schema(raw: Option<&Value>) -> Result<Arc<PermissionProjectionState>, BoxError> {
    let raw =
        raw.ok_or_else(|| PermissionError::message("permission: projection state is missing"))?;
    let object = raw.as_object().ok_or_else(|| {
        PermissionError::message("permission: projection state must be an object")
    })?;
    for required in ["preset", "sandbox", "approval", "seeded"] {
        if !object.contains_key(required) {
            return Err(PermissionError::message(format!(
                "permission: projection state is missing {required}"
            )));
        }
    }
    Ok(Arc::new(serde_json::from_value(raw.clone())?))
}

fn permission_projection_definition(
    service: &PermissionPresetService,
) -> ProjectionDefinition<PermissionProjectionState> {
    let weak = Arc::downgrade(&service.inner);
    ProjectionDefinition {
        key: "permissions".into(),
        state_schema: Arc::new(state_schema),
        init: Arc::new(|_, _| Ok(Arc::new(PermissionProjectionState::empty()))),
        apply: Arc::new(apply_permission_event),
        wire: Some(ProjectionWire::new(
            Arc::new(|raw| {
                let value = raw.ok_or_else(|| {
                    PermissionError::message("permission: projection view is missing")
                })?;
                if value["currentValue"].as_str().is_none_or(str::is_empty) {
                    return Err(PermissionError::message(
                        "permission: projection currentValue must be a non-empty string",
                    ));
                }
                Ok(Arc::new(value.clone()))
            }),
            Arc::new(move |state| {
                let inner = weak.upgrade().ok_or_else(|| {
                    PermissionError::message("permission: presets service is unavailable")
                })?;
                let service = PermissionPresetService { inner };
                Ok(Some(Arc::new(json!({
                    "currentValue": service.derive(&state.knobs())
                }))))
            }),
        )),
        state_version: 2,
    }
}

/// Required services of the source plugin.
#[derive(Inject)]
pub struct PermissionPresetDeps {
    shell: Service<Shell>,
    approval: Service<Approval>,
    sessions: Service<Sessions>,
    agents: Service<Agents>,
    projections: Service<SessionProjections>,
}

impl Plugin for PermissionPresetService {
    const NAME: &'static str = "PermissionPresetService";
    type Config = Config;
    type Deps = PermissionPresetDeps;

    async fn apply(
        ctx: Context,
        deps: PermissionPresetDeps,
        config: Cfg<Config>,
    ) -> Result<(), BoxError> {
        // SHIM(hoh-settings): the optional `configure({ auto: false }, fiber)` child cannot be
        // connected until hoh-settings exposes a service/configuration API.
        let read_default_config = config.clone();
        let shell = deps.shell.clone();
        let approval = deps.approval.clone();
        let service = Self::build(
            config.get().presets.clone(),
            Arc::new(move || read_default_config.get().default_preset.0.clone()),
            Arc::new(move || shell.sandbox_mode()),
            Arc::new(move || approval.config().policy),
            StateSource::Pending,
        )?;

        {
            let service = service.clone();
            ctx.listen()
                .prepend()
                .on_serial::<AgentCreated, (), _>(move |event| {
                    let service = service.clone();
                    async move {
                        service.pin_initial_permission(event.agent.session())?;
                        Ok(None)
                    }
                })?;
        }

        let handle = deps
            .projections
            .register(&ctx, permission_projection_definition(&service))
            .await?;
        *lock(&service.inner.state_source) = StateSource::Projection(handle);

        for agent in deps.agents.list() {
            service.pin_initial_permission(agent.session())?;
        }

        // Keep `sessions` as a real required dependency, matching the source inject list.
        let _ = deps.sessions.list();
        ctx.provide::<PermissionPresets>(Arc::new(service)).await?;
        Ok(())
    }
}

#[cfg(test)]
impl PermissionPresetService {
    fn direct_for_tests(
        config: Config,
        sandbox: Option<SandboxMode>,
        approval: ApprovalPolicy,
        default: Arc<Mutex<Option<String>>>,
    ) -> Result<Self, BoxError> {
        Self::build(
            config.presets,
            Arc::new(move || lock(&default).clone()),
            Arc::new(move || sandbox),
            Arc::new(move || approval),
            StateSource::Direct,
        )
    }
}

#[cfg(test)]
#[path = "../tests/initialization.rs"]
mod initialization_tests;
#[cfg(test)]
#[path = "../tests/permission_presets.rs"]
mod permission_presets_tests;
#[cfg(test)]
#[path = "../tests/projection.rs"]
mod projection_tests;
