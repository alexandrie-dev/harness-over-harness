//! Pure helper from apps/cli/tests/profiles/sdk/tool-parity.ts; no SDK runtime required.
use indexmap::IndexSet;
#[derive(Clone, Copy)]
pub enum ToolMode {
    Standard,
    Ptc,
}
#[derive(Clone, Copy)]
pub enum Side {
    Sdk,
    Desktop,
}
impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::Sdk => "sdk",
            Self::Desktop => "desktop",
        }
    }
}
pub struct Exception<'a> {
    pub tool: &'a str,
    pub side: Side,
    pub reason: &'a str,
}
const STANDARD: &[Exception<'_>] = &[Exception {
    tool: "list_subagent_models",
    side: Side::Desktop,
    reason: "Desktop enables subagent model discovery; SDK delegation inherits the caller model by default.",
}];
const PTC: &[Exception<'_>] = &[
    Exception {
        tool: "list_subagent_models",
        side: Side::Desktop,
        reason: "Desktop enables subagent model discovery; SDK delegation inherits the caller model by default.",
    },
    Exception {
        tool: "plugin_manager",
        side: Side::Sdk,
        reason: "SDK PTC changes are persisted for restart without replacing active bindings because HMR is disabled; Desktop PTC excludes management in its HMR-enabled preset.",
    },
    Exception {
        tool: "workflow",
        side: Side::Sdk,
        reason: "The SDK PTC overlay changes only the executor so existing base workflow callers keep working; Desktop PTC uses run_code orchestration and omits the separate workflow surface.",
    },
];
fn sorted(values: impl IntoIterator<Item = String>) -> String {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    if values.is_empty() {
        "(none)".into()
    } else {
        values.join(", ")
    }
}
pub fn assert_tool_parity(
    mode: ToolMode,
    sdk: &[&str],
    desktop: &[&str],
    exceptions: Option<&[Exception<'_>]>,
) -> Result<(), String> {
    let (name, defaults) = match mode {
        ToolMode::Standard => ("standard", STANDARD),
        ToolMode::Ptc => ("ptc", PTC),
    };
    if sdk.is_empty() || desktop.is_empty() {
        return Err(format!("Empty tool inventory: {name}"));
    }
    let mut sdk_only = sdk
        .iter()
        .copied()
        .filter(|n| !desktop.contains(n))
        .collect::<IndexSet<_>>();
    let mut desktop_only = desktop
        .iter()
        .copied()
        .filter(|n| !sdk.contains(n))
        .collect::<IndexSet<_>>();
    let mut stale = vec![];
    for exception in exceptions.unwrap_or(defaults) {
        if hoh_util_values::js_trim(exception.reason).is_empty() {
            return Err(format!(
                "Tool exception requires a reason: {}",
                exception.tool
            ));
        }
        let only = match exception.side {
            Side::Sdk => &mut sdk_only,
            Side::Desktop => &mut desktop_only,
        };
        if !only.shift_remove(exception.tool) {
            stale.push(format!("{}:{}", exception.side.name(), exception.tool));
        }
    }
    if sdk_only.is_empty() && desktop_only.is_empty() && stale.is_empty() {
        return Ok(());
    }
    Err(format!(
        "SDK/Desktop tool mismatch: {name}\nMissing from SDK: {}\nOnly in SDK: {}\nStale exceptions: {}",
        sorted(desktop_only.into_iter().map(str::to_owned)),
        sorted(sdk_only.into_iter().map(str::to_owned)),
        sorted(stale)
    ))
}
