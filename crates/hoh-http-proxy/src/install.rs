//! Owned transport and environment overlays; no process-wide mutable state.

use indexmap::IndexMap;
use reqwest::{Client, ClientBuilder, Proxy};
use serde::{Deserialize, Serialize};
use std::{fmt, time::Duration};
use url::Url;

use crate::policy::{
    EnvLookup, POLICY_ENV_NAMES, PROXY_ENV_NAMES, ProxyDiagnostic, ProxyPolicy, ProxyResolution,
    ProxySource, is_supported_proxy_url, proxy_for_url, redacted_proxy_url,
    resolve_proxy_policy_with_validation,
};

/// Apply these entries to a child environment: `None` removes a name.
pub type ProxyEnvironment = IndexMap<String, Option<String>>;

#[derive(Debug, thiserror::Error)]
pub enum ProxyBuildError {
    #[error("{origin}: proxy URL cannot be used to build the HTTP client")]
    InvalidProxyUrl { origin: &'static str },
    #[error(transparent)]
    Client(#[from] reqwest::Error),
}

fn validated_proxy_url(value: &str, origin: &'static str) -> Result<Url, ProxyBuildError> {
    let invalid = || ProxyBuildError::InvalidProxyUrl { origin };
    let url = Url::parse(value).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid());
    }
    // Use reqwest's IntoProxy conversion, then every fallible URI step of
    // hyper-util 0.1.21 parse_env_uri, including its credential-stripped rebuild.
    Proxy::all(url.clone()).map_err(|_| invalid())?;
    let uri = url.as_str().parse::<http::Uri>().map_err(|_| invalid())?;
    let scheme = uri.scheme().ok_or_else(invalid)?;
    let authority = uri.authority().ok_or_else(invalid)?.as_str();
    let host_port = authority
        .split_once('@')
        .map_or(authority, |(_, host_port)| host_port);
    http::Uri::builder()
        .scheme(scheme.clone())
        .authority(host_port)
        .path_and_query("/")
        .build()
        .map_err(|_| invalid())?;
    Ok(url)
}

fn resolve_transport_policy(env: &(impl EnvLookup + ?Sized)) -> ProxyResolution {
    resolve_proxy_policy_with_validation(env, |url| {
        validated_proxy_url(url.as_str(), "proxy").is_ok()
    })
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_h2: Option<bool>,
}

impl DispatcherOptions {
    pub fn http1_only(self) -> bool {
        self.allow_h2 == Some(false)
    }
}

#[derive(Clone, Debug)]
struct RouteConfiguration {
    policy: ProxyPolicy,
    options: DispatcherOptions,
}

/// A pooled client together with the policy and transport choices it was built from.
#[derive(Clone, Debug)]
pub struct RouteDispatcher {
    client: Client,
    configuration: RouteConfiguration,
    diagnostics: Vec<ProxyDiagnostic>,
}

impl RouteDispatcher {
    fn build(
        configuration: RouteConfiguration,
        diagnostics: Vec<ProxyDiagnostic>,
    ) -> Result<Self, ProxyBuildError> {
        let client = configuration.builder()?.build()?;
        Ok(Self {
            client,
            configuration,
            diagnostics,
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Rejections from environment resolution; the caller decides how to report them.
    pub fn diagnostics(&self) -> &[ProxyDiagnostic] {
        &self.diagnostics
    }

    pub fn options(&self) -> DispatcherOptions {
        self.configuration.options
    }

    pub fn http1_only(&self) -> bool {
        self.options().http1_only()
    }

    pub fn route_for(&self, url: &Url) -> ProxyRoute {
        match proxy_for_url(&self.configuration.policy, url) {
            Some(proxy) => ProxyRoute::Proxied {
                proxy: proxy.to_owned(),
                dispatcher: self.clone(),
            },
            None => ProxyRoute::Direct {
                options: self.options(),
            },
        }
    }
}

impl std::ops::Deref for RouteDispatcher {
    type Target = Client;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

/// A captured route owns a clone of the same pooled client, so it outlives its installation.
#[derive(Clone)]
pub enum ProxyRoute {
    Proxied {
        proxy: String,
        dispatcher: RouteDispatcher,
    },
    Direct {
        options: DispatcherOptions,
    },
}

impl ProxyRoute {
    pub fn options(&self) -> DispatcherOptions {
        match self {
            Self::Proxied { dispatcher, .. } => dispatcher.options(),
            Self::Direct { options } => *options,
        }
    }

    pub fn http1_only(&self) -> bool {
        self.options().http1_only()
    }
}

/// The launcher owns this value and explicitly shares its client with HTTP consumers.
#[derive(Clone)]
pub struct ProxyInstallation {
    dispatcher: RouteDispatcher,
    inherited: Option<ProxyEnvironment>,
    environment: ProxyEnvironment,
}

/// Build a client whose single proxy predicate is also used by `proxy_route_for`.
/// Further request-specific transport settings can be applied before `.build()`.
pub fn route_client_builder(
    policy: ProxyPolicy,
    options: DispatcherOptions,
) -> Result<ClientBuilder, ProxyBuildError> {
    RouteConfiguration { policy, options }.builder()
}

impl RouteConfiguration {
    fn builder(&self) -> Result<ClientBuilder, ProxyBuildError> {
        let http = self
            .policy
            .http_proxy
            .as_deref()
            .map(|value| validated_proxy_url(value, "http_proxy"))
            .transpose()?;
        let https = self
            .policy
            .https_proxy
            .as_deref()
            .map(|value| validated_proxy_url(value, "https_proxy"))
            .transpose()?;
        // Disable reqwest's automatic environment proxy matcher, including for direct policies.
        let mut builder = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::limited(20))
            .referer(false);
        if self.options.http1_only() {
            builder = builder.http1_only();
        }
        let policy = self.policy.clone();
        Ok(builder.proxy(Proxy::custom(move |url| {
            proxy_for_url(&policy, url)?;
            // Only previously validated, immutable URLs reach reqwest's custom converter.
            match url.scheme() {
                "http" => http.clone(),
                "https" => https.clone(),
                _ => None,
            }
        })))
    }
}

fn snapshot_proxy_env() -> ProxyEnvironment {
    snapshot_proxy_env_from(|name| std::env::var_os(name))
}

fn snapshot_proxy_env_from(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> ProxyEnvironment {
    POLICY_ENV_NAMES
        .iter()
        .flat_map(|(_, names)| names)
        .map(|name| {
            (
                (*name).to_owned(),
                get(name).map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

struct RedactedEnvironment<'a>(&'a ProxyEnvironment);

impl fmt::Debug for RedactedEnvironment<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(
                self.0
                    .iter()
                    .map(|(name, value)| (name, value.as_deref().map(redacted_proxy_url))),
            )
            .finish()
    }
}

impl fmt::Debug for ProxyInstallation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyInstallation")
            .field("dispatcher", &self.dispatcher)
            .field(
                "inherited",
                &self.inherited.as_ref().map(RedactedEnvironment),
            )
            .field("environment", &RedactedEnvironment(&self.environment))
            .finish()
    }
}

impl fmt::Debug for ProxyRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proxied { proxy, dispatcher } => f
                .debug_struct("Proxied")
                .field("proxy", &redacted_proxy_url(proxy))
                .field("dispatcher", dispatcher)
                .finish(),
            Self::Direct { options } => f.debug_struct("Direct").field("options", options).finish(),
        }
    }
}

fn complete_snapshot(snapshot: ProxyEnvironment) -> ProxyEnvironment {
    POLICY_ENV_NAMES
        .iter()
        .flat_map(|(_, names)| names)
        .map(|name| ((*name).to_owned(), snapshot.get(*name).cloned().flatten()))
        .collect()
}

fn published_environment(policy: &ProxyPolicy) -> ProxyEnvironment {
    let values = [
        policy.http_proxy.as_ref(),
        policy.https_proxy.as_ref(),
        Some(&policy.no_proxy),
    ];
    POLICY_ENV_NAMES
        .iter()
        .zip(values)
        .flat_map(|((_, names), value)| {
            names
                .iter()
                .map(move |name| ((*name).to_owned(), value.cloned()))
        })
        .collect()
}

impl ProxyInstallation {
    /// Resolve using an explicit snapshot of the original process proxy names.
    /// This is useful when the launcher already owns an environment snapshot.
    pub fn with_inherited_environment(
        env: &(impl EnvLookup + ?Sized),
        inherited: ProxyEnvironment,
        report: impl FnMut(&str),
        options: DispatcherOptions,
    ) -> Result<Self, ProxyBuildError> {
        Self::resolve(env, || inherited, None, report, options)
    }

    fn resolve(
        env: &(impl EnvLookup + ?Sized),
        snapshot: impl FnOnce() -> ProxyEnvironment,
        outer_inherited: Option<ProxyEnvironment>,
        mut report: impl FnMut(&str),
        options: DispatcherOptions,
    ) -> Result<Self, ProxyBuildError> {
        let resolution = resolve_transport_policy(env);
        for diagnostic in &resolution.diagnostics {
            report(&diagnostic.message);
        }
        let policy = resolution.policy;
        let (inherited, environment) = if policy.source == ProxySource::None {
            // A direct layer restores the outermost user's values for child inheritance.
            let environment = outer_inherited.clone().unwrap_or_default();
            (outer_inherited, environment)
        } else {
            let environment = published_environment(&policy);
            (
                Some(outer_inherited.unwrap_or_else(|| complete_snapshot(snapshot()))),
                environment,
            )
        };
        let dispatcher = RouteDispatcher::build(
            RouteConfiguration { policy, options },
            resolution.diagnostics,
        )?;
        Ok(Self {
            dispatcher,
            inherited,
            environment,
        })
    }

    /// Create a replacement without changing this installation. Drop the replacement and
    /// resume passing this value to consumers to restore the outer policy.
    pub fn layer(
        &self,
        env: &(impl EnvLookup + ?Sized),
        report: impl FnMut(&str),
        options: DispatcherOptions,
    ) -> Result<Self, ProxyBuildError> {
        Self::resolve(
            env,
            snapshot_proxy_env,
            self.inherited.clone(),
            report,
            options,
        )
    }

    pub fn client(&self) -> &Client {
        self.dispatcher.client()
    }

    pub fn policy(&self) -> &ProxyPolicy {
        &self.dispatcher.configuration.policy
    }

    pub fn options(&self) -> DispatcherOptions {
        self.dispatcher.options()
    }

    pub fn http1_only(&self) -> bool {
        self.dispatcher.http1_only()
    }

    /// The six policy environment names this installation would publish.
    /// Apply this before the child-specific overlay when constructing a child's environment.
    pub fn environment(&self) -> &ProxyEnvironment {
        &self.environment
    }
}

pub fn install_proxy_from_environment(
    env: &(impl EnvLookup + ?Sized),
    report: impl FnMut(&str),
    options: DispatcherOptions,
) -> Result<ProxyInstallation, ProxyBuildError> {
    ProxyInstallation::resolve(env, snapshot_proxy_env, None, report, options)
}

/// Returns an owned pooled client. Drop it when its last request finishes.
pub fn create_route_dispatcher(
    env: &(impl EnvLookup + ?Sized),
    options: DispatcherOptions,
) -> Result<RouteDispatcher, ProxyBuildError> {
    let resolution = resolve_transport_policy(env);
    RouteDispatcher::build(
        RouteConfiguration {
            policy: resolution.policy,
            options,
        },
        resolution.diagnostics,
    )
}

pub fn proxy_route_for(installation: Option<&ProxyInstallation>, url: &Url) -> ProxyRoute {
    let Some(installation) = installation else {
        return ProxyRoute::Direct {
            options: DispatcherOptions::default(),
        };
    };
    installation.dispatcher.route_for(url)
}

pub fn proxy_environment_for_child(installation: Option<&ProxyInstallation>) -> ProxyEnvironment {
    let Some(installation) = installation else {
        return IndexMap::new();
    };
    if installation.policy().source == ProxySource::None {
        return IndexMap::new();
    }
    let Some(inherited) = &installation.inherited else {
        return IndexMap::new();
    };
    let mut overlay = IndexMap::from([("NODE_USE_ENV_PROXY".to_owned(), Some("1".to_owned()))]);
    let resolved = published_environment(installation.policy());
    for (field, names) in POLICY_ENV_NAMES {
        let named = field != "noProxy"
            && names
                .iter()
                .any(|name| inherited.get(*name).is_some_and(Option::is_some));
        for name in names {
            overlay.insert(
                name.to_owned(),
                if named {
                    inherited.get(name).cloned().flatten()
                } else {
                    resolved.get(name).cloned().flatten()
                },
            );
        }
    }
    if POLICY_ENV_NAMES[..2]
        .iter()
        .flat_map(|(_, names)| names)
        .any(|name| {
            overlay
                .get(*name)
                .and_then(Option::as_deref)
                .is_some_and(|value| !is_supported_proxy_url(value))
        })
    {
        overlay.shift_remove("NODE_USE_ENV_PROXY");
    }
    overlay
}

pub fn cleared_proxy_env() -> ProxyEnvironment {
    PROXY_ENV_NAMES
        .into_iter()
        .map(|name| (name.to_owned(), None))
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    #[test]
    fn non_unicode_environment_value_remains_named_and_reaches_child_lossily() {
        let snapshot = snapshot_proxy_env_from(|name| {
            (name == "HTTP_PROXY").then(|| OsString::from_vec(b"socks5://proxy.test/\xff".to_vec()))
        });
        assert_eq!(
            snapshot["HTTP_PROXY"].as_deref(),
            Some("socks5://proxy.test/\u{fffd}")
        );
        let installed = ProxyInstallation::with_inherited_environment(
            &|name: &str| (name == "ALL_PROXY").then(|| "http://proxy.test:3128".to_owned()),
            snapshot,
            |_| {},
            DispatcherOptions::default(),
        )
        .unwrap();
        let child = proxy_environment_for_child(Some(&installed));
        assert_eq!(
            child["HTTP_PROXY"].as_deref(),
            Some("socks5://proxy.test/\u{fffd}")
        );
        assert_eq!(child["http_proxy"], None);
        assert!(!child.contains_key("NODE_USE_ENV_PROXY"));
    }
}
