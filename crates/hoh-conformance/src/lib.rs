//! Replays recorded DeepSeek Harness scenarios through the built `hoh` binary.
pub mod body;
pub mod compare;
pub mod corpus;
pub mod fixtures;
pub mod manifest;
pub mod normalize;
pub mod protocol;
pub mod runner;
pub mod v1;
pub mod workspace;
pub mod yaml;

pub type Result<T> = std::result::Result<T, String>;

pub fn read(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn records(text: &str) -> Result<Vec<serde_json::Value>> {
    text.lines()
        .filter(|s| !hoh_util_values::js_trim(s).is_empty())
        .map(|s| hoh_util_values::js_json_parse(s).map_err(|e| e.to_string()))
        .collect()
}

pub fn jsonl(rows: &[serde_json::Value]) -> String {
    rows.iter()
        .map(|v| format!("{}\n", hoh_util_values::js_json_stringify(v)))
        .collect()
}
