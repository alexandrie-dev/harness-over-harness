//! YAML syntax tree with source spans, and the typed values read from it.
//!
//! The source reads the document with `parseDocument(text, { prettyErrors, uniqueKeys })` of the
//! `yaml` package and edits the same tree. Rust has no document API that keeps comments, so the
//! tree here keeps byte ranges instead and `edit.rs` patches the original text at those ranges.
//! Plain scalars are typed by the YAML 1.2 core schema, the schema `yaml` 2 applies by default.

use std::ops::Range;

use saphyr_parser::{Event, Parser, ScalarStyle};

/// One node of the syntax tree.
#[derive(Debug)]
pub(crate) struct Node {
    /// Byte range of the node in the text (a block scalar's range is whatever the parser reports).
    pub range: Range<usize>,
    pub kind: Kind,
    /// Anchor id (0: none).
    pub anchor: usize,
}

#[derive(Debug)]
pub(crate) enum Kind {
    Scalar {
        text: String,
        style: ScalarStyle,
        /// The explicit core-schema tag suffix (`str`, `int`, ...), when the scalar carries one.
        tag: Option<String>,
    },
    Map(Vec<(Node, Node)>),
    Seq(Vec<Node>),
    Alias(usize),
}

/// One parsed document: the root node (absent for an empty or comment-only text).
#[derive(Debug)]
pub(crate) struct Syntax {
    pub root: Option<Node>,
}

/// A parse failure carrying only the error code and position: the parser's own message can quote
/// the offending source line, and in this document that line is a secret.
#[derive(Debug)]
pub(crate) struct SyntaxError {
    pub code: &'static str,
    pub line: usize,
    pub column: usize,
}

impl SyntaxError {
    /// `<CODE> at line L, column C` — the form `describeYamlError` of the source prints.
    pub fn describe(&self) -> String {
        format!(
            "{} at line {}, column {}",
            self.code, self.line, self.column
        )
    }
}

struct Builder<'a> {
    events: std::vec::IntoIter<(Event<'a>, saphyr_parser::Span)>,
    /// Char index → byte offset, with the text length as the final entry.
    bytes: Vec<usize>,
}

impl Builder<'_> {
    fn byte(&self, char_index: usize) -> usize {
        self.bytes[char_index.min(self.bytes.len() - 1)]
    }

    fn node(&mut self) -> Result<Node, SyntaxError> {
        let (event, span) = self.events.next().ok_or(SyntaxError {
            code: "UNEXPECTED_TOKEN",
            line: 1,
            column: 1,
        })?;
        let start = self.byte(span.start.index());
        let mut end = self.byte(span.end.index());
        let (kind, anchor) = match event {
            Event::Scalar(text, style, anchor, tag) => (
                Kind::Scalar {
                    text: text.into_owned(),
                    style,
                    tag: tag
                        .filter(|tag| tag.is_yaml_core_schema())
                        .map(|tag| tag.suffix.clone()),
                },
                anchor,
            ),
            Event::Alias(id) => (Kind::Alias(id), 0),
            Event::MappingStart(anchor, _) => {
                let mut entries = Vec::new();
                loop {
                    if matches!(self.events.as_slice().first(), Some((Event::MappingEnd, _))) {
                        let (_, closing) = self.events.next().expect("peeked");
                        end = self.byte(closing.end.index());
                        break;
                    }
                    let key = self.node()?;
                    let value = self.node()?;
                    entries.push((key, value));
                }
                (Kind::Map(entries), anchor)
            }
            Event::SequenceStart(anchor, _) => {
                let mut items = Vec::new();
                loop {
                    if matches!(
                        self.events.as_slice().first(),
                        Some((Event::SequenceEnd, _))
                    ) {
                        let (_, closing) = self.events.next().expect("peeked");
                        end = self.byte(closing.end.index());
                        break;
                    }
                    items.push(self.node()?);
                }
                (Kind::Seq(items), anchor)
            }
            _ => {
                return Err(SyntaxError {
                    code: "UNEXPECTED_TOKEN",
                    line: span.start.line(),
                    column: span.start.col() + 1,
                });
            }
        };
        Ok(Node {
            range: start..end.max(start),
            kind,
            anchor,
        })
    }
}

/// Parse `text` into a syntax tree. Several documents, parse errors and duplicate keys fail.
pub(crate) fn parse_syntax(text: &str) -> Result<Syntax, SyntaxError> {
    let mut events = Vec::new();
    let mut documents = 0;
    for event in Parser::new_from_str(text) {
        let (event, span) = event.map_err(|error| {
            // The `yaml` package reports an unterminated quoted scalar where the input ends; the
            // scanner here reports where the quote opened.
            if error
                .info()
                .contains("quoted scalar, found unexpected end of stream")
            {
                let line = text.matches('\n').count() + 1;
                let column = text
                    .rsplit('\n')
                    .next()
                    .map_or(0, |tail| tail.chars().count())
                    + 1;
                return SyntaxError {
                    code: "YAML_PARSE_ERROR",
                    line,
                    column,
                };
            }
            SyntaxError {
                code: "YAML_PARSE_ERROR",
                line: error.marker().line(),
                column: error.marker().col() + 1,
            }
        })?;
        match event {
            Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => {}
            Event::DocumentStart(_) => {
                documents += 1;
                if documents > 1 {
                    return Err(SyntaxError {
                        code: "MULTIPLE_DOCS",
                        line: span.start.line(),
                        column: span.start.col() + 1,
                    });
                }
            }
            event => events.push((event, span)),
        }
    }
    if events.is_empty() {
        return Ok(Syntax { root: None });
    }
    let mut bytes: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    bytes.push(text.len());
    let mut builder = Builder {
        events: events.into_iter(),
        bytes,
    };
    let root = builder.node()?;
    let syntax = Syntax { root: Some(root) };
    check_unique_keys(syntax.root.as_ref().expect("just set"), text)?;
    Ok(syntax)
}

fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rfind('\n').map_or(before.chars().count(), |index| {
        before[index + 1..].chars().count()
    }) + 1;
    (line, column)
}

/// `uniqueKeys: true`: two scalar keys with one resolved value are a `DUPLICATE_KEY` error.
fn check_unique_keys(node: &Node, text: &str) -> Result<(), SyntaxError> {
    match &node.kind {
        Kind::Map(entries) => {
            let mut seen: Vec<Option<Yaml>> = Vec::new();
            for (key, value) in entries {
                if let Some(resolved) = scalar_value(key) {
                    if seen
                        .iter()
                        .any(|earlier| earlier.as_ref() == Some(&resolved))
                    {
                        let (line, column) = position(text, key.range.start);
                        return Err(SyntaxError {
                            code: "DUPLICATE_KEY",
                            line,
                            column,
                        });
                    }
                    seen.push(Some(resolved));
                }
                check_unique_keys(key, text)?;
                check_unique_keys(value, text)?;
            }
            Ok(())
        }
        Kind::Seq(items) => items
            .iter()
            .try_for_each(|item| check_unique_keys(item, text)),
        _ => Ok(()),
    }
}

/// A value as `toJS()` of the source yields it, before JSON admission.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Yaml {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<Yaml>),
    /// Mapping entries in document order; keys are `String(key)` of the source.
    Map(Vec<(String, Yaml)>),
    /// An alias that refers to a node still being built: a cycle.
    Cycle,
}

/// The typed value of a scalar node (`None` for a collection or an alias).
pub(crate) fn scalar_value(node: &Node) -> Option<Yaml> {
    match &node.kind {
        Kind::Scalar { text, style, tag } => Some(resolve_scalar(text, *style, tag.as_deref())),
        _ => None,
    }
}

fn resolve_scalar(text: &str, style: ScalarStyle, tag: Option<&str>) -> Yaml {
    if let Some(tag) = tag {
        match tag {
            "str" => return Yaml::Str(text.to_owned()),
            "null" => return Yaml::Null,
            "bool" => {
                if let Some(value) = resolve_bool(text) {
                    return Yaml::Bool(value);
                }
            }
            "int" => {
                if let Some(value) = resolve_int(text) {
                    return value;
                }
            }
            "float" => {
                if let Some(value) = resolve_float(text) {
                    return value;
                }
            }
            _ => {}
        }
    }
    if !matches!(style, ScalarStyle::Plain) {
        return Yaml::Str(text.to_owned());
    }
    resolve_plain(text)
}

/// A plain scalar's value under the YAML 1.2 core schema.
pub(crate) fn resolve_plain(text: &str) -> Yaml {
    if matches!(text, "" | "~" | "null" | "Null" | "NULL") {
        return Yaml::Null;
    }
    if let Some(value) = resolve_bool(text) {
        return Yaml::Bool(value);
    }
    if let Some(value) = resolve_int(text) {
        return value;
    }
    if let Some(value) = resolve_float(text) {
        return value;
    }
    Yaml::Str(text.to_owned())
}

fn resolve_bool(text: &str) -> Option<bool> {
    match text {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

fn resolve_int(text: &str) -> Option<Yaml> {
    let radix = |digits: &str, radix: u32| -> Option<Yaml> {
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        Some(
            i64::from_str_radix(digits, radix)
                .map_or_else(|_| Yaml::Float(radix_to_f64(digits, radix)), Yaml::Int),
        )
    };
    if let Some(digits) = text.strip_prefix("0o") {
        return radix(digits, 8);
    }
    if let Some(digits) = text.strip_prefix("0x") {
        return radix(digits, 16);
    }
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    if unsigned.is_empty() || !unsigned.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(
        text.strip_prefix('+')
            .unwrap_or(text)
            .parse::<i64>()
            .map_or_else(
                |_| Yaml::Float(text.parse::<f64>().unwrap_or(f64::NAN)),
                Yaml::Int,
            ),
    )
}

fn radix_to_f64(digits: &str, radix: u32) -> f64 {
    digits.chars().fold(0.0, |value, digit| {
        value * f64::from(radix) + f64::from(digit.to_digit(radix).unwrap_or(0))
    })
}

fn resolve_float(text: &str) -> Option<Yaml> {
    match text {
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Some(Yaml::Float(f64::INFINITY));
        }
        "-.inf" | "-.Inf" | "-.INF" => return Some(Yaml::Float(f64::NEG_INFINITY)),
        ".nan" | ".NaN" | ".NAN" => return Some(Yaml::Float(f64::NAN)),
        _ => {}
    }
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(index) => (&body[..index], Some(&body[index + 1..])),
        None => (body, None),
    };
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    let valid = match mantissa.split_once('.') {
        Some((whole, fraction)) => {
            let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
            digits(whole)
                && digits(fraction)
                && (!fraction.is_empty() || !whole.is_empty())
                && (!whole.is_empty() || !fraction.is_empty())
        }
        None => !mantissa.is_empty() && mantissa.bytes().all(|b| b.is_ascii_digit()),
    };
    if !valid {
        return None;
    }
    // `1e3` without a dot is a float in the core schema only when it has an exponent.
    if !mantissa.contains('.') && exponent.is_none() {
        return None;
    }
    text.strip_prefix('+')
        .unwrap_or(text)
        .parse::<f64>()
        .ok()
        .map(Yaml::Float)
}

/// `String(value)` of the source: the form a non-string mapping key takes in `toJS()`.
fn key_string(value: &Yaml) -> String {
    match value {
        Yaml::Null => "null".to_owned(),
        Yaml::Bool(value) => value.to_string(),
        Yaml::Int(value) => value.to_string(),
        Yaml::Float(value) => hoh_util_values::js_number_string(*value),
        Yaml::Str(value) => value.clone(),
        Yaml::Seq(items) => items.iter().map(key_string).collect::<Vec<_>>().join(","),
        Yaml::Map(_) | Yaml::Cycle => "[object Object]".to_owned(),
    }
}

/// Convert a syntax tree to typed values, resolving anchors and aliases; an alias to a node that
/// is still open is a cycle.
pub(crate) fn to_yaml(node: &Node) -> Result<Yaml, SyntaxError> {
    let mut anchors = std::collections::HashMap::new();
    let mut open = std::collections::HashSet::new();
    convert(node, &mut anchors, &mut open)
}

fn convert(
    node: &Node,
    anchors: &mut std::collections::HashMap<usize, Yaml>,
    open: &mut std::collections::HashSet<usize>,
) -> Result<Yaml, SyntaxError> {
    if node.anchor != 0 {
        open.insert(node.anchor);
    }
    let value = match &node.kind {
        Kind::Scalar { .. } => scalar_value(node).expect("scalar"),
        Kind::Alias(id) => {
            if open.contains(id) {
                Yaml::Cycle
            } else if let Some(value) = anchors.get(id) {
                value.clone()
            } else {
                return Err(SyntaxError {
                    code: "BAD_ALIAS",
                    line: 1,
                    column: 1,
                });
            }
        }
        Kind::Seq(items) => Yaml::Seq(
            items
                .iter()
                .map(|item| convert(item, anchors, open))
                .collect::<Result<_, _>>()?,
        ),
        Kind::Map(entries) => {
            let mut converted: Vec<(String, Yaml)> = Vec::new();
            for (key, value) in entries {
                let name = key_string(&convert(key, anchors, open)?);
                let value = convert(value, anchors, open)?;
                // JavaScript objects keep one entry per key; a later duplicate replaces the value.
                match converted.iter_mut().find(|(existing, _)| *existing == name) {
                    Some(slot) => slot.1 = value,
                    None => converted.push((name, value)),
                }
            }
            Yaml::Map(converted)
        }
    };
    if node.anchor != 0 {
        open.remove(&node.anchor);
        anchors.insert(node.anchor, value.clone());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(text: &str) -> String {
        match parse_syntax(text) {
            Ok(syntax) => format!("{:#?}", syntax.root),
            Err(error) => format!("ERR {}", error.describe()),
        }
    }

    #[test]
    fn spans_probe() {
        for text in [
            "version: 1\nrefs:\n  A: plain\n  B: \"quoted\"\n  C: |-\n    l1\n    l2\n  D: x # c\n",
            "DSH_CRED_TEST: \"unterminated\n",
            "refs: {}\n",
            "a: !!str 1\n",
        ] {
            eprintln!("---- {text:?}\n{}", dump(text));
        }
    }
}
