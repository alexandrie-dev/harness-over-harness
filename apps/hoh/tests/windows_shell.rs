mod support;
use hoh_loader::{Entry, Expr, PluginRegistry, compose};
use serde_json::Value;
fn disabled(
    value: &Value,
    platform: &'static str,
    ctx: &hoh_kernel::Context,
    root: &std::path::Path,
) -> bool {
    if let Some(expression) = value.get("__jsExpr").and_then(Value::as_str) {
        let mut env = support::env(root);
        env.platform = platform;
        Expr::parse(expression)
            .unwrap()
            .eval(&env, ctx, &PluginRegistry::default(), root)
            .unwrap()
            == Some(Value::Bool(true))
    } else {
        *value == true
    }
}
fn row<'a>(rows: &'a [Entry], id: &str) -> &'a Entry {
    rows.iter().find(|r| r.id == id).unwrap()
}
#[tokio::test(start_paused = true)]
async fn composes_the_confined_pwsh_roster_on_win32_and_the_bash_roster_on_posix_from_the_same_rows()
 {
    let root = tempfile::tempdir().unwrap();
    let k = hoh_kernel::Kernel::start();
    let rows = compose(&[], &support::layers(&["base", "web-app"]), &mut |w| {
        panic!("{w}")
    })
    .entries;
    for (id, win) in [("bash-sandbox", true), ("pwsh-sandbox", false)] {
        assert_eq!(
            disabled(&row(&rows, id).disabled, "win32", &k.root(), root.path()),
            win
        );
        assert_eq!(
            disabled(&row(&rows, id).disabled, "linux", &k.root(), root.path()),
            !win
        );
    }
    for id in ["tool-bash", "tool-pwsh"] {
        assert_eq!(row(&rows, id).disabled, true);
    }
    for id in [
        "permission",
        "ui-permission",
        "sandbox",
        "sandbox-policy",
        "fs-sandbox",
        "approval",
    ] {
        assert_ne!(row(&rows, id).disabled, true);
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn base_only_profiles_carry_both_stacks_with_the_same_platform_gating() {
    let root = tempfile::tempdir().unwrap();
    let k = hoh_kernel::Kernel::start();
    let rows = compose(&[], &support::layers(&["base"]), &mut |w| panic!("{w}")).entries;
    for (id, win) in [
        ("bash-sandbox", true),
        ("tool-bash", true),
        ("pwsh-sandbox", false),
        ("tool-pwsh", false),
    ] {
        assert_eq!(
            disabled(&row(&rows, id).disabled, "win32", &k.root(), root.path()),
            win
        );
        assert_eq!(
            disabled(&row(&rows, id).disabled, "linux", &k.root(), root.path()),
            !win
        );
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn presets_gate_their_shell_tool_rows_by_platform() {
    let root = tempfile::tempdir().unwrap();
    let k = hoh_kernel::Kernel::start();
    let rows = compose(&[], &support::layers(&["web-app"]), &mut |_| {}).entries;
    for preset in ["standard", "ptc", "cordis"] {
        let definition = rows
            .iter()
            .find(|r| r.name == "@deepseek-ai/dsh-agent-preset" && r.config["id"] == preset)
            .unwrap();
        let entries = definition.config["plugins"].as_array().unwrap();
        for (id, win) in [("tool-bash", true), ("tool-pwsh", false)] {
            let r = entries.iter().find(|r| r["id"] == id).unwrap();
            assert!(r["disabled"]["__jsExpr"].is_string());
            assert_eq!(
                disabled(&r["disabled"], "win32", &k.root(), root.path()),
                win
            );
            assert_eq!(
                disabled(&r["disabled"], "linux", &k.root(), root.path()),
                !win
            );
        }
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn minimal_mounts_no_shell_tool_row_and_gates_its_persistent_shell_stack_by_platform() {
    let root = tempfile::tempdir().unwrap();
    let k = hoh_kernel::Kernel::start();
    let rows = compose(&[], &support::layers(&["web-app"]), &mut |_| {}).entries;
    let definition = rows
        .iter()
        .find(|r| r.name == "@deepseek-ai/dsh-agent-preset" && r.config["id"] == "minimal")
        .unwrap();
    let entries = definition.config["plugins"].as_array().unwrap();
    for id in ["tool-bash", "tool-pwsh"] {
        assert!(!entries.iter().any(|r| r["id"] == id));
    }
    let rows = entries
        .iter()
        .find(|r| r["id"] == "persistent-shell")
        .unwrap()["config"]
        .as_array()
        .unwrap();
    for (id, win) in [
        ("terminal-bash", true),
        ("persistent-bash", true),
        ("terminal-pwsh", false),
        ("persistent-pwsh", false),
    ] {
        let r = rows.iter().find(|r| r["id"] == id).unwrap();
        assert_eq!(
            disabled(&r["disabled"], "win32", &k.root(), root.path()),
            win
        );
        assert_eq!(
            disabled(&r["disabled"], "linux", &k.root(), root.path()),
            !win
        );
    }
    assert_eq!(
        rows.iter().find(|r| r["id"] == "terminal-pwsh").unwrap()["config"]["shellDialect"],
        "pwsh"
    );
    k.shutdown().await;
}
