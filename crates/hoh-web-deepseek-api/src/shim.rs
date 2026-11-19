//! Fixed interface for the not-yet-ported DeepSeek account model route.

use hoh_launch_environment::LaunchEnvironmentSnapshot;
use hoh_llm_deepseek::{Options, resolve_adapter_options_with_environment};
use serde_json::Value;

/// SHIM(hoh-llm-deepseek-account): resolve the protocol subset of the active account route;
/// replace when that crate provides `plain_account_options` and `resolve_account_options`.
pub(crate) fn resolve_account_base(
    raw: Option<&Value>,
    environment: &LaunchEnvironmentSnapshot,
) -> Result<String, String> {
    let value = raw
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let mut options: Options = serde_json::from_value(value).map_err(|error| error.to_string())?;
    if options.models.is_some() {
        return Err("llm-deepseek-account: models is no longer configurable; the account /models listing supplies the catalog. Remove models from this entry and adjust single models with modelOverrides.<model id>".into());
    }
    options.extra.remove("modelsTimeoutMs");
    options.extra.remove("modelsMaxAgeMs");
    options.extra.remove("modelOverrides");
    resolve_adapter_options_with_environment(&options, Some(environment))
        .map(|resolved| resolved.base_url)
        .map_err(|error| error.to_string())
}
