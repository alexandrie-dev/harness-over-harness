//! Exclusive named registration for the computer-use capability.

use std::sync::{Arc, Mutex, MutexGuard};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, NoConfig, Plugin, Undo};

pub mod brand;
pub mod types;

use brand::ComputerUseProviderName;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug)]
struct Registration {
    name: ComputerUseProviderName,
}

/// Owns one optional provider registration in the shared computer-use service.
#[derive(Clone, Default)]
pub struct ComputerUseRegistry {
    registration: Arc<Mutex<Option<Arc<Registration>>>>,
}

hoh_kernel::service!(pub ComputerUse: ComputerUseRegistry = "computerUse");

impl Plugin for ComputerUseRegistry {
    const NAME: &'static str = "computer-use";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(
        ctx: Context,
        _deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        ctx.provide::<ComputerUse>(Arc::new(Self::default()))
            .await?;
        Ok(())
    }
}

impl ComputerUseRegistry {
    /// Name of the registered provider, including while its resources are closing.
    pub fn provider_name(&self) -> Option<ComputerUseProviderName> {
        lock(&self.registration)
            .as_ref()
            .map(|registration| registration.name.clone())
    }

    /// Reserve the sole provider slot until the contribution is disposed.
    pub fn register(
        &self,
        ctx: &Context,
        name: ComputerUseProviderName,
    ) -> Result<Disposer, BoxError> {
        if let Some(current) = lock(&self.registration).as_ref() {
            return Err(format!(
                "computer use provider \"{}\" is already registered",
                current.name
            )
            .into());
        }

        let registration = Arc::new(Registration { name });
        let state = self.registration.clone();
        ctx.try_effect("computerUse.register()", move |undos| {
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
                        "computer use provider \"{}\" is already registered",
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
