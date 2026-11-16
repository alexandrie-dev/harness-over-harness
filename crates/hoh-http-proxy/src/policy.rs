//! Transport-independent environment resolution and bypass matching.

use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt};
use url::Url;

/// Lookup in the launcher's already layered environment snapshot.
pub trait EnvLookup {
    fn get(&self, name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> EnvLookup for F {
    fn get(&self, name: &str) -> Option<String> {
        self(name)
    }
}

impl EnvLookup for indexmap::IndexMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

impl EnvLookup for std::collections::HashMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

pub const LOOPBACK_NO_PROXY: [&str; 4] = ["localhost", "127.0.0.1", "::1", "[::1]"];
pub const POLICY_ENV_NAMES: [(&str, [&str; 2]); 3] = [
    ("httpProxy", ["http_proxy", "HTTP_PROXY"]),
    ("httpsProxy", ["https_proxy", "HTTPS_PROXY"]),
    ("noProxy", ["no_proxy", "NO_PROXY"]),
];
pub const PROXY_ENV_NAMES: [&str; 8] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "no_proxy",
    "NO_PROXY",
    "all_proxy",
    "ALL_PROXY",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxySource {
    Env,
    None,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub https_proxy: Option<String>,
    pub no_proxy: String,
    pub source: ProxySource,
}

// Preserve the spelling, and also redact malformed inherited values that URL cannot parse.
pub(crate) fn redacted_proxy_url(value: &str) -> Cow<'_, str> {
    if let Ok(mut url) = Url::parse(value)
        && url.password().is_some()
        && url.set_password(Some("***")).is_ok()
    {
        return Cow::Owned(url.into());
    }
    // An inherited value may be malformed (including an unescaped slash in its password).
    // In that case redact through the final @ rather than trusting authority boundaries.
    let start = value.find("://").map_or(0, |i| i + 3);
    let Some(at) = value[start..].rfind('@') else {
        return Cow::Borrowed(value);
    };
    let Some(colon) = value[start..start + at].find(':') else {
        return Cow::Borrowed(value);
    };
    Cow::Owned(format!(
        "{}***{}",
        &value[..start + colon + 1],
        &value[start + at..]
    ))
}

impl fmt::Debug for ProxyPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyPolicy")
            .field(
                "http_proxy",
                &self.http_proxy.as_deref().map(redacted_proxy_url),
            )
            .field(
                "https_proxy",
                &self.https_proxy.as_deref().map(redacted_proxy_url),
            )
            .field("no_proxy", &redacted_proxy_url(&self.no_proxy))
            .field("source", &self.source)
            .finish()
    }
}

pub const DIRECT_POLICY: ProxyPolicy = ProxyPolicy {
    http_proxy: None,
    https_proxy: None,
    no_proxy: String::new(),
    source: ProxySource::None,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyDiagnosticKind {
    Socks,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProxyDiagnostic {
    pub kind: ProxyDiagnosticKind,
    pub origin: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProxyResolution {
    pub policy: ProxyPolicy,
    pub diagnostics: Vec<ProxyDiagnostic>,
}

// ECMAScript WhiteSpace + LineTerminator, used by trim() and /\s/ in the source.
fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn entries(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|c| c == ',' || js_whitespace(c))
        .filter(|s| !s.is_empty())
}

fn read_env(env: &(impl EnvLookup + ?Sized), lower: &str) -> Option<(String, String)> {
    for name in [lower.to_owned(), lower.to_uppercase()] {
        if let Some(value) = env.get(&name) {
            let value = value.trim_matches(js_whitespace);
            if !value.is_empty() {
                return Some((value.to_owned(), name));
            }
        }
    }
    None
}

enum ProxyCandidate {
    Accepted(String),
    Rejected,
    Absent,
}

fn accept_proxy_url(
    candidate: Option<(String, String)>,
    diagnostics: &mut Vec<ProxyDiagnostic>,
) -> ProxyCandidate {
    let Some((value, origin)) = candidate else {
        return ProxyCandidate::Absent;
    };
    let (kind, message) = match Url::parse(&value) {
        Err(_) => (
            ProxyDiagnosticKind::Invalid,
            format!("{origin} is not a valid URL; connecting directly"),
        ),
        Ok(parsed) => match parsed.scheme() {
            "http" | "https" => return ProxyCandidate::Accepted(value),
            "socks" | "socks4" | "socks4a" | "socks5" | "socks5h" => (
                ProxyDiagnosticKind::Socks,
                format!(
                    "{origin} names a SOCKS proxy, which is not supported; connecting directly for that scheme — set an http:// or https:// proxy URL instead"
                ),
            ),
            scheme => (
                ProxyDiagnosticKind::Invalid,
                format!(
                    "{origin} uses the unsupported {scheme}:// scheme; connecting directly for that scheme — set an http:// or https:// proxy URL instead"
                ),
            ),
        },
    };
    diagnostics.push(ProxyDiagnostic {
        kind,
        origin,
        message,
    });
    ProxyCandidate::Rejected
}

pub fn is_supported_proxy_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn resolve_scheme(own: ProxyCandidate, fallbacks: &[Option<&str>]) -> Option<String> {
    match own {
        ProxyCandidate::Accepted(value) => Some(value),
        ProxyCandidate::Rejected => None,
        ProxyCandidate::Absent => fallbacks.iter().flatten().next().map(|s| (*s).to_owned()),
    }
}

fn with_loopback(no_proxy: &str) -> String {
    let mut result: Vec<_> = entries(no_proxy).map(str::to_owned).collect();
    if result.iter().any(|entry| entry == "*") {
        return "*".to_owned();
    }
    let present: Vec<_> = result.iter().map(|entry| entry.to_lowercase()).collect();
    result.extend(
        LOOPBACK_NO_PROXY
            .into_iter()
            .filter(|entry| !present.iter().any(|s| s == entry))
            .map(str::to_owned),
    );
    result.join(",")
}

fn split_host_port(entry: &str) -> (&str, Option<&str>) {
    if let Some(bracketed) = entry.strip_prefix('[')
        && let Some((host, rest)) = bracketed.split_once(']')
    {
        return (host, rest.strip_prefix(':'));
    }
    if let Some((host, port)) = entry.split_once(':')
        && !port.contains(':')
    {
        return (host, Some(port));
    }
    (entry, None)
}

fn normalized_host(host: &str) -> String {
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    host.strip_suffix('.').unwrap_or(host).to_lowercase()
}

pub fn is_loopback_host(hostname: &str) -> bool {
    let host = normalized_host(hostname);
    if host == "localhost"
        || host.ends_with(".localhost")
        || matches!(host.as_str(), "::1" | "::" | "0.0.0.0")
    {
        return true;
    }
    let ipv4 = host.strip_prefix("::ffff:").unwrap_or(&host);
    if host.starts_with("::ffff:")
        && let Some((high, low)) = ipv4.split_once(':')
        && [high, low]
            .iter()
            .all(|s| !s.is_empty() && s.len() <= 4 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        && let Ok(high) = u16::from_str_radix(high, 16)
    {
        return high >> 8 == 127;
    }
    let octets: Vec<_> = ipv4.split('.').collect();
    octets.len() == 4
        && octets[0] == "127"
        && octets[1..].iter().all(|s| {
            !s.is_empty()
                && (s.len() == 1 || !s.starts_with('0'))
                && s.bytes().all(|c| c.is_ascii_digit())
                && s.parse::<u8>().is_ok()
        })
}

pub fn bypasses_proxy(no_proxy: &str, url: &Url) -> bool {
    let host = normalized_host(url.host_str().unwrap_or(""));
    let port = url
        .port()
        .map(|p| p.to_string())
        .unwrap_or_else(|| if url.scheme() == "https" { "443" } else { "80" }.to_owned());
    for raw in entries(no_proxy) {
        let entry = raw.to_lowercase();
        if entry == "*" {
            return true;
        }
        let (candidate, entry_port) = split_host_port(&entry);
        if entry_port.is_some_and(|p| p != port) {
            continue;
        }
        let candidate = candidate
            .strip_prefix("*.")
            .or_else(|| candidate.strip_prefix('.'))
            .unwrap_or(candidate);
        let candidate = candidate.strip_suffix('.').unwrap_or(candidate);
        if !candidate.is_empty() && (host == candidate || host.ends_with(&format!(".{candidate}")))
        {
            return true;
        }
    }
    false
}

pub fn resolve_proxy_policy(env: &(impl EnvLookup + ?Sized)) -> ProxyResolution {
    let mut diagnostics = Vec::new();
    let all = accept_proxy_url(read_env(env, "all_proxy"), &mut diagnostics);
    let all_value = match &all {
        ProxyCandidate::Accepted(value) => Some(value.as_str()),
        _ => None,
    };
    let env_http = accept_proxy_url(read_env(env, "http_proxy"), &mut diagnostics);
    let env_https = accept_proxy_url(read_env(env, "https_proxy"), &mut diagnostics);
    let http_proxy = resolve_scheme(env_http, &[all_value]);
    let https_proxy = resolve_scheme(env_https, &[all_value, http_proxy.as_deref()]);
    let policy = if http_proxy.is_none() && https_proxy.is_none() {
        DIRECT_POLICY
    } else {
        ProxyPolicy {
            http_proxy,
            https_proxy,
            no_proxy: with_loopback(
                &read_env(env, "no_proxy")
                    .map(|(value, _)| value)
                    .unwrap_or_default(),
            ),
            source: ProxySource::Env,
        }
    };
    ProxyResolution {
        policy,
        diagnostics,
    }
}

pub fn proxy_for_url<'a>(policy: &'a ProxyPolicy, url: &Url) -> Option<&'a str> {
    let proxy = match url.scheme() {
        "https" => policy.https_proxy.as_deref(),
        "http" => policy.http_proxy.as_deref(),
        _ => None,
    }?;
    if is_loopback_host(url.host_str().unwrap_or("")) || bypasses_proxy(&policy.no_proxy, url) {
        None
    } else {
        Some(proxy)
    }
}
