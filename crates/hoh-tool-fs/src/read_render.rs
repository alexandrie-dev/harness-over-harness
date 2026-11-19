//! Pure read presentation: turn provider-decoded text into a bounded, line-numbered window and
//! model-facing envelope. Chunk scanning caps the current line, so even one newline-free giant line
//! cannot grow memory without bound.

use futures_util::{Stream, StreamExt};
use hoh_error::BoxError;
use hoh_fs::{FsError, FsErrorCode};
use hoh_util_values::js_number_string;
use serde_json::Value;

/// Default maximum characters returned for a single line (the `readMaxLineLength` config).
pub const READ_MAX_LINE_LENGTH: u64 = 2000;

/// Default maximum bytes returned for selected file lines (the `readMaxBytes` config).
pub const READ_MAX_BYTES: u64 = 50 * 1024;

/// Resolved read window. The consumer applies its defaults/caps before calling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadWindow {
    /// 1-based first line to return.
    pub offset: f64,
    /// Maximum number of lines to return.
    pub limit: u64,
    /// Maximum characters (UTF-16 units) returned for a single line; overflow is truncated with a suffix.
    pub max_line_length: u64,
    /// Maximum bytes of selected output; overflow stops the scan and marks `truncated_by_bytes`.
    pub max_bytes: u64,
}

/// One line returned from a text file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTextLine {
    /// 1-based line number in the file.
    pub number: u64,
    /// Line text without its trailing newline.
    pub text: String,
}

/// The windowed result [`build_window`] produces from a file's decoded text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowResult {
    /// Returned lines, already numbered.
    pub lines: Vec<FileTextLine>,
    /// Exact total line count in the file.
    pub total_lines: u64,
    /// Whether selected output hit the byte cap.
    pub truncated_by_bytes: bool,
}

/// Outcome of a bounded text read — what [`format_read_output`] renders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReadOutcome {
    /// 1-based first line requested.
    pub offset: u64,
    /// Returned lines, already numbered.
    pub lines: Vec<FileTextLine>,
    /// Exact total line count in the file.
    pub total_lines: u64,
    /// Whether selected output hit the byte cap.
    pub truncated_by_bytes: bool,
}

struct Accumulator {
    lines: Vec<FileTextLine>,
    total_lines: u64,
    output_bytes: u64,
    truncated_by_bytes: bool,
}

/// `text.substring(0, units)` of a JavaScript string, with the one difference a Rust string forces:
/// a cut inside a surrogate pair leaves a lone high surrogate in JavaScript, which UTF-8 encoding
/// (the bytes the model receives) turns into U+FFFD, so that is what is kept. Returns the prefix and
/// whether anything was cut.
fn utf16_prefix(text: &str, units: u64) -> (String, bool) {
    let mut used = 0_u64;
    for (index, ch) in text.char_indices() {
        let width = ch.len_utf16() as u64;
        if used + width > units {
            let mut prefix = text[..index].to_owned();
            if used < units {
                prefix.push('\u{FFFD}');
            }
            return (prefix, true);
        }
        used += width;
    }
    (text.to_owned(), false)
}

fn truncate_line(line: &str, line_units: u64, max_line_length: u64) -> String {
    if line_units > max_line_length {
        format!(
            "{}... (line truncated to {max_line_length} chars)",
            utf16_prefix(line, max_line_length).0
        )
    } else {
        line.to_owned()
    }
}

fn consume_line(acc: &mut Accumulator, raw_line: &str, line_units: u64, request: &ReadWindow) {
    acc.total_lines += 1;
    if acc.truncated_by_bytes
        || (acc.total_lines as f64) < request.offset
        || acc.lines.len() as u64 >= request.limit
    {
        return;
    }
    let text = truncate_line(raw_line, line_units, request.max_line_length);
    let bytes = text.len() as u64 + u64::from(!acc.lines.is_empty());
    if acc.output_bytes + bytes > request.max_bytes {
        acc.truncated_by_bytes = true;
        return;
    }
    acc.output_bytes += bytes;
    acc.lines.push(FileTextLine {
        number: acc.total_lines,
        text,
    });
}

fn finish(
    acc: Accumulator,
    request: &ReadWindow,
    display_path: &str,
) -> Result<WindowResult, BoxError> {
    if !acc.truncated_by_bytes
        && request.offset > acc.total_lines as f64
        && !(acc.total_lines == 0 && request.offset == 1.0)
    {
        return Err(FsError::new(
            format!(
                "offset {} is out of range for \"{display_path}\" ({} lines)",
                js_number_string(request.offset),
                acc.total_lines
            ),
            FsErrorCode::NotFound,
        )
        .into());
    }
    Ok(WindowResult {
        lines: acc.lines,
        total_lines: acc.total_lines,
        truncated_by_bytes: acc.truncated_by_bytes,
    })
}

/// The bounded text of the line being scanned: one UTF-16 unit past the truncation point is enough
/// to prove that the line overflows. As in the source, the trailing `\r` is stripped from this
/// bounded text, so a `\r` standing exactly on the cap position leaves a line that fits.
struct LineBuffer {
    cap: u64,
    text: String,
    units: u64,
}

impl LineBuffer {
    fn new(max_line_length: u64) -> Self {
        Self {
            cap: max_line_length + 1,
            text: String::new(),
            units: 0,
        }
    }

    fn append(&mut self, segment: &str) {
        if segment.is_empty() {
            return;
        }
        if self.units >= self.cap {
            return;
        }
        let room = self.cap - self.units;
        let (kept, _) = utf16_prefix(segment, room);
        self.units += kept.encode_utf16().count() as u64;
        self.text.push_str(&kept);
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn flush(&mut self, acc: &mut Accumulator, request: &ReadWindow) {
        let line = std::mem::take(&mut self.text);
        let stripped = line.strip_suffix('\r');
        let (line, units) = match stripped {
            Some(stripped) => (stripped, self.units - 1),
            None => (line.as_str(), self.units),
        };
        consume_line(acc, line, units, request);
        self.units = 0;
    }
}

/// Build one window from streamed or whole-file chunks, enforcing line and byte caps while still
/// scanning to an exact total line count, and failing with `FS_NOT_FOUND` when the requested
/// offset is past EOF.
///
/// `chunks` are decoded text chunks in file order; chunk boundaries carry no meaning. `request`
/// is the resolved window (the caller has already applied its defaults and caps); `display_path`
/// is the caller-facing path used in the offset-out-of-range error.
///
/// # Errors
/// The stream's own failure, or `FS_NOT_FOUND` for an offset past the end of the file.
pub async fn build_window<S>(
    mut chunks: S,
    request: &ReadWindow,
    display_path: &str,
) -> Result<WindowResult, BoxError>
where
    S: Stream<Item = Result<String, BoxError>> + Unpin,
{
    let mut acc = Accumulator {
        lines: Vec::new(),
        total_lines: 0,
        output_bytes: 0,
        truncated_by_bytes: false,
    };
    let mut line = LineBuffer::new(request.max_line_length);
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        let mut rest = chunk.as_str();
        while let Some(position) = rest.find('\n') {
            line.append(&rest[..position]);
            line.flush(&mut acc, request);
            rest = &rest[position + 1..];
        }
        line.append(rest);
    }
    if !line.is_empty() {
        line.flush(&mut acc, request);
    }
    finish(acc, request, display_path)
}

/// Format a read outcome as one OpenCode-style line-numbered text block body.
///
/// `display_path` is the backend-resolved path rendered in the envelope's `<path>` element. Returns
/// the model-facing envelope: numbered lines plus a continuation or end-of-file footer.
pub fn format_read_output(display_path: &str, outcome: &FileReadOutcome) -> String {
    let end_line = outcome
        .lines
        .last()
        .map_or_else(|| outcome.offset.saturating_sub(1), |line| line.number);
    let footer = if outcome.truncated_by_bytes {
        format!(
            "(Output capped. Showing lines {}-{end_line}. Use offset={} to continue.)",
            outcome.offset,
            end_line + 1
        )
    } else if end_line < outcome.total_lines {
        format!(
            "(Showing lines {}-{end_line} of {}. Use offset={} to continue.)",
            outcome.offset,
            outcome.total_lines,
            end_line + 1
        )
    } else {
        format!("(End of file - total {} lines)", outcome.total_lines)
    };
    let body = if outcome.lines.is_empty() {
        footer
    } else {
        let numbered: Vec<String> = outcome
            .lines
            .iter()
            .map(|line| format!("{}: {}", line.number, line.text))
            .collect();
        format!("{}\n\n{footer}", numbered.join("\n"))
    };
    format!("<path>{display_path}</path>\n<type>file</type>\n<content>\n{body}\n</content>")
}

/// Derive the persisted `lang` hint from a read path's file extension. The shared table in
/// `hoh-util-code-language` owns the recognized suffixes and the path rules; this is the read card's
/// short-id projection over it, so a suffix whose value a recorded session already holds keeps it
/// byte-identical while every other suffix uses its language's short name.
pub fn lang_from_path(path: &str) -> Option<&'static str> {
    hoh_util_code_language::read_lang_hint_for_path(path)
}

/// The `read` tool's private `tool/result` `meta` payload: the structured line-numbered window a
/// capable UI renders as a code view. Attached opaquely on the tool result and persisted with the
/// session log, so `present_result` reproduces the read card on replay when the raw structured
/// output is no longer on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsReadMeta {
    /// The read file's model-facing path.
    pub path: String,
    /// The 1-based first line the window requested, kept even when `lines` is empty.
    pub offset: u64,
    /// The returned window's lines, each keeping its file line number.
    pub lines: Vec<FileTextLine>,
    /// Exact total line count in the file.
    pub total_lines: u64,
    /// Syntax-highlighting language hint from the extension, or omitted for plain text.
    pub lang: Option<String>,
}

/// A JSON number that is an integer (`Number.isInteger`), as an `f64`.
fn integer(value: &Value) -> Option<f64> {
    let number = value.as_f64()?;
    (number.is_finite() && number.fract() == 0.0).then_some(number)
}

/// Whether `value` is a valid [`FileTextLine`] (defensive narrowing from opaque `meta`): `number`
/// must be a 1-based integer line number, since a card rendered from a zero, fractional, or
/// non-finite line number would violate the 1-based numbering contract.
fn file_text_line(value: &Value) -> Option<(f64, FileTextLine)> {
    let object = value.as_object()?;
    let number = integer(object.get("number")?)?;
    let text = object.get("text")?.as_str()?;
    if number < 1.0 {
        return None;
    }
    Some((
        number,
        FileTextLine {
            number: number as u64,
            text: text.to_owned(),
        },
    ))
}

/// Narrow opaque live or replayed result metadata to a structured read window. Malformed metadata
/// returns `None` so presentation can fall back to the generic text card instead of failing during
/// replay. Beyond shape, the semantic contract of a read window is enforced against replayed JSON
/// that is well-typed but out of range: `offset` must be a 1-based integer, `totalLines` a
/// non-negative integer, each line number a 1-based integer no less than `offset`, the line numbers
/// strictly increasing, and none past `totalLines`.
pub fn read_meta_from_meta(meta: Option<&Value>) -> Option<FsReadMeta> {
    let object = meta?.as_object()?;
    let path = object.get("path")?.as_str()?;
    let total_lines = object.get("totalLines")?;
    let offset = object.get("offset")?;
    let (Value::Number(_), Value::Number(_)) = (total_lines, offset) else {
        return None;
    };
    let offset = integer(offset)?;
    if offset < 1.0 {
        return None;
    }
    let total_lines = integer(total_lines)?;
    if total_lines < 0.0 {
        return None;
    }
    let raw_lines = object.get("lines")?.as_array()?;
    let lines: Vec<(f64, FileTextLine)> = raw_lines
        .iter()
        .map(file_text_line)
        .collect::<Option<_>>()?;
    let lang = match object.get("lang") {
        None => None,
        Some(Value::String(lang)) => Some(lang.clone()),
        Some(_) => return None,
    };
    let mut previous = offset - 1.0;
    for (number, _) in &lines {
        if *number <= previous || *number > total_lines {
            return None;
        }
        previous = *number;
    }
    Some(FsReadMeta {
        path: path.to_owned(),
        offset: offset as u64,
        lines: lines.into_iter().map(|(_, line)| line).collect(),
        total_lines: total_lines as u64,
        lang,
    })
}
