//! Owned transport and environment overlays; no process-wide mutable state.

use indexmap::IndexMap;
use reqwest::{Client, ClientBuilder, Proxy};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::policy::{
    EnvLookup, POLICY_ENV_NAMES, PROXY_ENV_NAMES, ProxyPolicy, ProxySource, is_supported_proxy_url,
    proxy_for_url, resolve_proxy_policy,
};

/// Apply these entries to a child environment: `None` removes a name.
pub type ProxyEnvironment = IndexMap<String, Option<String>>;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_h2: Option<bool>,
}

/// A captured route owns a clone of the same pooled client, so it outlives its installation.
#[derive(Clone, Debug)]
pub enum ProxyRoute {
    Proxied { proxy: String, dispatcher: Client },
    Direct,
}

/// The launcher owns this value and explicitly shares its client with HTTP consumers.
#[derive(Clone, Debug)]
pub struct ProxyInstallation {
    policy: ProxyPolicy,
    client: Client,
    inherited: Option<ProxyEnvironment>,
    environment: ProxyEnvironment,
}

/// Build a client whose single proxy predicate is also used by `proxy_route_for`.
/// Further request-specific transport settings can be applied before `.build()`.
pub fn route_client_builder(policy: ProxyPolicy, options: DispatcherOptions) -> ClientBuilder {
    // Disable reqwest's automatic environment proxy matcher, including for direct policies.
    let mut builder = Client::builder().no_proxy();
    if options.allow_h2 == Some(false) {
        builder = builder.http1_only();
    }
    builder.proxy(Proxy::custom(move |url| {
        proxy_for_url(&policy, url).map(str::to_owned)
    }))
}

fn snapshot_proxy_env() -> ProxyEnvironment {
    POLICY_ENV_NAMES
        .iter()
        .flat_map(|(_, names)| names)
        .map(|name| ((*name).to_owned(), std::env::var(name).ok()))
        .collect()
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
    ) -> Result<Self, reqwest::Error> {
        Self::resolve(env, || inherited, None, report, options)
    }

    fn resolve(
        env: &(impl EnvLookup + ?Sized),
        snapshot: impl FnOnce() -> ProxyEnvironment,
        outer_inherited: Option<ProxyEnvironment>,
        mut report: impl FnMut(&str),
        options: DispatcherOptions,
    ) -> Result<Self, reqwest::Error> {
        let resolution = resolve_proxy_policy(env);
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
        let client = route_client_builder(policy.clone(), options).build()?;
        Ok(Self {
            policy,
            client,
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
    ) -> Result<Self, reqwest::Error> {
        Self::resolve(
            env,
            snapshot_proxy_env,
            self.inherited.clone(),
            report,
            options,
        )
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn policy(&self) -> &ProxyPolicy {
        &self.policy
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
) -> Result<ProxyInstallation, reqwest::Error> {
    ProxyInstallation::resolve(env, snapshot_proxy_env, None, report, options)
}

/// Returns an owned pooled client. Drop it when its last request finishes.
pub fn create_route_dispatcher(
    env: &(impl EnvLookup + ?Sized),
    options: DispatcherOptions,
) -> Result<Client, reqwest::Error> {
    route_client_builder(resolve_proxy_policy(env).policy, options).build()
}

pub fn proxy_route_for(installation: Option<&ProxyInstallation>, url: &Url) -> ProxyRoute {
    let Some(installation) = installation else {
        return ProxyRoute::Direct;
    };
    match proxy_for_url(&installation.policy, url) {
        Some(proxy) => ProxyRoute::Proxied {
            proxy: proxy.to_owned(),
            dispatcher: installation.client.clone(),
        },
        None => ProxyRoute::Direct,
    }
}

pub fn proxy_environment_for_child(installation: Option<&ProxyInstallation>) -> ProxyEnvironment {
    let Some(installation) = installation else {
        return IndexMap::new();
    };
    if installation.policy.source == ProxySource::None {
        return IndexMap::new();
    }
    let Some(inherited) = &installation.inherited else {
        return IndexMap::new();
    };
    let mut overlay = IndexMap::from([("NODE_USE_ENV_PROXY".to_owned(), Some("1".to_owned()))]);
    let resolved = published_environment(&installation.policy);
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
