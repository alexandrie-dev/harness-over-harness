//! Exclusive named registration for the browser-use capability.

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, NoConfig, Plugin, Undo};
use std::sync::{Arc, Mutex, MutexGuard};

pub mod brand;
pub mod types;

use brand::BrowserUseProviderName;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug)]
struct Registration {
    name: BrowserUseProviderName,
}

/// Owns one optional provider registration in the shared browser-use service.
#[derive(Clone, Default)]
pub struct BrowserUseRegistry {
    registration: Arc<Mutex<Option<Arc<Registration>>>>,
}

hoh_kernel::service!(pub BrowserUse: BrowserUseRegistry = "browserUse");

impl Plugin for BrowserUseRegistry {
    const NAME: &'static str = "browser-use";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(
        ctx: Context,
        _deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        ctx.provide::<BrowserUse>(Arc::new(Self::default())).await?;
        Ok(())
    }
}

impl BrowserUseRegistry {
    /// Name of the registered provider, including while its resources are closing.
    pub fn provider_name(&self) -> Option<BrowserUseProviderName> {
        lock(&self.registration)
            .as_ref()
            .map(|registration| registration.name.clone())
    }

    /// Reserve the sole provider slot until the contribution is disposed.
    pub fn register(
        &self,
        ctx: &Context,
        name: BrowserUseProviderName,
    ) -> Result<Disposer, BoxError> {
        if let Some(current) = lock(&self.registration).as_ref() {
            return Err(format!(
                "browser use provider \"{}\" is already registered",
                current.name
            )
            .into());
        }

        let registration = Arc::new(Registration { name });
        let state = self.registration.clone();
        ctx.try_effect("browserUse.register()", move |undos| {
            let undo_state = state.clone();
            let undo_registration = registration.clone();
            undos.push(Undo::sync(move || {
                let mut current = lock(&undo_state);
                if current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &undo_registration))
                {
                    *current = None;
                }
            }))?;

            {
                let mut current = lock(&state);
                if let Some(current) = current.as_ref() {
                    return Err(format!(
                        "browser use provider \"{}\" is already registered",
                        current.name
                    )
                    .into());
                }
                *current = Some(registration.clone());
            }
            Ok(())
        })
    }
}
