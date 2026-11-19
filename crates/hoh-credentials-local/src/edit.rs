//! Patching the document text.
//!
//! The source edits the comment-preserving tree of the `yaml` package and prints it back. Here
//! the original text is patched at the spans of the entry being written, so every untouched
//! entry keeps its comments and spelling byte for byte; only the written entry is rendered
//! afresh (see `emit.rs`). A document in flow style is rebuilt as a block document instead.

use hoh_credentials::CredentialRecord;
use hoh_error::BoxError;
use serde_json::{Map, Value};

use crate::document::DOCUMENT_VERSION;
use crate::emit::{StringForm, entry_text, string_form, write_map};
use saphyr_parser::ScalarStyle;

use crate::syntax::{Kind, Node, Yaml, parse_syntax, scalar_value, to_yaml};

/// The new content of one section entry.
enum Entry<'a> {
    /// `key: value` for a reference.
    Ref(&'a str),
    /// `key:` and the record's block for a record.
    Record(&'a CredentialRecord),
}

/// Render the next document text with one reference set or deleted.
pub(crate) fn render_ref(
    text: Option<&str>,
    reference: &str,
    value: Option<&str>,
) -> Result<String, BoxError> {
    edit(text, "refs", reference, value.map(Entry::Ref))
}

/// Render the next document text with one record written or deleted.
pub(crate) fn render_record(
    text: Option<&str>,
    key: &str,
    record: Option<&CredentialRecord>,
) -> Result<String, BoxError> {
    edit(text, "records", key, record.map(Entry::Record))
}

/// Where the explicit document-end marker (`...` at the start of a line) begins, or the text's
/// length when there is none. Only comments can follow it, since a second document is rejected.
fn document_end(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("...")
            && (rest.is_empty() || rest.starts_with([' ', '\t', '\n']))
        {
            return offset;
        }
        offset += line.len();
    }
    text.len()
}

fn line_start(text: &str, index: usize) -> usize {
    text[..index].rfind('\n').map_or(0, |at| at + 1)
}

fn line_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Lines of `text[..end]` from the bottom up, as (start, content-without-newline).
fn previous_line(text: &str, start: usize) -> Option<(usize, &str)> {
    if start == 0 {
        return None;
    }
    let before = &text[..start - 1];
    let begin = before.rfind('\n').map_or(0, |at| at + 1);
    Some((begin, &text[begin..start - 1]))
}

/// Where the comment block directly above the entry whose key starts at `key_start` begins. A
/// comment annotates the entry below it; comments indented deeper than the key are content of the
/// previous entry's block scalar, not annotations.
fn annotation_start(text: &str, key_start: usize) -> usize {
    let key_column = key_start - line_start(text, key_start);
    let mut start = line_start(text, key_start);
    while let Some((begin, line)) = previous_line(text, start) {
        let trimmed = line.trim_start_matches(' ');
        if trimmed.starts_with('#') && line_indent(line) <= key_column {
            start = begin;
        } else {
            break;
        }
    }
    start
}

/// The end of the last content line before `boundary`: blank lines and comment lines at or above
/// the key's own indentation belong to what follows, not to the entry.
fn content_end(text: &str, boundary: usize, key_column: usize) -> usize {
    let mut end = boundary;
    while let Some((begin, line)) = previous_line(text, end) {
        let trimmed = line.trim_start_matches(' ');
        if trimmed.is_empty() || (trimmed.starts_with('#') && line_indent(line) <= key_column) {
            end = begin;
        } else {
            break;
        }
    }
    end
}

struct Located<'a> {
    key: &'a Node,
    value: &'a Node,
    /// Start of the entry's own lines (its annotation excluded).
    start: usize,
    /// Start of the annotation block directly above.
    annotation: usize,
    /// End of the entry's last content line.
    end: usize,
}

/// Locate the entries of a block mapping, each with its own span.
fn locate<'a>(text: &str, entries: &'a [(Node, Node)], parent_end: usize) -> Vec<Located<'a>> {
    (0..entries.len())
        .map(|index| {
            let (key, value) = &entries[index];
            let start = line_start(text, key.range.start);
            let column = key.range.start - start;
            let boundary = entries.get(index + 1).map_or(parent_end, |(next, _)| {
                annotation_start(text, next.range.start)
            });
            Located {
                key,
                value,
                start,
                annotation: annotation_start(text, key.range.start),
                end: content_end(text, boundary, column).max(start),
            }
        })
        .collect()
}

fn key_name(node: &Node) -> Option<String> {
    match scalar_value(node) {
        Some(Yaml::Str(name)) => Some(name),
        _ => None,
    }
}

fn is_block_map(text: &str, node: &Node) -> bool {
    matches!(&node.kind, Kind::Map(entries) if !entries.is_empty())
        && !text[node.range.start..].starts_with('{')
}

/// The text of one entry at `indent` (possibly several lines, newline-terminated).
fn entry_lines(indent: usize, key: &str, entry: &Entry<'_>) -> String {
    match entry {
        Entry::Ref(value) => entry_text(indent, key, &Value::String((*value).to_owned())),
        Entry::Record(record) => {
            let mut out = format!("{}{key}:\n", " ".repeat(indent));
            let fields = match serde_json::to_value(record) {
                Ok(Value::Object(fields)) => fields,
                _ => Map::new(),
            };
            write_map(&mut out, indent + 2, &fields);
            out
        }
    }
}

fn with_final_newline(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn edit(
    text: Option<&str>,
    section: &str,
    key: &str,
    entry: Option<Entry<'_>>,
) -> Result<String, BoxError> {
    let normalized = text.map(|text| with_final_newline(text.replace("\r\n", "\n")));
    let Some(source) = normalized.as_deref() else {
        return Ok(match entry {
            None => String::new(),
            Some(entry) => fresh_document("", section, key, &entry),
        });
    };
    let syntax = parse_syntax(source).map_err(|failure| -> BoxError {
        format!(
            "credentials-local: invalid document: {}",
            failure.describe()
        )
        .into()
    })?;
    let root_entries = match &syntax.root {
        Some(root) if is_block_map(source, root) => match &root.kind {
            Kind::Map(entries) => Some(entries),
            _ => None,
        },
        _ => None,
    };
    let Some(root_entries) = root_entries else {
        // An absent, comment-only or flow-style root.
        return match &syntax.root {
            Some(root) if matches!(&root.kind, Kind::Map(entries) if !entries.is_empty()) => {
                rebuild(source, root, section, key, entry.as_ref())
            }
            Some(root) if !matches!(&root.kind, Kind::Map(_)) && !is_empty_node(root) => {
                Err("credentials-local: the document is not a mapping".into())
            }
            _ => Ok(match entry {
                None => source.to_owned(),
                Some(entry) => {
                    // An empty store: keep the comments above, then the new document.
                    let head = match &syntax.root {
                        Some(root) => &source[..root.range.start],
                        None => source,
                    };
                    fresh_document(head, section, key, &entry)
                }
            }),
        };
    };

    // An explicit `...` ends the document: entries go in before it and deletions never touch it.
    let root_end = document_end(source);
    let located = locate(source, root_entries, root_end);
    let section_entry = located
        .iter()
        .find(|located| key_name(located.key).as_deref() == Some(section));
    let Some(section_entry) = section_entry else {
        return Ok(match entry {
            None => source.to_owned(),
            Some(entry) => {
                // `setIn` appends a missing section at the end of the root mapping.
                let end = located.last().map_or(root_end, |last| last.end);
                format!(
                    "{}{section}:\n{}{}",
                    &source[..end],
                    entry_lines(2, key, &entry),
                    &source[end..]
                )
            }
        });
    };

    let value = section_entry.value;
    let items = match &value.kind {
        Kind::Map(items) if is_block_map(source, value) => Some(items),
        _ => None,
    };
    let Some(items) = items else {
        // A null, empty or flow-style section.
        if is_empty_node(value) || matches!(&value.kind, Kind::Map(items) if items.is_empty()) {
            return Ok(match entry {
                None => source.to_owned(),
                Some(entry) => {
                    let head = &source[..line_start(source, section_entry.key.range.start)];
                    format!(
                        "{head}{section}:\n{}{}",
                        entry_lines(2, key, &entry),
                        &source[section_entry.end..]
                    )
                }
            });
        }
        let root = syntax.root.as_ref().expect("root exists");
        return rebuild(source, root, section, key, entry.as_ref());
    };

    let following = located
        .iter()
        .position(|located| std::ptr::eq(located.key, section_entry.key))
        .and_then(|index| root_entries.get(index + 1))
        .map_or(root_end, |(next, _)| {
            annotation_start(source, next.range.start)
        });
    let rows = locate(source, items, following);
    let found = rows
        .iter()
        .find(|row| key_name(row.key).as_deref() == Some(key));
    let indent = rows
        .first()
        .map_or(2, |first| first.key.range.start - first.start);
    let last_end = rows.last().map_or(section_entry.end, |row| row.end);

    match (found, entry) {
        (None, None) => Ok(source.to_owned()),
        (None, Some(entry)) => Ok(format!(
            "{}{}{}",
            &source[..last_end],
            entry_lines(indent, key, &entry),
            &source[last_end..]
        )),
        (Some(row), Some(entry)) => {
            if let Entry::Ref(value) = &entry
                && let Some(inline) = same_line_value(source, row, value)
            {
                return Ok(format!(
                    "{}{inline}{}",
                    &source[..row.value.range.start],
                    &source[row.value.range.end..]
                ));
            }
            Ok(format!(
                "{}{}{}",
                &source[..row.start],
                entry_lines(indent, key, &entry),
                &source[row.end..]
            ))
        }
        (Some(row), None) => {
            if rows.len() == 1 {
                // The section keeps an empty mapping, as `deleteIn` leaves it.
                let section_start = line_start(source, section_entry.key.range.start);
                let pad = " ".repeat(section_entry.key.range.start - section_start);
                return Ok(format!(
                    "{}{pad}{section}: {{}}\n{}",
                    &source[..section_start],
                    &source[row.end..]
                ));
            }
            Ok(format!(
                "{}{}",
                &source[..row.annotation],
                &source[row.end..]
            ))
        }
    }
}

fn is_empty_node(node: &Node) -> bool {
    matches!(&node.kind, Kind::Scalar { text, .. } if text.is_empty())
}

/// The inline replacement for a reference value that sits on its key's line, when both the old
/// and the new value are single-line scalars; the rest of the line (a trailing comment) stays.
fn same_line_value(text: &str, row: &Located<'_>, value: &str) -> Option<String> {
    let range = &row.value.range;
    if !matches!(&row.value.kind, Kind::Scalar { .. })
        || text[range.clone()].contains('\n')
        || range.start < row.key.range.end
        || text[row.key.range.end..range.start].contains('\n')
        || range.is_empty()
    {
        return None;
    }
    match string_form(value, false) {
        StringForm::Inline(inline) => Some(inline),
        StringForm::Block { .. } => None,
    }
}

/// A new document: `head` (comments of an empty store), then the version stamp and the entry.
fn fresh_document(head: &str, section: &str, key: &str, entry: &Entry<'_>) -> String {
    let head = head.trim_end();
    let mut out = String::new();
    if !head.is_empty() {
        out.push_str(head);
        out.push_str("\n\n");
    }
    out.push_str(&format!("version: {DOCUMENT_VERSION}\n{section}:\n"));
    out.push_str(&entry_lines(2, key, entry));
    out
}

/// Re-render a flow-style document as a block document with the change applied.
fn rebuild(
    text: &str,
    root: &Node,
    section: &str,
    key: &str,
    entry: Option<&Entry<'_>>,
) -> Result<String, BoxError> {
    // A rebuild prints the document afresh, so it is only allowed where nothing can be lost: the
    // `yaml` package of the source keeps comments and exact integers, this printer does not.
    if has_comment(text) || has_unkept_markup(text) || has_unkept_node(root) {
        return Err("credentials-local: the document is written in flow style and holds comments, tags, directives, anchors or document markers, which cannot be kept when it is rewritten; nothing was written. Convert it to block style or edit it by hand".into());
    }
    if !numbers_are_exact(root) {
        return Err("credentials-local: the document is written in flow style and holds a number that cannot be rewritten exactly; nothing was written. Convert it to block style or edit it by hand".into());
    }
    let Yaml::Map(fields) = to_yaml(root).map_err(|failure| -> BoxError {
        format!(
            "credentials-local: invalid document: {}",
            failure.describe()
        )
        .into()
    })?
    else {
        return Err("credentials-local: the document is not a mapping".into());
    };
    let mut document = Map::new();
    for (name, value) in &fields {
        document.insert(name.clone(), to_value(value)?);
    }
    document.insert("version".to_owned(), Value::from(DOCUMENT_VERSION));
    let target = document
        .entry(section.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    if let Value::Object(entries) = target {
        match entry {
            None => {
                entries.shift_remove(key);
            }
            Some(Entry::Ref(value)) => {
                entries.insert(key.to_owned(), Value::String((*value).to_owned()));
            }
            Some(Entry::Record(record)) => {
                entries.insert(
                    key.to_owned(),
                    serde_json::to_value(record).map_err(|error| -> BoxError { error.into() })?,
                );
            }
        }
    }
    let mut out = String::new();
    write_map(&mut out, 0, &document);
    Ok(out)
}

/// Whether any line carries a comment (`#` at the start of a line or after whitespace). Quoted
/// text holding ` #` counts too: refusing a rewrite is the safe side.
fn has_comment(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start().starts_with('#') || line.contains(" #") || line.contains("\t#")
    })
}

/// Whether the text carries a directive (`%YAML`, `%TAG`), an explicit document marker (`---`,
/// `...`) or a tag (`!`, `!!str`, `!<uri>` at the start of a token). A quoted string that merely
/// contains such a token counts too: refusing a rewrite is the safe side.
fn has_unkept_markup(text: &str) -> bool {
    text.lines().any(|line| {
        if line.starts_with(['%']) || line.starts_with("---") || line.starts_with("...") {
            return true;
        }
        let mut previous = None::<char>;
        line.chars().any(|c| {
            let tag = c == '!'
                && previous.is_none_or(|before| before.is_whitespace() || "[{,".contains(before));
            previous = Some(c);
            tag
        })
    })
}

/// Whether the tree holds an anchor or an alias, which a rewrite expands or drops.
fn has_unkept_node(node: &Node) -> bool {
    node.anchor != 0
        || match &node.kind {
            Kind::Alias(_) => true,
            Kind::Scalar { tag, .. } => tag.is_some(),
            Kind::Map(entries) => entries
                .iter()
                .any(|(key, value)| has_unkept_node(key) || has_unkept_node(value)),
            Kind::Seq(items) => items.iter().any(has_unkept_node),
        }
}

/// Whether every plain number in the tree prints back as it is spelled.
fn numbers_are_exact(node: &Node) -> bool {
    match &node.kind {
        Kind::Scalar { text, style, .. } if *style == ScalarStyle::Plain => {
            match scalar_value(node) {
                Some(Yaml::Int(value)) => value.to_string() == *text,
                Some(Yaml::Float(value)) => hoh_util_values::js_number_string(value) == *text,
                _ => true,
            }
        }
        Kind::Map(entries) => entries
            .iter()
            .all(|(key, value)| numbers_are_exact(key) && numbers_are_exact(value)),
        Kind::Seq(items) => items.iter().all(numbers_are_exact),
        _ => true,
    }
}

/// A canonical rendering of everything in the document except the one entry `key` of `section`.
/// Numbers carry their spelling, so a rewrite that rounded one shows up.
fn foreign_fingerprint(text: &str, section: &str, key: &str) -> Option<String> {
    let normalized = with_final_newline(text.replace("\r\n", "\n"));
    let syntax = parse_syntax(&normalized).ok()?;
    let mut out = String::new();
    let mut section_rest = String::new();
    if let Some(root) = &syntax.root {
        match &root.kind {
            Kind::Map(entries) => {
                for (name, value) in entries {
                    if key_name(name).as_deref() == Some(section)
                        && let Kind::Map(items) = &value.kind
                    {
                        for (item, body) in items {
                            if key_name(item).as_deref() != Some(key) {
                                fingerprint(item, &mut section_rest);
                                fingerprint(body, &mut section_rest);
                            }
                        }
                        continue;
                    }
                    if key_name(name).as_deref() == Some(section) && is_empty_node(value) {
                        continue;
                    }
                    fingerprint(name, &mut out);
                    fingerprint(value, &mut out);
                }
            }
            _ => fingerprint(root, &mut out),
        }
    }
    Some(format!("{out}|{section_rest}"))
}

fn fingerprint(node: &Node, out: &mut String) {
    use std::fmt::Write as _;
    let anchor = node.anchor;
    match &node.kind {
        Kind::Scalar { text, style, tag } => {
            let value = scalar_value(node);
            let spelled = match (&value, style) {
                (Some(Yaml::Int(_) | Yaml::Float(_)), ScalarStyle::Plain) => text.as_str(),
                _ => "",
            };
            let _ = write!(out, "&{anchor}S{tag:?}{value:?}{spelled};");
        }
        Kind::Map(entries) => {
            let _ = write!(out, "&{anchor}{{");
            for (name, value) in entries {
                fingerprint(name, out);
                fingerprint(value, out);
            }
            out.push('}');
        }
        Kind::Seq(items) => {
            let _ = write!(out, "&{anchor}[");
            for item in items {
                fingerprint(item, out);
            }
            out.push(']');
        }
        Kind::Alias(id) => {
            let _ = write!(out, "*{id};");
        }
    }
}

/// Whether the rewritten document keeps everything this provider does not manage exactly as the
/// previous one had it: other top-level fields, the other entries of the section, number spelling.
pub(crate) fn foreign_content_preserved(
    previous: Option<&str>,
    next: &str,
    section: &str,
    key: &str,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    match (
        foreign_fingerprint(previous, section, key),
        foreign_fingerprint(next, section, key),
    ) {
        (Some(before), Some(after)) => before == after,
        _ => false,
    }
}

fn to_value(value: &Yaml) -> Result<Value, BoxError> {
    Ok(match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => Value::Bool(*value),
        Yaml::Int(value) => Value::from(*value),
        Yaml::Float(value) if value.is_finite() => {
            serde_json::Number::from_f64(*value).map_or(Value::Null, Value::Number)
        }
        Yaml::Float(_) | Yaml::Cycle => {
            return Err(
                "credentials-local: the document holds a value that cannot be rewritten".into(),
            );
        }
        Yaml::Str(value) => Value::String(value.clone()),
        Yaml::Seq(items) => Value::Array(items.iter().map(to_value).collect::<Result<_, _>>()?),
        Yaml::Map(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| Ok((key.clone(), to_value(value)?)))
                .collect::<Result<_, BoxError>>()?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(text: Option<&str>, key: &str, value: &str) -> String {
        render_ref(text, key, Some(value)).unwrap()
    }

    #[test]
    fn creates_a_fresh_document() {
        assert_eq!(
            set(None, "DSH_CRED_TEST", "sk-fresh"),
            "version: 1\nrefs:\n  DSH_CRED_TEST: sk-fresh\n"
        );
    }

    #[test]
    fn patches_one_entry_and_keeps_the_rest() {
        let text = "version: 1\nrefs:\n  # deployment notes\n  DSH_CRED_OTHER: keep\n\n  # the one under edit\n  DSH_CRED_TEST: old\n";
        assert_eq!(
            set(Some(text), "DSH_CRED_TEST", "new value!"),
            "version: 1\nrefs:\n  # deployment notes\n  DSH_CRED_OTHER: keep\n\n  # the one under edit\n  DSH_CRED_TEST: new value!\n"
        );
    }

    #[test]
    fn appends_after_the_last_entry_and_keeps_unknown_top_level_keys() {
        let text = "version: 1\nGITLAB_TOKEN: glpat-secret\nACCOUNT_ID: 9007199254740993\nrefs:\n  DSH_CRED_TEST: sk-file\n";
        assert_eq!(
            set(Some(text), "DSH_CRED_OTHER", "sk-other"),
            "version: 1\nGITLAB_TOKEN: glpat-secret\nACCOUNT_ID: 9007199254740993\nrefs:\n  DSH_CRED_TEST: sk-file\n  DSH_CRED_OTHER: sk-other\n"
        );
    }

    #[test]
    fn deletes_an_entry_with_its_annotation() {
        let text = "version: 1\nrefs:\n  # about the doomed one\n  DSH_CRED_TEST: gone\n  # about the survivor\n  DSH_CRED_OTHER: stays\n";
        assert_eq!(
            render_ref(Some(text), "DSH_CRED_TEST", None).unwrap(),
            "version: 1\nrefs:\n  # about the survivor\n  DSH_CRED_OTHER: stays\n"
        );
    }

    #[test]
    fn leaves_an_empty_mapping_after_the_only_entry() {
        assert_eq!(
            render_ref(
                Some("version: 1\nrefs:\n  DSH_CRED_TEST: only\n"),
                "DSH_CRED_TEST",
                None
            )
            .unwrap(),
            "version: 1\nrefs: {}\n"
        );
    }

    #[test]
    fn leaves_a_sibling_block_scalar_alone() {
        let text = "version: 1\nrefs:\n  DSH_REVIEW_WRAPPED: |-\n    line1\n    line2\n  DSH_REVIEW_ALPHA: a\n";
        assert_eq!(
            set(Some(text), "DSH_REVIEW_ALPHA", "b"),
            "version: 1\nrefs:\n  DSH_REVIEW_WRAPPED: |-\n    line1\n    line2\n  DSH_REVIEW_ALPHA: b\n"
        );
    }

    #[test]
    fn writes_the_records_section_after_the_refs() {
        let text = "version: 1\nrefs:\n  DSH_RECORDS_KEY: sk-live\n";
        let record = CredentialRecord::Grant(hoh_credentials::GrantRecord {
            payload: serde_json::json!({"token": "t"}),
        });
        assert_eq!(
            render_record(Some(text), "llm-pi-ai/openai-codex", Some(&record)).unwrap(),
            "version: 1\nrefs:\n  DSH_RECORDS_KEY: sk-live\nrecords:\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload:\n      token: t\n"
        );
    }

    #[test]
    fn keeps_the_comments_of_an_empty_store() {
        assert_eq!(
            set(Some("# nothing stored yet\n"), "DSH_CRED_TEST", "v"),
            "# nothing stored yet\n\nversion: 1\nrefs:\n  DSH_CRED_TEST: v\n"
        );
    }

    const MARKED: &str = "---\nversion: 1\nrefs:\n  A: old\n...\n";

    #[test]
    fn inserts_before_an_explicit_document_end() {
        assert_eq!(
            set(Some(MARKED), "B", "new"),
            "---\nversion: 1\nrefs:\n  A: old\n  B: new\n...\n"
        );
        assert_eq!(
            set(Some(MARKED), "A", "newer"),
            "---\nversion: 1\nrefs:\n  A: newer\n...\n"
        );
        let record = CredentialRecord::Grant(hoh_credentials::GrantRecord {
            payload: serde_json::json!(1),
        });
        assert_eq!(
            render_record(Some(MARKED), "llm-pi-ai/codex", Some(&record)).unwrap(),
            "---\nversion: 1\nrefs:\n  A: old\nrecords:\n  llm-pi-ai/codex:\n    kind: grant\n    payload: 1\n...\n"
        );
    }

    #[test]
    fn deletes_without_touching_an_explicit_document_end() {
        let two = "---\nversion: 1\nrefs:\n  A: old\n  B: x\n# trailing note\n...\n";
        assert_eq!(
            render_ref(Some(two), "B", None).unwrap(),
            "---\nversion: 1\nrefs:\n  A: old\n# trailing note\n...\n"
        );
        assert_eq!(
            render_ref(Some(MARKED), "A", None).unwrap(),
            "---\nversion: 1\nrefs: {}\n...\n"
        );
    }

    #[test]
    fn refuses_a_flow_rewrite_that_would_drop_comments_or_round_a_number() {
        let commented = "{version: 1, ACCOUNT_ID: 7, refs: {A: x}} # keep\n";
        let failure = render_ref(Some(commented), "B", Some("y")).unwrap_err();
        assert!(failure.to_string().contains("comments"), "{failure}");
        for tagged in [
            "{version: 1, ACCOUNT_ID: !!str 7, refs: {A: x}}\n",
            "{version: 1, OWNER: !custom x, refs: {A: x}}\n",
            "{version: 1, refs: !!map {A: x}}\n",
            "%YAML 1.2\n---\n{version: 1, refs: {A: x}}\n",
            "---\n{version: 1, refs: {A: x}}\n...\n",
            "{version: 1, base: &b 1, copy: *b, refs: {A: x}}\n",
        ] {
            let failure = render_ref(Some(tagged), "B", Some("y")).unwrap_err();
            assert!(
                failure.to_string().contains("nothing was written"),
                "{tagged:?}: {failure}"
            );
        }
        let big = "{version: 1, ACCOUNT_ID: 18446744073709551615, refs: {A: x}}\n";
        let failure = render_ref(Some(big), "B", Some("y")).unwrap_err();
        assert!(failure.to_string().contains("number"), "{failure}");
        assert!(!failure.to_string().contains("18446744073709551615"));
    }

    #[test]
    fn still_rewrites_a_flow_document_when_nothing_can_be_lost() {
        assert_eq!(
            set(Some("{version: 1, refs: {A: x}}\n"), "B", "y"),
            "version: 1\nrefs:\n  A: x\n  B: y\n"
        );
    }

    #[test]
    fn notices_a_rewrite_that_changed_what_the_provider_does_not_manage() {
        let before = "version: 1\nACCOUNT_ID: 18446744073709551615\nrefs:\n  A: x\n";
        let kept = "version: 1\nACCOUNT_ID: 18446744073709551615\nrefs:\n  A: x\n  B: y\n";
        let rounded = "version: 1\nACCOUNT_ID: 18446744073709552000\nrefs:\n  A: x\n  B: y\n";
        let lost = "version: 1\nrefs:\n  A: x\n  B: y\n";
        let sibling = "version: 1\nACCOUNT_ID: 18446744073709551615\nrefs:\n  A: z\n  B: y\n";
        assert!(foreign_content_preserved(Some(before), kept, "refs", "B"));
        assert!(!foreign_content_preserved(
            Some(before),
            rounded,
            "refs",
            "B"
        ));
        assert!(!foreign_content_preserved(Some(before), lost, "refs", "B"));
        assert!(!foreign_content_preserved(
            Some(before),
            sibling,
            "refs",
            "B"
        ));
        assert!(foreign_content_preserved(None, lost, "refs", "B"));
    }
}
