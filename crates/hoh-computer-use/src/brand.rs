//! Computer-use provider identities.

hoh_brand::branded_string!(pub ComputerUseProviderName);

/// Brand a provider-owned name without changing or validating it.
pub fn computer_use_provider_name(name: impl Into<String>) -> ComputerUseProviderName {
    ComputerUseProviderName::from(name.into())
}
