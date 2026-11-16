use hoh_credentials::{
    CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
    CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef, Credentials,
    ResolvedCredential,
};
use hoh_kernel::{Cfg, Context, Error, plugin};
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

pub struct MemoryCredentials {
    ctx: Context,
    store: Mutex<IndexMap<String, String>>,
    records: Mutex<IndexMap<CredentialKey, CredentialRecord>>,
}

#[plugin(MemoryCredentialsPlugin, name = "credentials")]
pub async fn apply(ctx: Context, config: Cfg<IndexMap<String, String>>) -> Result<(), Error> {
    let provider = MemoryCredentials {
        ctx: ctx.clone(),
        store: Mutex::new((*config.get()).clone()),
        records: Mutex::new(IndexMap::new()),
    };
    ctx.provide::<Credentials>(Arc::new(provider)).await?;
    Ok(())
}

impl CredentialProvider for MemoryCredentials {
    fn resolve<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
        let value = self
            .store
            .lock()
            .unwrap()
            .get(reference.as_str())
            .filter(|value| !value.is_empty())
            .cloned();
        Box::pin(async move {
            Ok(value.map(|value| ResolvedCredential {
                value,
                source: "memory".into(),
            }))
        })
    }

    fn describe<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, CredentialInfo> {
        let configured = self
            .store
            .lock()
            .unwrap()
            .get(reference.as_str())
            .is_some_and(|value| !value.is_empty());
        Box::pin(async move {
            Ok(CredentialInfo {
                configured,
                source: configured.then(|| "memory".into()),
                writable: true,
                masked: None,
            })
        })
    }

    fn set<'a>(&'a self, reference: &'a CredentialRef, value: &'a str) -> CredentialFuture<'a, ()> {
        if value.is_empty() {
            return Box::pin(async {
                Err("memory credentials: an empty value cannot be stored; use unset".into())
            });
        }
        self.store
            .lock()
            .unwrap()
            .insert(reference.as_str().into(), value.into());
        self.notify_updated(&self.ctx, reference);
        Box::pin(async { Ok(()) })
    }

    fn unset<'a>(&'a self, reference: &'a CredentialRef) -> CredentialFuture<'a, ()> {
        let removed = self
            .store
            .lock()
            .unwrap()
            .shift_remove(reference.as_str())
            .is_some();
        if removed {
            self.notify_updated(&self.ctx, reference);
        }
        Box::pin(async { Ok(()) })
    }

    fn read_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        let current = self.records.lock().unwrap().get(key).cloned();
        Box::pin(async move { Ok(current) })
    }

    fn describe_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo> {
        let kind = self
            .records
            .lock()
            .unwrap()
            .get(key)
            .map(CredentialRecord::kind);
        Box::pin(async move {
            Ok(CredentialRecordInfo {
                configured: kind.is_some(),
                kind,
                writable: true,
            })
        })
    }

    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
        let entries = self
            .records
            .lock()
            .unwrap()
            .iter()
            .map(|(key, record)| CredentialRecordEntry {
                key: key.clone(),
                kind: record.kind(),
            })
            .collect();
        Box::pin(async move { Ok(entries) })
    }

    fn modify_record<'a>(
        &'a self,
        key: &'a CredentialKey,
        mutate: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        let current = self.records.lock().unwrap().get(key).cloned();
        Box::pin(async move {
            let Some(next) = mutate(current.clone()).await? else {
                return Ok(current);
            };
            self.records
                .lock()
                .unwrap()
                .insert(key.clone(), next.clone());
            self.notify_record_updated(&self.ctx, key);
            Ok(Some(next))
        })
    }

    fn delete_record<'a>(&'a self, key: &'a CredentialKey) -> CredentialFuture<'a, ()> {
        let removed = self.records.lock().unwrap().shift_remove(key).is_some();
        if removed {
            self.notify_record_updated(&self.ctx, key);
        }
        Box::pin(async { Ok(()) })
    }
}
