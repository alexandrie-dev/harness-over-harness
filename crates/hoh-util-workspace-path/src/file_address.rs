//! The `dsh-resource://file/…` grammar, independent of filesystem and URL resolution.

use serde::{Deserialize, Serialize};

/// A Session-scoped file or a Session-less absolute file address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope")]
pub enum FileAddress {
    #[serde(rename = "session")]
    Session {
        #[serde(rename = "sessionId")]
        session_id: String,
        path: String,
    },
    #[serde(rename = "absolute")]
    Absolute { path: String },
}

const FILE_ADDRESS_PREFIX: &str = "dsh-resource://file/";

pub(crate) fn push_percent_encoded(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('%');
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 15)]));
}

pub(crate) fn encode_component(value: &str, keep_colon: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
            || (keep_colon && byte == b':')
        {
            encoded.push(char::from(byte));
        } else {
            push_percent_encoded(&mut encoded, byte);
        }
    }
    encoded
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|segment| encode_component(segment, true))
        .collect::<Vec<_>>()
        .join("/")
}

fn decode_component(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            decoded.push((high * 16 + low) as u8);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).ok()
}

fn is_drive_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Build a Session address, normalizing backslashes and leading `./` prefixes.
pub fn session_file_address(session_id: &str, path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_start_matches("./");
    format!(
        "{FILE_ADDRESS_PREFIX}session/{}/{}",
        encode_component(session_id, true),
        encode_path(normalized)
    )
}

/// Build a Session-less absolute address, preserving a UNC marker.
pub fn absolute_file_address(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let unc = if normalized.starts_with("//") {
        "/"
    } else {
        ""
    };
    let absolute = normalized.trim_start_matches('/');
    format!(
        "{FILE_ADDRESS_PREFIX}absolute/{unc}{}",
        encode_path(absolute)
    )
}

/// Decode one address without canonicalizing dot segments or separators.
pub fn parse_file_address(address: &str) -> Option<FileAddress> {
    let address = address.strip_prefix(FILE_ADDRESS_PREFIX)?;
    let address = address.split(['?', '#']).next()?;
    let mut segments = address.split('/');
    match segments.next()? {
        "session" => {
            let id = segments.next().filter(|id| !id.is_empty())?;
            let paths = segments.map(decode_component).collect::<Option<Vec<_>>>()?;
            if paths.is_empty() {
                return None;
            }
            Some(FileAddress::Session {
                session_id: decode_component(id)?,
                path: paths.join("/"),
            })
        }
        "absolute" => {
            let rest = segments.collect::<Vec<_>>();
            let unc = rest.first() == Some(&"") && rest.len() > 1;
            let paths = if unc { &rest[1..] } else { &rest[..] };
            let paths = paths
                .iter()
                .map(|segment| decode_component(segment))
                .collect::<Option<Vec<_>>>()?;
            let first = paths.first().filter(|segment| !segment.is_empty())?;
            let path = paths.join("/");
            let path = if unc {
                format!("//{path}")
            } else if is_drive_segment(first) {
                path
            } else {
                format!("/{path}")
            };
            Some(FileAddress::Absolute { path })
        }
        _ => None,
    }
}
