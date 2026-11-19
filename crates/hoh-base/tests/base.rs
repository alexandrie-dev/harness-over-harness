use hoh_base::CORDIS_PATCH;
use hoh_loader::{Entry, parse_patches};
use serde_json::json;
use std::path::Path;

fn rows() -> Vec<Entry> {
    let patches = parse_patches(CORDIS_PATCH, Path::new("/base/cordis.patch.yml")).unwrap();
    assert_eq!(patches.len(), 1, "base must remain one patch layer");
    patches
        .into_iter()
        .flat_map(|patch| patch.insert.unwrap_or_default())
        .collect()
}

fn row<'a>(rows: &'a [Entry], id: &str) -> &'a Entry {
    rows.iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("missing base row {id}"))
}

#[test]
fn declares_a_parseable_single_insert_layer_with_the_source_core_rows() {
    let rows = rows();
    assert!(rows.len() > 50);
    assert_eq!(row(&rows, "agent-loop").name, "@deepseek-ai/dsh-agent-loop");
    assert_eq!(row(&rows, "hmr").config, json!({"root": []}));
    assert_eq!(
        row(&rows, "session-telemetry-otel").config.pointer("/mode"),
        Some(&json!({
            "__jsExpr": "process.env.DSH_TELEMETRY_MODE || 'FEEDBACK_ONLY'"
        }))
    );
    assert!(row(&rows, "session-telemetry-otel").disabled.is_null());
    assert!(!rows.iter().any(|row| row.id == "subagent-codex"));
    assert!(!rows.iter().any(|row| row.id == "subagent-claude-code"));

    assert_eq!(
        row(&rows, "web").config,
        json!({"searchProvider": "deepseek-search-api", "fetchProvider": "http"})
    );
    assert_eq!(
        row(&rows, "web-deepseek-api").name,
        "@deepseek-ai/dsh-web-deepseek-api"
    );
    assert_eq!(
        row(&rows, "web-fetch-http").name,
        "@deepseek-ai/dsh-web-fetch-http"
    );
    assert_eq!(
        row(&rows, "web-search-deepseek").config,
        json!({"apiKeyEnv": "DEEPSEEK_API_KEY"})
    );
    assert_eq!(row(&rows, "tool-web").config["fetch"], true);
}

#[test]
fn gates_the_posix_shell_stack_and_defers_the_windows_twins_to_stage_five() {
    let rows = rows();
    assert_eq!(
        row(&rows, "bash-sandbox").disabled,
        json!({"__jsExpr": "process.platform === 'win32'"})
    );
    assert_eq!(
        row(&rows, "tool-bash").disabled,
        json!({"__jsExpr": "process.platform === 'win32'"})
    );
    assert!(!rows.iter().any(|row| row.id == "pwsh-sandbox"));
    assert!(!rows.iter().any(|row| row.id == "tool-pwsh"));
}
