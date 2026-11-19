// SHIM(hoh-credentials-local): process-local provider for the replay binary;
// replace with accepted durable provider when engine integration is delivered.
use hoh_credentials::*;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::Mutex,
    task::{Context, Poll, Waker},
};
#[derive(Default)]
pub(crate) struct StubCredentials {
    values: Mutex<BTreeMap<String, String>>,
    records: Mutex<BTreeMap<String, CredentialRecord>>,
}
impl CredentialProvider for StubCredentials {
    fn resolve<'a>(
        &'a self,
        r: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
        let value = self.values.lock().unwrap().get(r.as_str()).cloned();
        Box::pin(async move {
            Ok(value.map(|value| ResolvedCredential {
                value,
                source: "stub-memory".into(),
            }))
        })
    }
    fn describe<'a>(&'a self, r: &'a CredentialRef) -> CredentialFuture<'a, CredentialInfo> {
        let configured = self.values.lock().unwrap().contains_key(r.as_str());
        Box::pin(async move {
            Ok(CredentialInfo {
                configured,
                source: configured.then(|| "stub-memory".into()),
                writable: true,
                masked: None,
            })
        })
    }
    fn set<'a>(&'a self, r: &'a CredentialRef, v: &'a str) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            if v.is_empty() {
                return Err("empty model credential".into());
            }
            self.values
                .lock()
                .unwrap()
                .insert(r.as_str().into(), v.into());
            Ok(())
        })
    }
    fn unset<'a>(&'a self, r: &'a CredentialRef) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            self.values.lock().unwrap().remove(r.as_str());
            Ok(())
        })
    }
    fn read_record<'a>(
        &'a self,
        k: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async move { Ok(self.records.lock().unwrap().get(k.as_str()).cloned()) })
    }
    fn describe_record<'a>(
        &'a self,
        k: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo> {
        Box::pin(async move {
            let kind = self
                .records
                .lock()
                .unwrap()
                .get(k.as_str())
                .map(CredentialRecord::kind);
            Ok(CredentialRecordInfo {
                configured: kind.is_some(),
                kind,
                writable: true,
            })
        })
    }
    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
        Box::pin(async move {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| CredentialRecordEntry {
                    key: parse_credential_key(k).expect("validated key"),
                    kind: v.kind(),
                })
                .collect())
        })
    }
    fn modify_record<'a>(
        &'a self,
        k: &'a CredentialKey,
        f: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async move {
            let current = self.records.lock().unwrap().get(k.as_str()).cloned();
            if let Some(next) = f(current.clone()).await? {
                self.records
                    .lock()
                    .unwrap()
                    .insert(k.as_str().into(), next.clone());
                Ok(Some(next))
            } else {
                Ok(current)
            }
        })
    }
    fn delete_record<'a>(&'a self, k: &'a CredentialKey) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            self.records.lock().unwrap().remove(k.as_str());
            Ok(())
        })
    }
}
fn immediate<F: Future>(future: F) -> F::Output {
    let mut pinned = std::pin::pin!(future);
    match pinned
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("stub credentials provider must complete immediately"),
    }
}
impl StubCredentials {
    pub fn store_model_key(&self, key: &str) -> Result<(), String> {
        let r = credential_ref("model_api_key").unwrap();
        immediate(self.set(&r, key)).map_err(|_| "cannot store model credential".into())
    }
    /// Mock probe consumes the same resolved credential production probes will use.
    pub fn probe_authenticated(&self) -> Result<bool, String> {
        let r = credential_ref("model_api_key").unwrap();
        immediate(self.resolve(&r))
            .map(|c| c.is_some_and(|v| !v.value.is_empty()))
            .map_err(|_| "cannot resolve model credential".into())
    }
}
