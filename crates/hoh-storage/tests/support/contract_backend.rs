//! Test-only memory fixture for exercising the shared runner and its assertions.
//! Reopening shares memory; this verifies test control flow, not disk durability.
use crate::contract::KvBackendContractHarness;
use hoh_error::BoxError;
use hoh_storage::{
    BackendFuture, KvFacet, KvSnapshot, KvUnit, KvUnitDescriptor, StorageBackend, StorageError,
    StorageErrorCode,
};
use indexmap::IndexMap;
use serde_json::Value;
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy)]
pub enum RejectionFault {
    None,
    Restamp,
    ChangeSnapshot,
}

#[derive(Clone)]
struct SavedUnit {
    version: u64,
    snapshot: KvSnapshot,
}
type Medium = Arc<Mutex<IndexMap<String, SavedUnit>>>;

struct Backend {
    medium: Medium,
    closed: Arc<AtomicBool>,
    open_names: Arc<Mutex<HashSet<String>>>,
    fault: RejectionFault,
}

fn backend(medium: Medium, fault: RejectionFault) -> Arc<dyn StorageBackend> {
    Arc::new(Backend {
        medium,
        closed: Arc::new(AtomicBool::new(false)),
        open_names: Arc::new(Mutex::new(HashSet::new())),
        fault,
    })
}

pub fn harness(fault: RejectionFault) -> KvBackendContractHarness {
    let medium = Arc::new(Mutex::new(IndexMap::new()));
    let initial = backend(medium.clone(), fault);
    KvBackendContractHarness {
        backend: initial,
        reopen: Box::new(move || {
            let reopened = backend(medium.clone(), fault);
            Box::pin(async move { Ok(reopened) })
        }),
    }
}

impl StorageBackend for Backend {
    fn kv(&self) -> Option<&dyn KvFacet> {
        Some(self)
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.closed.store(true, Ordering::Release);
            Ok(())
        })
    }
}

impl KvFacet for Backend {
    fn open(&self, descriptor: KvUnitDescriptor) -> BackendFuture<'_, Arc<dyn KvUnit>> {
        Box::pin(async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(StorageError::new(
                    StorageErrorCode::Closed,
                    "fixture backend is closed",
                )
                .into());
            }
            let stored = self.medium.lock().unwrap().get(&descriptor.name).cloned();
            if let Some(stored) = &stored
                && stored.version != descriptor.version
                && !descriptor
                    .compatible_versions
                    .as_ref()
                    .is_some_and(|versions| versions.contains(&stored.version))
            {
                return Err(StorageError::new(
                    StorageErrorCode::VersionMismatch,
                    "fixture version mismatch",
                )
                .into());
            }
            if !self
                .open_names
                .lock()
                .unwrap()
                .insert(descriptor.name.clone())
            {
                return Err(std::io::Error::other("fixture unit is already open").into());
            }
            let snapshot = stored.map_or_else(
                || KvSnapshot {
                    tables: descriptor
                        .tables
                        .iter()
                        .map(|name| (name.clone(), IndexMap::new()))
                        .collect(),
                    global: Value::Null,
                },
                |stored| stored.snapshot,
            );
            let unit: Arc<dyn KvUnit> = Arc::new(Unit {
                medium: self.medium.clone(),
                descriptor,
                snapshot: Mutex::new(snapshot),
                closed: AtomicBool::new(false),
                backend_closed: self.closed.clone(),
                open_names: self.open_names.clone(),
                fault: self.fault,
            });
            Ok(unit)
        })
    }
}

struct Unit {
    medium: Medium,
    descriptor: KvUnitDescriptor,
    snapshot: Mutex<KvSnapshot>,
    closed: AtomicBool,
    backend_closed: Arc<AtomicBool>,
    open_names: Arc<Mutex<HashSet<String>>>,
    fault: RejectionFault,
}

impl Unit {
    fn ensure_open(&self) -> Result<(), BoxError> {
        if self.closed.load(Ordering::Acquire) || self.backend_closed.load(Ordering::Acquire) {
            Err(StorageError::new(StorageErrorCode::Closed, "fixture unit is closed").into())
        } else {
            Ok(())
        }
    }
    fn commit(&self, snapshot: &KvSnapshot) {
        self.medium.lock().unwrap().insert(
            self.descriptor.name.clone(),
            SavedUnit {
                version: self.descriptor.version,
                snapshot: snapshot.clone(),
            },
        );
    }
}

impl KvUnit for Unit {
    fn load_all(&self) -> BackendFuture<'_, KvSnapshot> {
        Box::pin(async move {
            self.ensure_open()?;
            Ok(self.snapshot.lock().unwrap().clone())
        })
    }
    fn put_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
        value: Value,
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.ensure_open()?;
            let mut snapshot = self.snapshot.lock().unwrap();
            let Some(records) = snapshot.tables.get_mut(table) else {
                match self.fault {
                    RejectionFault::None => {}
                    RejectionFault::Restamp => {
                        if let Some(saved) =
                            self.medium.lock().unwrap().get_mut(&self.descriptor.name)
                        {
                            saved.version = self.descriptor.version;
                        }
                    }
                    RejectionFault::ChangeSnapshot => {
                        snapshot
                            .tables
                            .get_mut("alpha")
                            .unwrap()
                            .insert("broken".into(), value);
                    }
                }
                return Err(std::io::Error::other("fixture unit does not declare table").into());
            };
            records.insert(key.into(), value);
            self.commit(&snapshot);
            Ok(())
        })
    }
    fn delete_record<'a>(&'a self, table: &'a str, key: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(async move { self.delete_records(table, &[key.into()]).await })
    }
    fn delete_records<'a>(&'a self, table: &'a str, keys: &'a [String]) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.ensure_open()?;
            let mut snapshot = self.snapshot.lock().unwrap();
            let records = snapshot
                .tables
                .get_mut(table)
                .ok_or_else(|| std::io::Error::other("fixture unit does not declare table"))?;
            let mut changed = false;
            for key in keys {
                changed |= records.shift_remove(key).is_some();
            }
            if changed {
                self.commit(&snapshot);
            }
            Ok(())
        })
    }
    fn set_global(&self, value: Value) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.ensure_open()?;
            if !self.descriptor.has_global {
                return Err(std::io::Error::other("fixture unit has no global slot").into());
            }
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.global = value;
            self.commit(&snapshot);
            Ok(())
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            if !self.closed.swap(true, Ordering::AcqRel) {
                self.open_names
                    .lock()
                    .unwrap()
                    .remove(&self.descriptor.name);
            }
            Ok(())
        })
    }
}
