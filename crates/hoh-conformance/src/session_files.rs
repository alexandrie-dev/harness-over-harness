//! Immutable Session-generation filenames of recorded fixtures and persistence
//! (`session-snapshot/src/session-files.ts`).
use crate::Result;
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

/// One canonical recorded-session fixture filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFixtureFile {
    /// Parent is `0`; positive values are child/ordinal slots.
    pub index: u64,
    /// Physical Session format generation; zero is encoded by omission.
    pub version: u64,
    pub name: String,
}

/// One canonical persistence filename in a Session's own storage directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSessionFile {
    pub version: u64,
    /// `true` for the zstd-compressed backend.
    pub zstd: bool,
    pub name: String,
}

fn non_negative_safe_integer(value: f64, label: &str) -> Result<u64> {
    if !value.is_finite()
        || value.fract() != 0.0
        || !(0.0..=MAX_SAFE).contains(&value)
        || (value == 0.0 && value.is_sign_negative())
    {
        return Err(format!("{label} must be a non-negative safe integer"));
    }
    Ok(value as u64)
}

/// The canonical fixture filename of one parent/ordinal and generation (`0` is omitted).
pub fn session_fixture_name(index: f64, version: f64) -> Result<String> {
    let index = non_negative_safe_integer(index, "session fixture index")?;
    let version = non_negative_safe_integer(version, "Session format version")?;
    let ordinal = if index == 0 {
        String::new()
    } else {
        format!(".{index}")
    };
    let generation = if version == 0 {
        String::new()
    } else {
        format!(".v{version}")
    };
    Ok(format!("session{ordinal}{generation}.jsonl"))
}

/// The native-writer oracle name of a retained historical replay role.
pub fn writer_snapshot_name(index: f64) -> Result<String> {
    let index = non_negative_safe_integer(index, "writer snapshot index")?;
    Ok(if index == 0 {
        "writer.expected.jsonl".to_owned()
    } else {
        format!("writer.{index}.expected.jsonl")
    })
}

/// Parse one canonical fixture filename; `None` for an unrelated file.
pub fn parse_session_fixture_name(name: &str) -> Result<Option<SessionFixtureFile>> {
    let pattern = Regex::new(r"^session(?:\.([1-9][0-9]*))?(?:\.v([1-9][0-9]*))?\.jsonl$")
        .map_err(|e| e.to_string())?;
    let Some(found) = pattern.captures(name) else {
        if name.starts_with("session") && name.ends_with(".jsonl") {
            return Err(format!("invalid session fixture name: {name}"));
        }
        return Ok(None);
    };
    let number = |group: usize| -> Result<u64> {
        match found.get(group) {
            None => Ok(0),
            Some(text) => text
                .as_str()
                .parse::<f64>()
                .ok()
                .filter(|n| *n <= MAX_SAFE)
                .map(|n| n as u64)
                .ok_or_else(|| format!("invalid session fixture name: {name}")),
        }
    };
    Ok(Some(SessionFixtureFile {
        index: number(1)?,
        version: number(2)?,
        name: name.to_owned(),
    }))
}

/// The highest generation of every parent/ordinal role; parent first, roles contiguous.
pub fn session_fixture_files(names: &[&str]) -> Result<Vec<SessionFixtureFile>> {
    let mut selected: BTreeMap<u64, SessionFixtureFile> = BTreeMap::new();
    let mut identities = BTreeSet::new();
    for name in names {
        let Some(fixture) = parse_session_fixture_name(name)? else {
            continue;
        };
        if !identities.insert((fixture.index, fixture.version)) {
            return Err(format!("duplicate session fixture generation: {name}"));
        }
        if selected
            .get(&fixture.index)
            .is_none_or(|previous| fixture.version > previous.version)
        {
            selected.insert(fixture.index, fixture);
        }
    }
    if !selected.contains_key(&0) {
        return Err("missing parent session fixture".into());
    }
    let ordered: Vec<_> = selected.into_values().collect();
    for (offset, fixture) in ordered.iter().enumerate() {
        if fixture.index != offset as u64 {
            return Err(format!(
                "session fixture roles must be contiguous: expected index {offset}, found {}",
                fixture.name
            ));
        }
    }
    Ok(ordered)
}

/// Filenames of the selected fixtures.
pub fn session_fixture_names(names: &[&str]) -> Result<Vec<String>> {
    Ok(session_fixture_files(names)?
        .into_iter()
        .map(|file| file.name)
        .collect())
}

/// The declared physical generation of one Session JSONL header.
pub fn session_header_version(content: &str, label: &str) -> Result<u64> {
    let line = content
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .find(|l| !hoh_util_values::js_trim(l).is_empty())
        .ok_or_else(|| format!("{label}: session fixture is empty"))?;
    let value = hoh_util_values::js_json_parse(line)
        .map_err(|_| format!("{label}: session header contains invalid JSON"))?;
    if !value.is_object() || value["type"] != "session" {
        return Err(format!("{label}: first record must be a Session header"));
    }
    let valid = match &value["version"] {
        Value::Number(n) => {
            // `-0` is a float zero with a sign in JSON text; it is not a valid generation.
            n.as_u64().is_some_and(|v| v as f64 <= MAX_SAFE)
                && !line.contains("\"version\":-0")
                && !line.contains("\"version\": -0")
        }
        _ => false,
    };
    if !valid {
        return Err(format!(
            "{label}: Session header version must be a non-negative safe integer"
        ));
    }
    Ok(value["version"].as_u64().unwrap_or_default())
}

/// A fixture's filename generation must equal its header (a versionless projected header is v0).
pub fn assert_session_fixture_version(name: &str, content: &str) -> Result<u64> {
    let fixture = parse_session_fixture_name(name)?
        .ok_or_else(|| format!("not a session fixture name: {name}"))?;
    if let Some(first) = content
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .find(|l| !hoh_util_values::js_trim(l).is_empty())
        && let Ok(projected) = hoh_util_values::js_json_parse(first)
        && projected.is_object()
        && projected["type"] == "session"
        && projected.get("version").is_none()
    {
        if fixture.version != 0 {
            return Err(format!(
                "{name}: a versionless projected Session header is format v0"
            ));
        }
        return Ok(0);
    }
    let header = session_header_version(content, name)?;
    if header != fixture.version {
        return Err(format!(
            "{name}: filename declares Session format v{}, header declares v{header}",
            fixture.version
        ));
    }
    Ok(header)
}

/// The canonical persistence basename for one generation and compression.
pub fn persisted_session_filename(version: f64, zstd: bool) -> Result<String> {
    let version = non_negative_safe_integer(version, "Session format version")?;
    let base = if version == 0 {
        "session.jsonl".to_owned()
    } else {
        format!("session.v{version}.jsonl")
    };
    Ok(if zstd { format!("{base}.zstd") } else { base })
}

/// Parse a canonical persistence basename; `None` for noise and noncanonical names.
pub fn parse_persisted_session_filename(name: &str) -> Option<PersistedSessionFile> {
    let zstd = name.ends_with(".zstd");
    let base = if zstd {
        &name[..name.len() - ".zstd".len()]
    } else {
        name
    };
    let pattern = Regex::new(r"^session(?:\.v([1-9][0-9]*))?\.jsonl$").ok()?;
    let found = pattern.captures(base)?;
    let version = match found.get(1) {
        None => 0,
        Some(text) => text
            .as_str()
            .parse::<f64>()
            .ok()
            .filter(|n| *n <= MAX_SAFE)? as u64,
    };
    Some(PersistedSessionFile {
        version,
        zstd,
        name: name.to_owned(),
    })
}

/// One highest-generation persistence path per physical Session directory, in stable order.
pub fn latest_persisted_session_paths(paths: &[&str], zstd: bool) -> Vec<String> {
    let mut selected: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for path in paths {
        let (directory, base) = match path.rfind('/') {
            Some(at) => (&path[..at], &path[at + 1..]),
            None => (".", *path),
        };
        let Some(parsed) = parse_persisted_session_filename(base) else {
            continue;
        };
        if parsed.zstd != zstd {
            continue;
        }
        if selected
            .get(directory)
            .is_none_or(|(_, version)| parsed.version > *version)
        {
            selected.insert(directory.to_owned(), ((*path).to_owned(), parsed.version));
        }
    }
    let mut result: Vec<String> = selected.into_values().map(|(path, _)| path).collect();
    result.sort();
    result
}

/// A persistence basename's generation must equal its Session header.
pub fn assert_persisted_session_version(name: &str, content: &str) -> Result<u64> {
    let persisted = parse_persisted_session_filename(name)
        .ok_or_else(|| format!("not a canonical Session persistence filename: {name}"))?;
    let header = session_header_version(content, name)?;
    if header != persisted.version {
        return Err(format!(
            "{name}: filename declares Session format v{}, header declares v{header}",
            persisted.version
        ));
    }
    Ok(header)
}
