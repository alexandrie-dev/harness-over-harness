//! Configuration resolution for deterministic tool-result pruning (`src/config.ts`).

use crate::ToolResultPruneError;
use crate::types::{ResolvedConfig, ToolResultPruneConfig};
use hoh_util_values::{js_number_string, js_string};
use serde_json::Value;

/// Fixed marker substituted for every removed middle span.
pub const PRUNE_MARKER: &str = "\n\n[... tool result middle pruned ...]\n\n";

/// Low-friction defaults for coding-agent tool output.
pub const DEFAULTS: ResolvedConfig = ResolvedConfig {
    threshold_chars: 8192,
    head_chars: 4096,
    tail_chars: 1024,
};

const CONFIG_KEYS: [&str; 3] = ["thresholdChars", "headChars", "tailChars"];

/// Count Unicode code points without splitting surrogate pairs.
pub fn code_point_length(text: &str) -> u64 {
    text.chars().count() as u64
}

/// JavaScript `Object.keys` order: integer-like keys ascending, then insertion order.
fn js_key_order(config: &ToolResultPruneConfig) -> Vec<&str> {
    let mut indexes: Vec<(u32, &str)> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    for key in config.0.keys() {
        match key.parse::<u32>() {
            Ok(index) if index != u32::MAX && index.to_string() == *key => {
                indexes.push((index, key));
            }
            _ => rest.push(key),
        }
    }
    indexes.sort_by_key(|(index, _)| *index);
    indexes
        .into_iter()
        .map(|(_, key)| key)
        .chain(rest)
        .collect()
}

/// Resolve and validate pruning budgets.
pub fn resolve_config(
    config: &ToolResultPruneConfig,
) -> Result<ResolvedConfig, ToolResultPruneError> {
    for key in js_key_order(config) {
        if !CONFIG_KEYS.contains(&key) {
            return Err(ToolResultPruneError::new(format!(
                "ToolResultPruneConfig: unknown key \"{key}\" \
                 (allowed: thresholdChars, headChars, tailChars)"
            )));
        }
    }

    let threshold = integer(config, "thresholdChars", DEFAULTS.threshold_chars, true)?;
    let head = integer(config, "headChars", DEFAULTS.head_chars, false)?;
    let tail = integer(config, "tailChars", DEFAULTS.tail_chars, false)?;

    let emitted = head + code_point_length(PRUNE_MARKER) as f64 + tail;
    if emitted > threshold {
        return Err(ToolResultPruneError::new(format!(
            "ToolResultPruneConfig: headChars + marker + tailChars ({}) \
             must be at most thresholdChars ({})",
            js_number_string(emitted),
            js_number_string(threshold)
        )));
    }
    Ok(ResolvedConfig {
        threshold_chars: threshold as u64,
        head_chars: head as u64,
        tail_chars: tail as u64,
    })
}

/// `config.<name> ?? default`, then `assertPositiveInteger` / `assertNonNegativeInteger`.
fn integer(
    config: &ToolResultPruneConfig,
    name: &str,
    default: u64,
    positive: bool,
) -> Result<f64, ToolResultPruneError> {
    let value = match config.get(name) {
        None => return Ok(default as f64),
        Some(value) => value,
    };
    let number = match value {
        Value::Number(number) => number.as_f64(),
        _ => None,
    };
    match number {
        Some(number)
            if number.is_finite()
                && number.fract() == 0.0
                && if positive {
                    number > 0.0
                } else {
                    number >= 0.0
                } =>
        {
            Ok(number)
        }
        _ => Err(ToolResultPruneError::new(format!(
            "ToolResultPruneConfig: {name} ({}) must be a {} integer",
            js_string(value),
            if positive { "positive" } else { "non-negative" }
        ))),
    }
}
