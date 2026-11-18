use hoh_kernel::{Plugin, PluginDef};
use indexmap::{IndexMap, IndexSet};
use std::path::PathBuf;

/// Explicit module table. Builtins require no package resolution.
pub struct PluginRegistry {
    plugins: IndexMap<String, PluginDef>,
    aliases: IndexMap<String, String>,
    carriers: IndexSet<String>,
    resources: IndexMap<String, PathBuf>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        let mut registry = Self {
            plugins: IndexMap::new(),
            aliases: IndexMap::new(),
            carriers: IndexSet::new(),
            resources: IndexMap::new(),
        };
        registry
            .add::<crate::runtime::GroupPlugin>("cordis:group")
            .add::<crate::runtime::IncludePlugin>("cordis:include")
            .alias("@deepseek-ai/cordis-plugin-group", "cordis:group")
            .alias("@deepseek-ai/cordis-plugin-include", "cordis:include")
            .carrier("cordis:group")
            .carrier("cordis:include");
        registry
    }
}

impl PluginRegistry {
    pub fn add<P: Plugin>(&mut self, name: &str) -> &mut Self {
        self.plugins.insert(name.into(), PluginDef::of::<P>());
        self
    }
    pub fn alias(&mut self, alias: &str, name: &str) -> &mut Self {
        self.aliases.insert(alias.into(), name.into());
        self
    }
    pub fn carrier(&mut self, name: &str) -> &mut Self {
        self.carriers.insert(name.into());
        self
    }
    pub fn resource_dir(&mut self, name: &str, dir: PathBuf) -> &mut Self {
        self.resources.insert(name.into(), dir);
        self
    }
    pub(crate) fn canonical<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        let mut current = name;
        let mut seen = IndexSet::new();
        while !self.plugins.contains_key(current) {
            if !seen.insert(current) {
                return None;
            }
            let Some(next) = self.aliases.get(current) else {
                return Some(current);
            };
            current = next;
        }
        Some(current)
    }
    pub fn resolve(&self, name: &str) -> Option<&PluginDef> {
        self.plugins.get(self.canonical(name)?)
    }
    pub(crate) fn is_carrier(&self, name: &str) -> bool {
        if self.carriers.contains(name) {
            return true;
        }
        let Some(canonical) = self.canonical(name) else {
            return false;
        };
        self.carriers
            .iter()
            .any(|marked| self.canonical(marked) == Some(canonical))
    }
    pub(crate) fn resource(&self, name: &str) -> Option<PathBuf> {
        self.resources
            .get(name)
            .or_else(|| self.canonical(name).and_then(|n| self.resources.get(n)))
            .cloned()
    }
}
