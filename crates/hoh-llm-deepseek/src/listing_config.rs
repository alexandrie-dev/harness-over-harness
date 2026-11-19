use hoh_timeout::MAX_TIMER_DELAY_MS;

pub const DEFAULT_MODELS_TIMEOUT_MS: f64 = 5_000.0;
pub const DEFAULT_MODELS_MAX_AGE_MS: u64 = 600_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelListingOptions {
    pub models_timeout_ms: Option<f64>,
    pub models_max_age_ms: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListingTiming {
    pub timeout_ms: f64,
    pub max_age_ms: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ListingSettings {
    pub url: String,
    pub timeout_ms: f64,
    pub max_age_ms: u64,
}

pub fn resolve_listing_timing(
    options: ModelListingOptions,
    owner: &str,
) -> Result<ListingTiming, String> {
    let timeout_ms = options
        .models_timeout_ms
        .unwrap_or(DEFAULT_MODELS_TIMEOUT_MS);
    if !timeout_ms.is_finite() || timeout_ms <= 0.0 || timeout_ms > MAX_TIMER_DELAY_MS as f64 {
        return Err(format!(
            "{owner}: modelsTimeoutMs must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
        ));
    }
    Ok(ListingTiming {
        timeout_ms,
        max_age_ms: options
            .models_max_age_ms
            .unwrap_or(DEFAULT_MODELS_MAX_AGE_MS),
    })
}
