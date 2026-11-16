//! Port of @deepseek-ai/dsh-launch-environment (packages/util/launch-environment).
//! Immutable launch-time environment values, resolved in canonical trust order.

use std::sync::Arc;

use hoh_kernel::Context;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// The layer that supplied an environment value, most trusted first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchEnvironmentSource {
    Process,
    ProjectEnv,
    UserEnv,
}

const SOURCE_ORDER: [LaunchEnvironmentSource; 3] = [
    LaunchEnvironmentSource::Process,
    LaunchEnvironmentSource::ProjectEnv,
    LaunchEnvironmentSource::UserEnv,
];

/// One resolved variable and its provenance. Empty strings remain present.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchEnvironmentEntry {
    pub value: String,
    pub source: LaunchEnvironmentSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Raw contents of one layer. Paths are retained exactly as supplied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchEnvironmentLayerInput {
    pub source: LaunchEnvironmentSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub values: IndexMap<String, String>,
}

#[derive(Debug)]
struct Layer {
    path: Option<String>,
    values: IndexMap<String, String>,
}

/// Frozen values for one launch; clones share the same immutable storage.
#[derive(Clone, Debug)]
pub struct LaunchEnvironmentSnapshot {
    layers: Arc<IndexMap<LaunchEnvironmentSource, Layer>>,
}

impl LaunchEnvironmentSnapshot {
    /// Resolve across every layer, most trusted first.
    pub fn get(&self, name: &str) -> Option<LaunchEnvironmentEntry> {
        self.get_from(name, &SOURCE_ORDER)
    }

    /// Resolve only from allowed layers, retaining canonical trust order.
    pub fn get_from(
        &self,
        name: &str,
        sources: &[LaunchEnvironmentSource],
    ) -> Option<LaunchEnvironmentEntry> {
        let key = lookup_key(name);
        for source in SOURCE_ORDER {
            if !sources.contains(&source) {
                continue;
            }
            if let Some(layer) = self.layers.get(&source)
                && let Some(value) = layer.values.get(&key)
            {
                return Some(LaunchEnvironmentEntry {
                    value: value.clone(),
                    source,
                    path: layer.path.clone(),
                });
            }
        }
        None
    }
}

fn lookup_key(name: &str) -> String {
    #[cfg(windows)]
    {
        name.to_uppercase()
    }
    #[cfg(not(windows))]
    {
        name.to_owned()
    }
}

/// Copy all inputs. A later layer with the same source replaces the earlier one.
pub fn create_launch_environment_snapshot(
    layers: &[LaunchEnvironmentLayerInput],
) -> LaunchEnvironmentSnapshot {
    let mut by_source = IndexMap::new();
    for layer in layers {
        by_source.insert(
            layer.source,
            Layer {
                path: layer.path.clone(),
                values: layer
                    .values
                    .iter()
                    .map(|(name, value)| (lookup_key(name), value.clone()))
                    .collect(),
            },
        );
    }
    LaunchEnvironmentSnapshot {
        layers: Arc::new(by_source),
    }
}

/// Context slot filled by the launcher before mounting configuration entries.
pub const DSH_LAUNCH_ENVIRONMENT_KEY: &str = "launchEnvironment";

/// Typed key for the launcher's optional snapshot.
pub struct LaunchEnvironment;

impl hoh_kernel::ServiceKey for LaunchEnvironment {
    type Value = LaunchEnvironmentSnapshot;
    const NAME: &'static str = DSH_LAUNCH_ENVIRONMENT_KEY;
}

/// Return the launcher's snapshot, otherwise snapshot only the inherited process.
pub fn launch_environment_of(ctx: &Context) -> LaunchEnvironmentSnapshot {
    if let Some(environment) = ctx.get::<LaunchEnvironment>() {
        return (*environment).clone();
    }
    create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::Process,
        path: None,
        // Node exposes environment names and values as strings. Decode native
        // non-Unicode values lossily instead of panicking as std::env::vars does.
        values: std::env::vars_os()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect(),
    }])
}

/// Only non-empty inherited SSH markers establish an SSH session.
pub fn launched_through_ssh(environment: &LaunchEnvironmentSnapshot) -> bool {
    ["SSH_CONNECTION", "SSH_TTY"].iter().any(|name| {
        environment
            .get_from(name, &[LaunchEnvironmentSource::Process])
            .is_some_and(|entry| !entry.value.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn launcher_snapshot_shares_immutable_storage() {
        let kernel = hoh_kernel::Kernel::start();
        let snapshot = create_launch_environment_snapshot(&[]);
        kernel
            .root()
            .provide::<LaunchEnvironment>(Arc::new(snapshot.clone()))
            .await
            .unwrap();
        let resolved = launch_environment_of(&kernel.root());
        assert!(Arc::ptr_eq(&snapshot.layers, &resolved.layers));
        kernel.shutdown().await;
    }
}
