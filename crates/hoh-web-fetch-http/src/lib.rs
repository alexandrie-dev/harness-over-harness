//! Anonymous HTTP(S) fetch provider for `web`.
pub mod policy;
pub mod provider;
pub mod transport_failure;
pub use provider::{HttpFetchLimits, HttpFetchProvider, LOCAL_FETCH_PROVIDER_ID};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_util_values::js_number_string;
use hoh_web::Web;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const DEFAULT_USER_AGENT: &str = "deepseek-harness/0.0.1 (+https://github.com/deepseek-ai)";

// Raw numeric config must reach the source's validation before becoming counters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("default"=serde_json::json!({})))]
pub struct Config {
    #[serde(default = "default_response_bytes")]
    pub max_response_bytes: f64,
    #[serde(default = "default_body_chars")]
    pub max_body_chars: f64,
    #[serde(default = "default_timeout")]
    pub timeout_ms: f64,
    #[serde(default = "default_redirects")]
    pub max_redirects: f64,
    #[serde(default = "default_agent")]
    pub user_agent: String,
}
fn default_response_bytes() -> f64 {
    5_000_000.0
}
fn default_body_chars() -> f64 {
    100_000.0
}
fn default_timeout() -> f64 {
    30_000.0
}
fn default_redirects() -> f64 {
    5.0
}
fn default_agent() -> String {
    DEFAULT_USER_AGENT.into()
}
impl Default for Config {
    fn default() -> Self {
        Self {
            max_response_bytes: default_response_bytes(),
            max_body_chars: default_body_chars(),
            timeout_ms: default_timeout(),
            max_redirects: default_redirects(),
            user_agent: default_agent(),
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Message(String),
}
impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (name, value) in [
            ("maxResponseBytes", self.max_response_bytes),
            ("maxBodyChars", self.max_body_chars),
            ("timeoutMs", self.timeout_ms),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(ConfigError::Message(format!(
                    "web-fetch-http: {name} must be a positive finite number"
                )));
            }
        }
        if self.timeout_ms > 2_147_483_647.0 {
            return Err(ConfigError::Message(
                "web-fetch-http: timeoutMs must be no greater than 2147483647".into(),
            ));
        }
        if !self.max_redirects.is_finite()
            || self.max_redirects.fract() != 0.0
            || self.max_redirects < 0.0
        {
            return Err(ConfigError::Message(
                "web-fetch-http: maxRedirects must be a non-negative integer".into(),
            ));
        }
        Ok(())
    }
    pub fn limits(&self) -> Result<HttpFetchLimits, ConfigError> {
        self.validate()?;
        // Source slice/subarray converts fractional capacities to integers. Positive values
        // beyond u64 are unobservable capacities for an in-memory response and saturate here.
        Ok(HttpFetchLimits {
            max_response_bytes: self.max_response_bytes.floor() as u64,
            max_body_chars: self.max_body_chars.floor() as u64,
            timeout_ms: self.timeout_ms,
            max_redirects: self.max_redirects as u64,
            user_agent: self.user_agent.clone(),
        })
    }
}

/// Shared transport composition supplies a fresh builder; the provider always disables redirects.
#[derive(Clone)]
pub struct HttpFetchClientBuilder(
    pub Arc<dyn Fn() -> Result<reqwest::ClientBuilder, BoxError> + Send + Sync>,
);

#[derive(Inject)]
pub struct Deps {
    pub web: Service<Web>,
}
pub struct WebFetchHttpPlugin;
impl Plugin for WebFetchHttpPlugin {
    const NAME: &'static str = "web-fetch-http";
    type Config = Config;
    type Deps = Deps;
    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        let limits = config.limits()?;
        let provider = match ctx.tag::<HttpFetchClientBuilder>() {
            Some(factory) => HttpFetchProvider::with_client_builder(limits, (factory.0)()?)?,
            None => HttpFetchProvider::new(limits)?,
        }
        .with_limit_diagnostics(
            js_number_string(config.max_response_bytes),
            js_number_string(config.max_redirects),
        );
        deps.web.register_fetch_provider(&ctx, Arc::new(provider))?;
        Ok(())
    }
}
