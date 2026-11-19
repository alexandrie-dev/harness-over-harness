use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_INSTRUCTION_BYTES: usize = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconnectConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub initial_delay_ms: Option<u64>,
    #[serde(default)]
    pub max_delay_ms: Option<u64>,
    #[serde(default)]
    pub max_attempts: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedReconnectPolicy {
    pub enabled: bool,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_attempts: u64,
}

pub const RECONNECT_DEFAULTS: ResolvedReconnectPolicy = ResolvedReconnectPolicy {
    enabled: true,
    initial_delay_ms: 500,
    max_delay_ms: 30_000,
    max_attempts: 10,
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ReconnectConfigError(pub String);

pub fn resolve_reconnect_policy(
    config: Option<&ReconnectConfig>,
    path: &str,
) -> Result<ResolvedReconnectPolicy, ReconnectConfigError> {
    let policy = ResolvedReconnectPolicy {
        enabled: config
            .and_then(|value| value.enabled)
            .unwrap_or(RECONNECT_DEFAULTS.enabled),
        initial_delay_ms: config
            .and_then(|value| value.initial_delay_ms)
            .unwrap_or(RECONNECT_DEFAULTS.initial_delay_ms),
        max_delay_ms: config
            .and_then(|value| value.max_delay_ms)
            .unwrap_or(RECONNECT_DEFAULTS.max_delay_ms),
        max_attempts: config
            .and_then(|value| value.max_attempts)
            .unwrap_or(RECONNECT_DEFAULTS.max_attempts),
    };
    let max = hoh_timeout::MAX_TIMER_DELAY_MS;
    if policy.initial_delay_ms == 0 || policy.initial_delay_ms > max {
        return Err(ReconnectConfigError(format!(
            "{path}.initialDelayMs must be a positive finite number no greater than {max}"
        )));
    }
    if policy.max_delay_ms == 0 || policy.max_delay_ms > max {
        return Err(ReconnectConfigError(format!(
            "{path}.maxDelayMs must be a positive finite number no greater than {max}"
        )));
    }
    if policy.initial_delay_ms > policy.max_delay_ms {
        return Err(ReconnectConfigError(format!(
            "{path}.initialDelayMs must be less than or equal to maxDelayMs"
        )));
    }
    if policy.max_attempts == 0 {
        return Err(ReconnectConfigError(format!(
            "{path}.maxAttempts must be a positive integer"
        )));
    }
    Ok(policy)
}

pub fn reconnect_delay(policy: ResolvedReconnectPolicy, failed_attempts: u64) -> u64 {
    let shift = failed_attempts.saturating_sub(1).min(63) as u32;
    policy
        .initial_delay_ms
        .saturating_mul(1_u64 << shift)
        .min(policy.max_delay_ms)
}
