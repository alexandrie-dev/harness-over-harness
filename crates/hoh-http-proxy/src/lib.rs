//! Port of @deepseek-ai/dsh-http-proxy (packages/util/http-proxy).
//! One outbound proxy policy and an explicitly shared reqwest client.
//!
//! Call [`install_proxy_from_environment`], then pass its [`ProxyInstallation::client`]
//! to every HTTP consumer. No process environment or global client is mutated.

pub mod install;
pub mod policy;

pub use install::{
    DispatcherOptions, ProxyEnvironment, ProxyInstallation, ProxyRoute, RouteDispatcher,
    cleared_proxy_env, create_route_dispatcher, install_proxy_from_environment,
    proxy_environment_for_child, proxy_route_for, route_client_builder,
};
