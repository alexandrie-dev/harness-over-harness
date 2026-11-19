//! Runtime plugin trees shared by agents selecting one preset revision.

use crate::{
    RegistryError,
    definition::{PresetDefinition, entry_list_problem},
    shim,
};
use feruca::{Collator, Locale, Tailoring};
use hoh_kernel::{Context, Fiber, FiberState, Realm, Service, ServiceKey};
use hoh_loader::{EntryInfo, EntryStatus, Group};
use hoh_scope::{ScopeKey, scope_of};
use std::collections::HashSet;

#[derive(Clone)]
pub struct PresetTree {
    group: Group,
    ctx: Context,
}

impl PresetTree {
    pub fn entries(&self) -> Vec<EntryInfo> {
        self.group.entries()
    }
}

#[derive(Clone)]
pub struct PresetMount {
    pub preset_id: String,
    pub fiber: Fiber,
    pub tree: PresetTree,
    pub key: Option<ScopeKey>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowAudit {
    pub failed: Vec<String>,
    pub pending: Vec<String>,
}

fn descendants(snapshot: &hoh_kernel::Snapshot, root: u32) -> HashSet<u32> {
    let mut found = HashSet::from([root]);
    loop {
        let before = found.len();
        for fiber in &snapshot.fibers {
            if found.contains(&fiber.parent) {
                found.insert(fiber.uid);
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

pub async fn leaked_services(ctx: &Context, mount: &Fiber) -> Vec<String> {
    let snapshot = ctx.reflect().store().await;
    let owners = descendants(&snapshot, mount.uid());
    let mut leaked: Vec<_> = snapshot
        .provisions
        .into_iter()
        .filter(|provision| provision.realm == Realm::ROOT && owners.contains(&provision.owner))
        .map(|provision| provision.name.to_string())
        .collect();
    sort_unique_services(&mut leaked);
    leaked
}

fn sort_unique_services(leaked: &mut Vec<String>) {
    let mut collator = Collator::new(Tailoring::Cldr(Locale::Root), false, false);
    leaked.sort_by(|left, right| collator.collate(left, right));
    leaked.dedup();
}

pub async fn audit_rows(tree: &PresetTree) -> RowAudit {
    let entries = tree.entries();
    for entry in &entries {
        if !entry.disabled
            && let Some(fiber) = &entry.fiber
        {
            let _ = fiber.settled().await;
        }
    }
    let snapshot = tree.ctx.reflect().store().await;
    let mut audit = RowAudit::default();
    for entry in entries {
        if entry.disabled || entry.options.group {
            continue;
        }
        let label = format!("{} ({})", entry.options.id, entry.options.name);
        let Some(fiber) = entry.fiber else {
            if entry.status == EntryStatus::Unresolved {
                audit.failed.push(format!("{label}: never started"));
            }
            continue;
        };
        if let Err(error) = fiber.settled().await {
            audit.failed.push(format!(
                "{label}: {}",
                hoh_loader::startup_diagnostics::format_activation_error(&error)
            ));
            continue;
        }
        let missing = snapshot
            .fibers
            .iter()
            .find(|info| info.uid == fiber.uid())
            .map(|info| &info.missing)
            .filter(|missing| !missing.is_empty());
        if let Some(missing) = missing {
            audit.pending.push(format!(
                "{label}: waiting for {}",
                missing
                    .iter()
                    .map(|name| name.as_ref())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        } else if fiber.state() == FiberState::Failed {
            audit.failed.push(format!("{label}: activation failed"));
        }
    }
    audit
}

pub async fn mount_preset_with_runtime(
    ctx: &Context,
    id: &str,
    definition: &PresetDefinition,
    runtime: &shim::PresetRuntime,
) -> Result<PresetMount, RegistryError> {
    if scope_of(ctx).is_none() {
        return Err(RegistryError(
            "agent-preset: mounting requires a scope".into(),
        ));
    }
    ctx.fiber()
        .settled()
        .await
        .map_err(|error| RegistryError(error.to_string()))?;
    if let Some(problem) = entry_list_problem(&definition.plugins) {
        return Err(RegistryError(problem));
    }
    let rows = shim::decode_entries(&definition.plugins)?;
    let group = Group::mount(ctx, runtime.registry.clone(), runtime.env.clone())
        .map_err(|error| RegistryError(error.to_string()))?;
    group.update(rows).await;
    let tree = PresetTree {
        group,
        ctx: ctx.clone(),
    };
    let audit = audit_rows(&tree).await;
    let leaked = leaked_services(ctx, &ctx.fiber()).await;
    if !audit.failed.is_empty() {
        return Err(RegistryError(audit.failed.join("\n")));
    }
    if !leaked.is_empty() {
        return Err(RegistryError(format!(
            "Preset services require isolate realms: {}",
            leaked.join(", ")
        )));
    }
    Ok(PresetMount {
        preset_id: id.to_owned(),
        fiber: ctx.fiber(),
        tree,
        key: scope_of(ctx),
    })
}

/// Mount with the fallback Loader runtime. Registry internals use
/// [`mount_preset_with_runtime`] with their configured runtime.
pub async fn mount_preset(
    ctx: &Context,
    id: &str,
    definition: &PresetDefinition,
) -> Result<PresetMount, RegistryError> {
    mount_preset_with_runtime(ctx, id, definition, &shim::PresetRuntime::fallback()).await
}

/// Best-effort typed counterpart of the source's dynamic `serviceForMount`.
///
/// SHIM(hoh-kernel): isolated provision values are not exposed by reflection,
/// so this can return only a value visible from `ctx` and owned by the mount.
pub async fn service_for_mount<K: ServiceKey>(
    ctx: &Context,
    mount: &PresetMount,
) -> Option<Service<K>> {
    let value = ctx.get::<K>()?;
    let snapshot = ctx.reflect().store().await;
    let owners = descendants(&snapshot, mount.fiber.uid());
    snapshot
        .provisions
        .iter()
        .any(|provision| {
            provision.name.as_ref() == K::NAME
                && provision.realm == ctx.realm_of(K::NAME)
                && owners.contains(&provision.owner)
        })
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::sort_unique_services;

    #[test]
    fn leaked_service_names_are_sorted_before_duplicates_are_removed() {
        let mut names = vec![
            "presetZulu".into(),
            "presetAlpha".into(),
            "presetZulu".into(),
        ];
        sort_unique_services(&mut names);
        assert_eq!(names, ["presetAlpha", "presetZulu"]);
    }
}
