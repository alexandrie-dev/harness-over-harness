//! Browser-use provider identities.

hoh_brand::branded_string!(pub BrowserUseProviderName);

/// Brand a provider-owned name without changing or validating it.
pub fn browser_use_provider_name(name: impl Into<String>) -> BrowserUseProviderName {
    BrowserUseProviderName::from(name.into())
}
