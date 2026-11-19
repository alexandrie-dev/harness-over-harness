//! Current JSONL records, safe path components, and incremental torn-tail scanning.
use crate::{
    Result,
    error::{cause, error},
    shim,
};
use hoh_session::{
    SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionId, SessionLogOffset,
};
use hoh_session_format::{
    SessionFormatRecovery, parse_session_format_log_filename, session_format_log_filename,
};
use hoh_util_values::{js_json_parse, js_json_stringify};
use hoh_util_workspace_path::node_path;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum JsonlCompression {
    #[default]
    Zstd,
    None,
}
impl JsonlCompression {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Zstd => Self::None,
            Self::None => Self::Zstd,
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::None => "none",
        }
    }
}
pub fn log_suffix(compression: JsonlCompression) -> &'static str {
    match compression {
        JsonlCompression::Zstd => ".jsonl.zstd",
        JsonlCompression::None => ".jsonl",
    }
}
pub fn generation_log_filename(version: u64, compression: JsonlCompression) -> Result<String> {
    Ok(format!(
        "{}{}",
        session_format_log_filename(version)?,
        if compression == JsonlCompression::Zstd {
            ".zstd"
        } else {
            ""
        }
    ))
}
pub fn parse_generation_log_filename(filename: &str, compression: JsonlCompression) -> Option<u64> {
    let name = if compression == JsonlCompression::Zstd {
        filename.strip_suffix(".zstd")?
    } else {
        filename
    };
    parse_session_format_log_filename(name)
}
pub fn assert_no_retired_header_fields(value: &Value) -> Result<()> {
    if value
        .as_object()
        .is_some_and(|v| v.contains_key("sandboxMode") || v.contains_key("approvalPolicy"))
    {
        return Err(error("session header uses retired policy baseline fields"));
    }
    Ok(())
}
pub fn to_header_line(
    header: &SessionHeader,
    inherited_event_count: Option<SessionLogOffset>,
) -> Result<Value> {
    if header.is_seeded && inherited_event_count.is_none() {
        return Err(error(
            "seeded session header requires an inherited event count",
        ));
    }
    let cut = inherited_event_count.unwrap_or_default();
    if !header.is_seeded && cut.get() != 0 {
        return Err(error(
            "unseeded session header inherited event count must be 0",
        ));
    }
    shim::encode_header(header, cut)
}
pub fn encode_segment(raw: &str) -> Result<String> {
    encode_segment_utf16(&raw.encode_utf16().collect::<Vec<_>>())
}
/// UTF-16 boundary for identities containing lone JavaScript surrogates.
pub fn encode_segment_utf16(raw: &[u16]) -> Result<String> {
    if raw.is_empty() {
        return Err(error("cannot encode an empty path segment"));
    }
    if raw == [46] {
        return Ok("~002E".into());
    }
    if raw == [46, 46] {
        return Ok("~002E~002E".into());
    }
    let mut out = String::new();
    for &unit in raw {
        append_unit(&mut out, unit);
    }
    Ok(out)
}
fn append_unit(out: &mut String, unit: u16) {
    if unit < 128 && (unit as u8).is_ascii_alphanumeric() || [46, 95, 45].contains(&unit) {
        out.push(char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}'));
    } else {
        use std::fmt::Write;
        let _ = write!(out, "~{unit:04X}");
    }
}
pub fn project_key(cwd: &str) -> Result<String> {
    if cwd.is_empty() {
        return Err(error("cannot encode an empty project path"));
    }
    let mut readable = String::new();
    let mut separator = false;
    for unit in cwd.encode_utf16() {
        if [47, 92, 58].contains(&unit) {
            if !separator {
                readable.push('-');
            }
            separator = true;
        } else {
            append_unit(&mut readable, unit);
            separator = false;
        }
    }
    let slug = readable.trim_start_matches('-');
    let slug = if slug.is_empty() { "root" } else { slug };
    Ok(format!("--{}--", &slug[..slug.len().min(251)]))
}
pub fn project_dir(root: &str, cwd: Option<&str>) -> Result<String> {
    Ok(node_path::join(&[
        root,
        &match cwd {
            Some(cwd) => project_key(cwd)?,
            None => "_no-cwd".into(),
        },
    ]))
}
pub fn session_dir(root: &str, cwd: Option<&str>, id: &SessionId) -> Result<String> {
    Ok(node_path::join(&[
        &project_dir(root, cwd)?,
        &encode_segment(id.as_str())?,
    ]))
}
pub fn generation_log_path(
    root: &str,
    cwd: Option<&str>,
    id: &SessionId,
    version: u64,
    compression: JsonlCompression,
) -> Result<String> {
    Ok(node_path::join(&[
        &session_dir(root, cwd, id)?,
        &generation_log_filename(version, compression)?,
    ]))
}
pub fn log_path(
    root: &str,
    cwd: Option<&str>,
    id: &SessionId,
    compression: JsonlCompression,
) -> Result<String> {
    generation_log_path(root, cwd, id, SESSION_FORMAT_VERSION, compression)
}
pub fn event_line(event: &SessionEvent) -> Result<String> {
    Ok(js_json_stringify(&shim::encode_event(event)?))
}
pub fn event_lines(events: &[SessionEvent]) -> Result<String> {
    events
        .iter()
        .map(event_line)
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionLogScan {
    pub meta: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
    pub events: Vec<SessionEvent>,
    pub committed_bytes: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScannerCheckpoint {
    pub input_bytes: usize,
    pub committed_bytes: usize,
    pub event_count: SessionLogOffset,
}
pub struct SessionLogScanner {
    meta: SessionHeader,
    events: Vec<SessionEvent>,
    fragments: Vec<u8>,
    input_bytes: usize,
    committed_bytes: usize,
    event_line: u64,
    issue: Option<hoh_error::SharedError>,
    finished: bool,
    recovery: SessionFormatRecovery,
}
impl SessionLogScanner {
    pub fn new(header_record: &[u8], recovery: SessionFormatRecovery) -> Result<Self> {
        if header_record.last() != Some(&b'\n')
            || header_record.iter().position(|&b| b == b'\n') != Some(header_record.len() - 1)
        {
            return Err(error("empty or header-less session log"));
        }
        let parsed = js_json_parse(&String::from_utf8_lossy(
            &header_record[..header_record.len() - 1],
        ))
        .map_err(|_| error("corrupt session log: header line is not valid JSON"))?;
        if !parsed.is_object() {
            return Err(error(
                "corrupt session log: first line is not a JSON object",
            ));
        }
        if let Some(version) = parsed.get("version").and_then(Value::as_f64)
            && version != SESSION_FORMAT_VERSION as f64
        {
            let id = parsed
                .get("id")
                .map_or_else(|| "undefined".into(), hoh_util_values::js_string);
            return Err(Box::new(
                hoh_session_persistence::SessionFormatUnsupportedError::new(
                    if version > SESSION_FORMAT_VERSION as f64 {
                        format!(
                            "session \"{id}\" uses log format v{}, but this harness reads only v4: the log was written by a newer harness — upgrade the harness to open it",
                            hoh_util_values::js_number_string(version)
                        )
                    } else {
                        format!(
                            "session \"{id}\" uses log format v{}, older than the supported v4, and this build ships no upgrade path for it",
                            hoh_util_values::js_number_string(version)
                        )
                    },
                    None,
                ),
            ));
        }
        assert_no_retired_header_fields(&parsed)?;
        let meta = shim::decode_header(&parsed)
            .map_err(|_| error("corrupt session log: first line is not a session header"))?;
        Ok(Self {
            meta,
            events: vec![],
            fragments: vec![],
            input_bytes: header_record.len(),
            committed_bytes: header_record.len(),
            event_line: 0,
            issue: None,
            finished: false,
            recovery,
        })
    }
    pub fn write(&mut self, chunk: &[u8]) -> Result<()> {
        if self.finished {
            return Err(error("cannot write to a finished session log scanner"));
        }
        let start = self.input_bytes;
        self.input_bytes += chunk.len();
        let mut offset = 0;
        for (index, &byte) in chunk.iter().enumerate() {
            if byte != b'\n' {
                continue;
            }
            self.fragments.extend_from_slice(&chunk[offset..index]);
            let line = std::mem::take(&mut self.fragments);
            self.consume(&line, start + index + 1)?;
            offset = index + 1;
        }
        self.fragments.extend_from_slice(&chunk[offset..]);
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<ScannerCheckpoint> {
        Ok(ScannerCheckpoint {
            input_bytes: self.input_bytes,
            committed_bytes: self.committed_bytes,
            event_count: SessionLogOffset::new(self.events.len() as u64)?,
        })
    }
    pub fn finish(&mut self) -> Result<SessionLogScan> {
        self.finished = true;
        let inherited = shim::inherited_cut(&self.meta, &self.events)?;
        shim::assert_relationships(&self.meta, &self.events, inherited)?;
        Ok(SessionLogScan {
            meta: self.meta.clone(),
            inherited_event_count: inherited,
            events: self.events.clone(),
            committed_bytes: self.committed_bytes,
        })
    }
    fn consume(&mut self, line: &[u8], end_byte: usize) -> Result<()> {
        self.event_line += 1;
        let decoded = match js_json_parse(&String::from_utf8_lossy(line)) {
            Ok(value) => value,
            Err(_) => {
                let issue = error(format!(
                    "corrupt session log: unparsable committed event at line {}",
                    self.event_line
                ));
                if self.recovery == SessionFormatRecovery::Strict {
                    return Err(issue);
                }
                if self.issue.is_none() {
                    self.issue = Some(hoh_error::SharedError::new(issue));
                }
                return Ok(());
            }
        };
        shim::assert_row_admission(&decoded)?;
        let terminal = decoded.get("type").is_some_and(|v| v == "turn/end");
        if let Some(issue) = &self.issue {
            return if terminal {
                Err(Box::new(issue.clone()))
            } else {
                Ok(())
            };
        }
        let event = match shim::decode_event(decoded, self.events.len() as u64, self.event_line - 1)
        {
            Ok(event) => event,
            Err(failure) => {
                let issue = cause(
                    format!(
                        "corrupt session log: invalid committed event at line {}: {failure}",
                        self.event_line
                    ),
                    failure,
                );
                if self.recovery == SessionFormatRecovery::Strict || terminal {
                    return Err(issue);
                }
                self.issue = Some(hoh_error::SharedError::new(issue));
                return Ok(());
            }
        };
        self.events.push(event);
        self.committed_bytes = end_byte;
        Ok(())
    }
}
pub fn scan_log(buffer: &[u8]) -> Result<SessionLogScan> {
    let header_end = buffer
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| error("empty or header-less session log"))?;
    let mut scanner =
        SessionLogScanner::new(&buffer[..=header_end], SessionFormatRecovery::Recoverable)?;
    scanner.write(&buffer[header_end + 1..])?;
    scanner.finish()
}
