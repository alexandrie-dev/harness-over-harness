//! Credential resolution shared by DeepSeek providers.

use futures_util::future::BoxFuture;
use hoh_credentials::{CredentialProvider, CredentialRef, Credentials};
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_launch_environment::LaunchEnvironmentSnapshot;

// SHIM(hoh-deepseek-account): optional account-token service declared by the
// later packages/credentials/deepseek-account port; replace this key and trait
// with that crate's public API when it exists.
pub trait DeepSeekAccountApi: Send + Sync + 'static {
    fn resolve_token<'a>(
        &'a self,
        endpoint: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, BoxError>>;
    fn reject_token<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
}

hoh_kernel::service!(pub DeepSeekAccount: dyn DeepSeekAccountApi = "deepseekAccount");

pub async fn resolve_deep_seek_api_key_from_sources(
    credentials: Option<&dyn CredentialProvider>,
    environment: &LaunchEnvironmentSnapshot,
    reference: &CredentialRef,
) -> Result<Option<String>, BoxError> {
    if let Some(credentials) = credentials {
        let hit = credentials.resolve(reference).await?;
        return hit
            .map(|hit| {
                hoh_llm::assert_usable_api_key(&hit.value, "llm-deepseek", reference.as_str())
            })
            .transpose()
            .map_err(Into::into);
    }
    let Some(value) = environment
        .get(reference.as_str())
        .map(|entry| entry.value)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(hoh_llm::assert_usable_api_key(
        &value,
        "llm-deepseek",
        reference.as_str(),
    )?))
}

pub async fn deep_seek_api_key_configured_from_sources(
    credentials: Option<&dyn CredentialProvider>,
    environment: &LaunchEnvironmentSnapshot,
    reference: &CredentialRef,
) -> Result<bool, BoxError> {
    match credentials {
        Some(credentials) => Ok(credentials.describe(reference).await?.configured),
        None => Ok(environment
            .get(reference.as_str())
            .is_some_and(|entry| !entry.value.is_empty())),
    }
}

pub async fn resolve_deep_seek_api_key(
    ctx: &Context,
    reference: &CredentialRef,
) -> Result<Option<String>, BoxError> {
    let credentials = ctx.get::<Credentials>();
    let environment = hoh_launch_environment::launch_environment_of(ctx);
    resolve_deep_seek_api_key_from_sources(
        credentials
            .as_ref()
            .map(|service| &**service as &dyn CredentialProvider),
        &environment,
        reference,
    )
    .await
}

pub async fn deep_seek_api_key_configured(
    ctx: &Context,
    reference: &CredentialRef,
) -> Result<bool, BoxError> {
    let credentials = ctx.get::<Credentials>();
    let environment = hoh_launch_environment::launch_environment_of(ctx);
    deep_seek_api_key_configured_from_sources(
        credentials
            .as_ref()
            .map(|service| &**service as &dyn CredentialProvider),
        &environment,
        reference,
    )
    .await
}

pub async fn resolve_deep_seek_account_token(
    ctx: &Context,
    endpoint: &str,
) -> Result<Option<String>, BoxError> {
    match ctx.get::<DeepSeekAccount>() {
        Some(account) => account.resolve_token(endpoint).await,
        None => Ok(None),
    }
}

pub async fn reject_deep_seek_account_token(ctx: &Context, token: &str) {
    if let Some(account) = ctx.get::<DeepSeekAccount>() {
        let _ = account.reject_token(token).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;
    use std::sync::{Arc, Mutex};

    struct Account {
        token: Option<String>,
        rejected: Arc<Mutex<Vec<String>>>,
        reject_fails: bool,
    }

    impl DeepSeekAccountApi for Account {
        fn resolve_token<'a>(
            &'a self,
            endpoint: &'a str,
        ) -> BoxFuture<'a, Result<Option<String>, BoxError>> {
            Box::pin(async move {
                if endpoint == "https://allowed.test" {
                    Ok(self.token.clone())
                } else {
                    Ok(None)
                }
            })
        }

        fn reject_token<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
            Box::pin(async move {
                self.rejected.lock().unwrap().push(token.into());
                if self.reject_fails {
                    Err(Box::new(std::io::Error::other("storage offline")) as BoxError)
                } else {
                    Ok(())
                }
            })
        }
    }

    #[tokio::test]
    async fn account_token_is_optional_and_destination_scoped() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        assert_eq!(
            resolve_deep_seek_account_token(&ctx, "https://allowed.test")
                .await
                .unwrap(),
            None
        );
        ctx.provide::<DeepSeekAccount>(Arc::new(Account {
            token: Some("account-token".into()),
            rejected: Arc::new(Mutex::new(vec![])),
            reject_fails: false,
        }))
        .await
        .unwrap();
        assert_eq!(
            resolve_deep_seek_account_token(&ctx, "https://allowed.test")
                .await
                .unwrap()
                .as_deref(),
            Some("account-token")
        );
        assert_eq!(
            resolve_deep_seek_account_token(&ctx, "https://other.test")
                .await
                .unwrap(),
            None
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn account_rejection_contains_storage_failures() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let rejected = Arc::new(Mutex::new(vec![]));
        ctx.provide::<DeepSeekAccount>(Arc::new(Account {
            token: Some("account-token".into()),
            rejected: rejected.clone(),
            reject_fails: true,
        }))
        .await
        .unwrap();
        reject_deep_seek_account_token(&ctx, "account-token").await;
        assert_eq!(*rejected.lock().unwrap(), ["account-token"]);
        kernel.shutdown().await;
    }
}
