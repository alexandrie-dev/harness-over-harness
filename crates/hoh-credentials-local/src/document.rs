//! The credentials document: strict parse, the flat-layout migration and the masked preview.

use hoh_credentials::{
    ApiKeyRecord, CredentialRecord, GrantRecord, credential_ref, parse_credential_key,
};
use hoh_error::BoxError;
use indexmap::IndexMap;
use serde_json::{Number, Value};

use crate::syntax::{Kind, Yaml, parse_syntax, scalar_value, to_yaml};

/// The document layout this build reads and writes.
pub const DOCUMENT_VERSION: u32 = 1;

/// One parsed credentials document: the two key spaces it stores, keyed as written, plus the
/// top-level keys it does not read.
#[derive(Clone, Default, PartialEq)]
pub struct CredentialsDocument {
    /// Reference entries, keyed by credential reference.
    pub refs: IndexMap<String, String>,
    /// Stored records, keyed by credential key.
    pub records: IndexMap<String, CredentialRecord>,
    /// Top-level keys this build does not read; they stay on disk and resolve nothing.
    pub ignored_keys: Vec<String>,
}

impl std::fmt::Debug for CredentialsDocument {
    /// Entry names and record kinds only: the document holds credentials, and a `Debug` that
    /// reaches a log or a panic message must not print them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let refs: Vec<&String> = self.refs.keys().collect();
        let records: Vec<(&String, hoh_credentials::CredentialRecordKind)> = self
            .records
            .iter()
            .map(|(key, record)| (key, record.kind()))
            .collect();
        f.debug_struct("CredentialsDocument")
            .field("refs", &refs)
            .field("records", &records)
            .field("ignored_keys", &self.ignored_keys)
            .finish()
    }
}

fn error(message: String) -> BoxError {
    message.into()
}

/// Describe parse failures without quoting the source: the parser's own message embeds the
/// offending line, which here holds a secret.
fn invalid_document(filename: &str, error: &crate::syntax::SyntaxError) -> BoxError {
    self::error(format!(
        "credentials-local: invalid document at {filename}: {}",
        error.describe()
    ))
}

/// A typed YAML value as JSON; `None` for a value JSON cannot hold (non-finite, cyclic).
fn to_json(value: &Yaml) -> Option<Value> {
    Some(match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => Value::Bool(*value),
        Yaml::Int(value) => Value::Number((*value).into()),
        Yaml::Float(value) => {
            if !value.is_finite() {
                return None;
            }
            if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
                Value::Number(Number::from(*value as i64))
            } else {
                Value::Number(Number::from_f64(*value)?)
            }
        }
        Yaml::Str(value) => Value::String(value.clone()),
        Yaml::Seq(items) => Value::Array(items.iter().map(to_json).collect::<Option<_>>()?),
        Yaml::Map(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| Some((key.clone(), to_json(value)?)))
                .collect::<Option<_>>()?,
        ),
        Yaml::Cycle => return None,
    })
}

/// Parse one credentials document. Inside `refs` and `records` everything is rejected rather
/// than skipped; an unknown top-level key is returned in `ignored_keys` for the caller to report.
/// An empty document is an empty store and needs no version.
pub fn parse_credentials_document(
    text: &str,
    filename: &str,
) -> Result<CredentialsDocument, BoxError> {
    let syntax = parse_syntax(text).map_err(|failure| invalid_document(filename, &failure))?;
    let root = match &syntax.root {
        None => Yaml::Null,
        Some(root) => to_yaml(root).map_err(|failure| invalid_document(filename, &failure))?,
    };
    let fields = match root {
        Yaml::Null => Vec::new(),
        Yaml::Map(fields) => fields,
        _ => {
            return Err(error(format!(
                "credentials-local: {filename} must be a mapping"
            )));
        }
    };
    // An empty (or comment-only) document is the empty store and needs no version.
    if fields.is_empty() {
        return Ok(CredentialsDocument::default());
    }
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    let Some(version) = field("version") else {
        return Err(error(format!(
            "credentials-local: {filename} uses the pre-release flat layout. Add `version: {DOCUMENT_VERSION}` and nest the existing {} {} under `refs:`. No values need to change.",
            fields.len(),
            if fields.len() == 1 {
                "entry"
            } else {
                "entries"
            },
        )));
    };
    let is_current = match version {
        Yaml::Int(value) => *value == i64::from(DOCUMENT_VERSION),
        Yaml::Float(value) => *value == f64::from(DOCUMENT_VERSION),
        _ => false,
    };
    if !is_current {
        return Err(error(format!(
            "credentials-local: {filename} declares an unsupported version; this build reads version {DOCUMENT_VERSION}"
        )));
    }
    Ok(CredentialsDocument {
        refs: parse_refs(field("refs"), filename)?,
        records: parse_records(field("records"), filename)?,
        ignored_keys: fields
            .iter()
            .map(|(key, _)| key.clone())
            .filter(|key| key != "version" && key != "refs" && key != "records")
            .collect(),
    })
}

/// One section of the document as its entries; absent and null both mean empty.
fn as_section<'a>(
    section: Option<&'a Yaml>,
    name: &str,
    filename: &str,
) -> Result<&'a [(String, Yaml)], BoxError> {
    match section {
        None | Some(Yaml::Null) => Ok(&[]),
        Some(Yaml::Map(entries)) => Ok(entries),
        Some(_) => Err(error(format!(
            "credentials-local: \"{name}\" in {filename} must be a mapping"
        ))),
    }
}

/// Admit a `refs` section: POSIX-identifier keys over non-empty string values.
fn parse_refs(
    section: Option<&Yaml>,
    filename: &str,
) -> Result<IndexMap<String, String>, BoxError> {
    let mut entries = IndexMap::new();
    for (key, value) in as_section(section, "refs", filename)? {
        // credentialRef throws on anything that is not a POSIX identifier, which is exactly the
        // constraint a stored reference must satisfy to be addressable through the seam.
        credential_ref(key)?;
        // The key name is quoted, never the value: a wrong-typed entry is still a secret the user
        // meant to store.
        let Yaml::Str(value) = value else {
            return Err(error(format!(
                "credentials-local: the value for \"{key}\" in {filename} must be a string"
            )));
        };
        if value.is_empty() {
            return Err(error(format!(
                "credentials-local: the value for \"{key}\" in {filename} is empty; remove the key instead"
            )));
        }
        entries.insert(key.clone(), value.clone());
    }
    Ok(entries)
}

/// Admit a `records` section: `<scope>/<id>` keys over tagged record mappings.
fn parse_records(
    section: Option<&Yaml>,
    filename: &str,
) -> Result<IndexMap<String, CredentialRecord>, BoxError> {
    let mut entries = IndexMap::new();
    for (key, value) in as_section(section, "records", filename)? {
        parse_credential_key(key)?;
        entries.insert(key.clone(), parse_record(key, value, filename)?);
    }
    Ok(entries)
}

/// Refuse an api-key record the read path could not admit, before it is rendered: an empty key,
/// an env name outside the reference grammar, or an empty env value would persist a document
/// `parse_record` rejects at the next boot.
pub(crate) fn assert_storable_api_key(key: &str, record: &ApiKeyRecord) -> Result<(), BoxError> {
    if record.key.as_deref() == Some("") {
        return Err(error(format!(
            "credentials-local: record \"{key}\" has an empty key; omit the field instead"
        )));
    }
    for (name, value) in record.env.iter().flatten() {
        credential_ref(name)?;
        if value.is_empty() {
            return Err(error(format!(
                "credentials-local: record \"{key}\" env \"{name}\" must be a non-empty string"
            )));
        }
    }
    Ok(())
}

fn field<'a>(fields: &'a [(String, Yaml)], name: &str) -> Option<&'a Yaml> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// Admit one record entry, rejecting an unknown tag or field rather than dropping it.
fn parse_record(key: &str, value: &Yaml, filename: &str) -> Result<CredentialRecord, BoxError> {
    let Yaml::Map(fields) = value else {
        return Err(error(format!(
            "credentials-local: record \"{key}\" in {filename} must be a mapping"
        )));
    };
    match field(fields, "kind") {
        Some(Yaml::Str(kind)) if kind == "api-key" => {
            assert_fields(key, fields, &["kind", "key", "env"], filename)?;
            let api_key = match field(fields, "key") {
                None => None,
                Some(Yaml::Str(text)) if !text.is_empty() => Some(text.clone()),
                Some(_) => {
                    return Err(error(format!(
                        "credentials-local: record \"{key}\" in {filename} has a non-string or empty key"
                    )));
                }
            };
            let env = parse_record_env(key, field(fields, "env"), filename)?;
            Ok(CredentialRecord::ApiKey(ApiKeyRecord { key: api_key, env }))
        }
        Some(Yaml::Str(kind)) if kind == "grant" => {
            assert_fields(key, fields, &["kind", "payload"], filename)?;
            let Some(payload) = field(fields, "payload") else {
                return Err(error(format!(
                    "credentials-local: record \"{key}\" in {filename} has no payload"
                )));
            };
            let payload = admit_json(&format!("record \"{key}\" payload in {filename}"), payload)?;
            Ok(CredentialRecord::Grant(GrantRecord { payload }))
        }
        None => Err(error(format!(
            "credentials-local: record \"{key}\" in {filename} has no kind"
        ))),
        Some(_) => Err(error(format!(
            "credentials-local: record \"{key}\" in {filename} has an unknown kind (expected \"api-key\" or \"grant\")"
        ))),
    }
}

/// Reject a field the tag does not define, so a typo is not silently dropped.
fn assert_fields(
    key: &str,
    fields: &[(String, Yaml)],
    allowed: &[&str],
    filename: &str,
) -> Result<(), BoxError> {
    for (name, _) in fields {
        if !allowed.contains(&name.as_str()) {
            return Err(error(format!(
                "credentials-local: record \"{key}\" in {filename} has unknown field \"{name}\""
            )));
        }
    }
    Ok(())
}

/// Admit an api-key record's provider environment: POSIX names over non-empty strings.
fn parse_record_env(
    key: &str,
    env: Option<&Yaml>,
    filename: &str,
) -> Result<Option<IndexMap<String, String>>, BoxError> {
    let Some(env) = env else {
        return Ok(None);
    };
    let Yaml::Map(entries) = env else {
        return Err(error(format!(
            "credentials-local: record \"{key}\" in {filename} has a non-mapping env"
        )));
    };
    let mut parsed = IndexMap::new();
    for (name, value) in entries {
        credential_ref(name)?;
        match value {
            Yaml::Str(text) if !text.is_empty() => {
                parsed.insert(name.clone(), text.clone());
            }
            _ => {
                return Err(error(format!(
                    "credentials-local: record \"{key}\" env \"{name}\" in {filename} must be a non-empty string"
                )));
            }
        }
    }
    Ok(Some(parsed))
}

/// Reject a payload that cannot survive a JSON round trip. The document may spell `.inf` or an
/// alias cycle; neither the value nor any nested value is quoted in a diagnostic.
fn admit_json(where_: &str, value: &Yaml) -> Result<Value, BoxError> {
    fn check(where_: &str, value: &Yaml) -> Result<(), BoxError> {
        match value {
            Yaml::Null | Yaml::Bool(_) | Yaml::Int(_) | Yaml::Str(_) => Ok(()),
            Yaml::Float(number) if number.is_finite() => Ok(()),
            Yaml::Float(_) => Err(error(format!(
                "credentials-local: {where_} holds a non-finite number"
            ))),
            Yaml::Seq(items) => items.iter().try_for_each(|item| check(where_, item)),
            Yaml::Map(entries) => entries.iter().try_for_each(|(_, item)| check(where_, item)),
            Yaml::Cycle => Err(error(format!("credentials-local: {where_} is cyclic"))),
        }
    }
    check(where_, value)?;
    to_json(value).ok_or_else(|| {
        error(format!(
            "credentials-local: {where_} holds a value JSON cannot represent"
        ))
    })
}

/// Structural equality over two admitted JSON values; key order is ignored because an external
/// editor may reorder a record's fields without changing what it stores.
pub(crate) fn same_json_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| same_json_value(value, other))
                })
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| same_json_value(a, b))
        }
        _ => left == right,
    }
}

/// Whether two stored records carry the same JSON content.
pub(crate) fn same_record(
    left: Option<&CredentialRecord>,
    right: Option<&CredentialRecord>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => same_json_value(
            &serde_json::to_value(left).unwrap_or(Value::Null),
            &serde_json::to_value(right).unwrap_or(Value::Null),
        ),
        _ => false,
    }
}

/// Keep the provider identifier and five leading/four trailing key code points only when at
/// least eight key code points stay hidden; otherwise mask all.
pub fn mask_credential(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let provider_end = chars
        .iter()
        .position(|c| *c == '-' || *c == '_')
        .map_or(0, |i| i + 1);
    let hidden = chars.len() as i64 - provider_end as i64 - 5 - 4;
    if hidden < 8 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..provider_end + 5].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}{}{tail}", "•".repeat(hidden as usize))
}

/// Render the version-1 layout for a pre-release flat document, or `None` for anything else.
///
/// The flat layout is recognized exactly — a non-empty top-level mapping of addressable reference
/// names to non-empty string scalars, with no `version` key and no document directives — and the
/// rewrite nests the original lines verbatim under `refs:` at two spaces' indent, so comments,
/// blank lines and each value's spelling survive byte for byte.
pub fn render_flat_layout_migration(text: &str) -> Option<String> {
    let syntax = parse_syntax(text).ok()?;
    let root = syntax.root.as_ref()?;
    let Kind::Map(entries) = &root.kind else {
        return None;
    };
    if entries.is_empty() {
        return None;
    }
    for line in text.split('\n') {
        // A directive or document marker would not survive being indented into the `refs:` block.
        if line.starts_with('%') || line.starts_with("---") || line.starts_with("...") {
            return None;
        }
    }
    for (key, value) in entries {
        let Some(Yaml::Str(name)) = scalar_value(key) else {
            return None;
        };
        if name == "version" || credential_ref(&name).is_err() {
            return None;
        }
        match scalar_value(value) {
            Some(Yaml::Str(text)) if !text.is_empty() => {}
            _ => return None,
        }
    }
    let body = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                line.to_owned()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "version: {DOCUMENT_VERSION}\nrefs:\n{body}{}",
        if text.ends_with('\n') { "" } else { "\n" }
    ))
}
