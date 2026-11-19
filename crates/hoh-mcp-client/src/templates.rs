use crate::diagnostic::{McpLocalError, McpLocalErrorCode};
use hoh_error::BoxError;
use std::{future::Future, pin::Pin};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct TemplateSyntaxError(pub String);

pub fn credential_refs_in(value: &str) -> Vec<String> {
    tokens(value)
        .filter_map(|token| token.ok().map(|(_, name, _)| name.to_owned()))
        .collect()
}

pub fn assert_template_syntax(value: &str, where_: &str) -> Result<(), TemplateSyntaxError> {
    for token in tokens(value) {
        if let Err(occurrence) = token {
            return Err(TemplateSyntaxError(format!(
                "{where_}: \"{occurrence}\" is not a template token — use ${{credential:REF}}"
            )));
        }
    }
    Ok(())
}

pub type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<String>, BoxError>> + Send + 'a>>;

pub async fn resolve_template_string(
    value: &str,
    where_: &str,
    mut credential: impl for<'a> FnMut(&'a str) -> ResolveFuture<'a>,
) -> Result<String, BoxError> {
    assert_template_syntax(value, where_).map_err(|error| Box::new(error) as BoxError)?;
    let mut out = String::new();
    let mut cursor = 0;
    for token in tokens(value) {
        let (start, name, end) = token.expect("syntax was checked");
        let Some(resolved) = credential(name).await? else {
            return Err(Box::new(McpLocalError::new(
                format!(
                    "{where_}: credential \"{name}\" is not configured — store it through the credential seam or remove the ${{credential:{name}}} template"
                ),
                McpLocalErrorCode::CredentialNotFound,
            )));
        };
        out.push_str(&value[cursor..start]);
        out.push_str(&resolved);
        cursor = end;
    }
    out.push_str(&value[cursor..]);
    Ok(out)
}

fn valid_ref(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn tokens(value: &str) -> impl Iterator<Item = Result<(usize, &str, usize), &str>> {
    let mut cursor = 0;
    std::iter::from_fn(move || {
        let start = cursor + value[cursor..].find("${")?;
        let end = value[start..].find('}').map(|n| start + n + 1)?;
        cursor = end.max(start + 2);
        let occurrence = &value[start..end];
        let name = occurrence
            .strip_prefix("${credential:")
            .and_then(|s| s.strip_suffix('}'));
        Some(match name {
            Some(name) if valid_ref(name) => Ok((start, name, end)),
            _ => Err(occurrence),
        })
    })
}
