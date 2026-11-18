use crate::error::ValueError;
use indexmap::IndexMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppIdentity<'a> {
    pub product: &'a str,
    pub version: &'a str,
    pub url: &'a str,
    pub suffix: Option<&'a str>,
}
pub static APP_IDENTITY: AppIdentity<'static> = AppIdentity {
    product: "deepseek-harness",
    version: env!("CARGO_PKG_VERSION"),
    url: "https://github.com/deepseek-ai/deepseek-harness",
    suffix: None,
};
pub fn launch_suffix_value(value: Option<&str>) -> Result<Option<&str>, ValueError> {
    let Some(value) = value.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&b))
    };
    let mut parts = value.split('/');
    if !token(parts.next().unwrap_or_default())
        || parts.next().is_some_and(|s| !token(s))
        || parts.next().is_some()
    {
        return Err(ValueError::Message(format!(
            "DSH_USER_AGENT_SUFFIX must be an RFC 9110 product token; got {}",
            hoh_util_values::js_json_stringify(&serde_json::Value::String(value.into()))
        )));
    }
    Ok(Some(value))
}
pub fn user_agent(identity: Option<&AppIdentity<'_>>) -> Result<String, ValueError> {
    let suffix = if identity.is_none_or(|identity| std::ptr::eq(identity, &APP_IDENTITY)) {
        std::env::var("DSH_USER_AGENT_SUFFIX").ok()
    } else {
        None
    };
    let launch = launch_suffix_value(suffix.as_deref())?;
    let identity = identity.unwrap_or(&APP_IDENTITY);
    let base = format!(
        "{}/{} (+{})",
        identity.product, identity.version, identity.url
    );
    Ok(match identity.suffix.or(launch) {
        Some(s) => format!("{base} {s}"),
        None => base,
    })
}
pub fn attribution_headers(
    identity: Option<&AppIdentity<'_>>,
) -> Result<IndexMap<String, String>, ValueError> {
    Ok(IndexMap::from_iter([(
        "user-agent".into(),
        user_agent(identity)?,
    )]))
}
