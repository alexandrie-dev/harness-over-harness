//! Lazily scanned view of one JSON object's top-level fields, built from text that may still
//! be streaming or from an already parsed object. Nothing is scanned until a reader asks; the
//! view remembers every question it answered and reports changed answers when the owner
//! refreshes for publication.
//!
//! All offsets and lengths are UTF-16 code units, as in the source. Text answers can therefore
//! contain a lone surrogate (a prefix cut inside a surrogate pair); the `*_utf16` readers return
//! the exact units and the `String` readers replace such units with U+FFFD.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::{Map, Number, Value};
use thiserror::Error;

use crate::JsonValue;

/// Granularity of a length read; a change is reported only when the rounded-up step moves.
#[derive(Debug, Clone, Copy, Default)]
pub struct LengthReadOptions {
    /// Characters per step; defaults to 1 (every character counts).
    pub step: Option<f64>,
    /// Completed characters included in the displayed total; affects change detection only.
    pub offset: Option<f64>,
}

/// Appending to a view that was sealed by `from_text`, `from_object`, `empty` or `settle`.
#[derive(Debug, Error)]
#[error("PartialArguments: cannot append to a sealed view")]
pub struct SealedViewError;

/// Scanner position inside the object text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Root,
    KeyOrEnd,
    KeyOnly,
    Key,
    Colon,
    Value,
    String,
    Scalar,
    Nested,
    CommaOrEnd,
    Closed,
    Invalid,
}

/// A content reader advances independently of the boundary scanner.
#[derive(Debug)]
struct StringRead {
    at: usize,
    length: usize,
    text: Vec<u16>,
}

impl StringRead {
    fn starting_at(at: usize) -> Self {
        Self {
            at,
            length: 0,
            text: Vec::new(),
        }
    }
}

/// Raw ranges exclude string quotes and include a non-string value's own brackets.
#[derive(Debug)]
struct StrEntry {
    start: usize,
    end: Option<usize>,
    needs_decoding: bool,
    invalid_at: Option<usize>,
    length: Option<StringRead>,
    text: Option<StringRead>,
    prefixes: HashMap<usize, StringRead>,
}

#[derive(Debug)]
struct ValEntry {
    start: usize,
    end: Option<usize>,
    parsed: Option<Value>,
    invalid: bool,
    /// Stands in for the object identity of the parsed value in change detection.
    serial: u64,
}

#[derive(Debug)]
enum Entry {
    Str(StrEntry),
    Val(ValEntry),
}

/// One kind of question; with its key it identifies a remembered read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ReadSpec {
    Closed,
    Keys,
    Has(String),
    Complete(String),
    Length { key: String, step: u64, offset: u64 },
    Exceeds { key: String, limit: usize },
    Text(String),
    Prefix { key: String, limit: usize },
    Value(String),
}

/// The answer to a question, before it is narrowed to the public return type.
enum Raw {
    Bool(bool),
    Keys(Vec<String>),
    Length(Option<usize>),
    Text(Option<Vec<u16>>),
    Value(Option<(Value, u64)>),
}

impl Raw {
    fn into_bool(self) -> bool {
        matches!(self, Raw::Bool(true))
    }

    fn into_keys(self) -> Vec<String> {
        if let Raw::Keys(keys) = self {
            keys
        } else {
            Vec::new()
        }
    }

    fn into_length(self) -> Option<usize> {
        if let Raw::Length(length) = self {
            length
        } else {
            None
        }
    }

    fn into_text(self) -> Option<Vec<u16>> {
        if let Raw::Text(text) = self {
            text
        } else {
            None
        }
    }

    fn into_value(self) -> Option<Value> {
        if let Raw::Value(value) = self {
            value.map(|(value, _)| value)
        } else {
            None
        }
    }
}

/// What change detection compares: the answer, or the value derived from it for the spec.
#[derive(Debug)]
enum Cmp {
    Bool(bool),
    Num(Option<f64>),
    Text(Option<Vec<u16>>),
    Value(Option<(Value, u64)>),
}

fn object_is(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || (a == b && a.is_sign_negative() == b.is_sign_negative())
}

impl Cmp {
    /// The `Object.is` comparison of the source: containers by identity, everything else by value.
    fn same(&self, other: &Cmp) -> bool {
        match (self, other) {
            (Cmp::Bool(a), Cmp::Bool(b)) => a == b,
            (Cmp::Num(None), Cmp::Num(None)) => true,
            (Cmp::Num(Some(a)), Cmp::Num(Some(b))) => object_is(*a, *b),
            (Cmp::Text(a), Cmp::Text(b)) => a == b,
            (Cmp::Value(None), Cmp::Value(None)) => true,
            (Cmp::Value(Some((a, sa))), Cmp::Value(Some((b, sb)))) => {
                if matches!(a, Value::Array(_) | Value::Object(_))
                    || matches!(b, Value::Array(_) | Value::Object(_))
                {
                    sa == sb
                } else {
                    a == b
                }
            }
            _ => false,
        }
    }
}

fn comparison(spec: &ReadSpec, raw: &Raw) -> Cmp {
    match (spec, raw) {
        (ReadSpec::Keys, Raw::Keys(keys)) => Cmp::Num(Some(keys.len() as f64)),
        (ReadSpec::Length { step, offset, .. }, Raw::Length(length)) => {
            let step = f64::from_bits(*step);
            let offset = f64::from_bits(*offset);
            Cmp::Num(length.map(|length| ((length as f64 + offset) / step).ceil()))
        }
        (_, Raw::Bool(flag)) => Cmp::Bool(*flag),
        (_, Raw::Text(text)) => Cmp::Text(text.clone()),
        (_, Raw::Value(value)) => Cmp::Value(value.clone()),
        (_, Raw::Keys(keys)) => Cmp::Num(Some(keys.len() as f64)),
        (_, Raw::Length(length)) => Cmp::Num(length.map(|length| length as f64)),
    }
}

/// One answered question, kept to detect whether later text changes the answer.
#[derive(Debug)]
struct Read {
    completion: bool,
    last: Cmp,
}

const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

fn is_whitespace(c: u16) -> bool {
    c == b' ' as u16 || c == b'\n' as u16 || c == b'\r' as u16 || c == b'\t' as u16
}

fn hex_value(c: u16) -> Option<u16> {
    u8::try_from(c)
        .ok()
        .and_then(|byte| char::from(byte).to_digit(16))
        .map(|digit| digit as u16)
}

/// `[\\\u0000-\u001f]`: a backslash or an unescaped control character.
fn is_content_escape(c: u16) -> bool {
    c == BACKSLASH || c < 0x20
}

fn simple_escape(c: u16) -> Option<u16> {
    match u8::try_from(c).ok()? {
        b'"' => Some(QUOTE),
        b'\\' => Some(BACKSLASH),
        b'/' => Some(b'/' as u16),
        b'b' => Some(0x08),
        b'f' => Some(0x0c),
        b'n' => Some(b'\n' as u16),
        b'r' => Some(b'\r' as u16),
        b't' => Some(b'\t' as u16),
        _ => None,
    }
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// What `JSON.parse('"' + body + '"')` produces, or `None` where it throws. Lone surrogate
/// escapes are kept, as in JavaScript.
fn decode_json_string(body: &[u16]) -> Option<Vec<u16>> {
    let mut out = Vec::with_capacity(body.len());
    let mut index = 0;
    while index < body.len() {
        let c = body[index];
        if c < 0x20 {
            return None;
        }
        if c != BACKSLASH {
            out.push(c);
            index += 1;
            continue;
        }
        let kind = *body.get(index + 1)?;
        if kind == b'u' as u16 {
            let digits = body.get(index + 2..index + 6)?;
            let mut unit = 0u16;
            for digit in digits {
                unit = unit * 16 + hex_value(*digit)?;
            }
            out.push(unit);
            index += 6;
        } else {
            out.push(simple_escape(kind)?);
            index += 2;
        }
    }
    Some(out)
}

/// JavaScript has one number type: an integral float prints as an integer, so it is stored as one.
fn normalize_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if number.is_f64()
                && let Some(float) = number.as_f64()
                && float.fract() == 0.0
                && float.abs() < 9_007_199_254_740_992.0
            {
                *number = Number::from(float as i64);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_numbers),
        Value::Object(map) => map.values_mut().for_each(normalize_numbers),
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

/// `JSON.parse` of a slice of the source text; `None` where it throws.
fn parse_json(units: &[u16]) -> Option<Value> {
    let text = String::from_utf16(units).ok()?;
    let mut value: Value = serde_json::from_str(&text).ok()?;
    normalize_numbers(&mut value);
    Some(value)
}

/// The view. A streaming instance grows through [`PartialArguments::append`];
/// [`PartialArguments::from_text`] and [`PartialArguments::from_object`] build sealed instances
/// over a finished call. Every reader is total: an absent or differently typed field answers
/// `None` (or `false`), never fails.
///
/// Equality compares the source only (fragments, parsed object, sealed flag), never the caches.
#[derive(Debug)]
pub struct PartialArguments {
    chunks: Vec<Arc<[u16]>>,
    object: Option<Map<String, Value>>,
    sealed: bool,
    // Scan progress, located fields, and remembered reads are caches over the source.
    ends: Vec<usize>,
    size: usize,
    consumed: usize,
    mode: Mode,
    escape: bool,
    key_start: usize,
    key_escaped: bool,
    key: Vec<u16>,
    nested_ends: Vec<u16>,
    nested_in_string: bool,
    invalid_at: Option<usize>,
    invalid_value: bool,
    entries: HashMap<Vec<u16>, Entry>,
    order: Vec<Vec<u16>>,
    reads: IndexMap<ReadSpec, Read>,
    next_serial: u64,
    parse_calls: usize,
}

impl PartialEq for PartialArguments {
    fn eq(&self, other: &Self) -> bool {
        self.chunks == other.chunks && self.object == other.object && self.sealed == other.sealed
    }
}

impl Default for PartialArguments {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialArguments {
    /// An empty streaming view.
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            object: None,
            sealed: false,
            ends: Vec::new(),
            size: 0,
            consumed: 0,
            mode: Mode::Root,
            escape: false,
            key_start: 0,
            key_escaped: false,
            key: Vec::new(),
            nested_ends: Vec::new(),
            nested_in_string: false,
            invalid_at: None,
            invalid_value: false,
            entries: HashMap::new(),
            order: Vec::new(),
            reads: IndexMap::new(),
            next_serial: 0,
            parse_calls: 0,
        }
    }

    /// The view of a call with no arguments available (`PartialArguments.EMPTY`). Every call
    /// returns its own sealed view, because readers need exclusive access.
    pub fn empty() -> Self {
        Self::from_object(&Value::Object(Map::new()))
    }

    /// View finished argument text without scanning it until a reader asks.
    pub fn from_text(text: &str) -> Self {
        let mut view = Self::new();
        view.push(utf16(text));
        view.sealed = true;
        view
    }

    /// View an already parsed argument payload; a non-object payload has no fields.
    pub fn from_object(value: &Value) -> Self {
        let mut view = Self::new();
        view.object = Some(match value {
            Value::Object(map) => map.clone(),
            _ => Map::new(),
        });
        view.sealed = true;
        view
    }

    /// Whether this view rejects further appends; does not scan text or register reads.
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// Whether indexing or a content read found invalid JSON; unread value contents are not validated.
    pub fn invalid(&mut self) -> bool {
        self.scan();
        self.mode == Mode::Invalid || self.invalid_value
    }

    /// Number of JSON text decodes made so far (`JSON.parse` calls in the source). Exists so the
    /// "nothing is parsed until requested" tests of the source can be checked.
    #[doc(hidden)]
    pub fn json_parse_count(&self) -> usize {
        self.parse_calls
    }

    /// Retain streamed argument text without scanning or comparing observed answers.
    pub fn append(&mut self, fragment: &str) -> Result<(), SealedViewError> {
        if self.sealed {
            return Err(SealedViewError);
        }
        self.push(utf16(fragment));
        Ok(())
    }

    fn push(&mut self, fragment: Vec<u16>) {
        if fragment.is_empty() {
            return;
        }
        self.size += fragment.len();
        self.chunks.push(Arc::from(fragment));
        self.ends.push(self.size);
    }

    /// Reconcile a streamed prefix with authoritative complete text without joining the fragments.
    ///
    /// Returns `None` when every character matched: this view is now sealed and keeps its caches
    /// (the source returns the same view). Otherwise returns a new sealed view and leaves this
    /// one unchanged.
    pub fn settle(&mut self, text: &str) -> Option<PartialArguments> {
        let units = utf16(text);
        if self.object.is_some() || units.len() != self.size {
            return Some(Self::from_text(text));
        }
        let mut offset = 0;
        for chunk in &self.chunks {
            if !units[offset..].starts_with(chunk) {
                return Some(Self::from_text(text));
            }
            offset += chunk.len();
        }
        if units.is_empty() {
            self.chunks = Vec::new();
            self.ends = Vec::new();
        } else {
            self.ends = vec![units.len()];
            self.chunks = vec![Arc::from(units)];
        }
        self.sealed = true;
        None
    }

    /// Compare observed answers and advance their publication baseline. Unread views remain unscanned.
    /// Returns whether any observed answer changed since its first read or the preceding refresh.
    pub fn refresh(&mut self) -> bool {
        if self.reads.is_empty() {
            return false;
        }
        self.scan();
        let count = self.reads.len();
        let mut changed = false;
        let mut completions = false;
        for index in 0..count {
            if self
                .reads
                .get_index(index)
                .is_some_and(|(_, read)| read.completion)
            {
                completions = true;
                continue;
            }
            changed = self.refresh_read(index) || changed;
        }
        // Content reads can discover errors that change a field's completion answer.
        if completions {
            for index in 0..count {
                if self
                    .reads
                    .get_index(index)
                    .is_some_and(|(_, read)| read.completion)
                {
                    changed = self.refresh_read(index) || changed;
                }
            }
        }
        if self.sealed {
            self.reads.clear();
        }
        changed
    }

    fn refresh_read(&mut self, index: usize) -> bool {
        let Some(spec) = self.reads.get_index(index).map(|(spec, _)| spec.clone()) else {
            return false;
        };
        let now = comparison(&spec, &self.answer_now(&spec));
        match self.reads.get_index_mut(index) {
            Some((_, read)) if !now.same(&read.last) => {
                read.last = now;
                true
            }
            _ => false,
        }
    }

    /// Whether the outer object closed, indexing failed, or the view is sealed; unread values are not validated.
    pub fn closed(&mut self) -> bool {
        self.observe(ReadSpec::Closed).into_bool()
    }

    /// Top-level keys seen so far, in first-appearance order.
    pub fn keys(&mut self) -> Vec<String> {
        self.observe(ReadSpec::Keys).into_keys()
    }

    /// Whether the field has appeared (a string opened or another value began).
    pub fn has(&mut self, key: &str) -> bool {
        self.observe(ReadSpec::Has(key.to_owned())).into_bool()
    }

    /// Whether the field's closing delimiter has arrived and no content reader has reported an error for it.
    pub fn complete(&mut self, key: &str) -> bool {
        self.observe(ReadSpec::Complete(key.to_owned())).into_bool()
    }

    /// Decoded UTF-16 length of the string field so far; `None` when absent or not a string.
    pub fn string_length(&mut self, key: &str, options: LengthReadOptions) -> Option<usize> {
        let step = options.step.unwrap_or(1.0).floor().max(1.0);
        let offset = options.offset.unwrap_or(0.0);
        self.observe(ReadSpec::Length {
            key: key.to_owned(),
            step: step.to_bits(),
            offset: offset.to_bits(),
        })
        .into_length()
    }

    /// Whether the string is longer than `max_length` decoded UTF-16 units; `false` when absent or not a string.
    pub fn string_exceeds(&mut self, key: &str, max_length: usize) -> bool {
        self.observe(ReadSpec::Exceeds {
            key: key.to_owned(),
            limit: max_length,
        })
        .into_bool()
    }

    /// The string field's decoded text so far, exact UTF-16 units; `None` when absent or not a string.
    pub fn text_utf16(&mut self, key: &str) -> Option<Vec<u16>> {
        self.observe(ReadSpec::Text(key.to_owned())).into_text()
    }

    /// The string field's decoded text so far; a lone surrogate becomes U+FFFD.
    pub fn text(&mut self, key: &str) -> Option<String> {
        self.text_utf16(key)
            .map(|units| String::from_utf16_lossy(&units))
    }

    /// At most the first `max_length` (at least one) decoded UTF-16 units of a string, exact.
    pub fn text_prefix_utf16(&mut self, key: &str, max_length: usize) -> Option<Vec<u16>> {
        self.observe(ReadSpec::Prefix {
            key: key.to_owned(),
            limit: max_length.max(1),
        })
        .into_text()
    }

    /// Like [`Self::text_prefix_utf16`]; a lone surrogate becomes U+FFFD.
    pub fn text_prefix(&mut self, key: &str, max_length: usize) -> Option<String> {
        self.text_prefix_utf16(key, max_length)
            .map(|units| String::from_utf16_lossy(&units))
    }

    /// The parsed non-string value once it closed; `None` while open, absent, or a string.
    pub fn value(&mut self, key: &str) -> Option<JsonValue> {
        self.observe(ReadSpec::Value(key.to_owned())).into_value()
    }

    /// Answer a question and, on a streaming view, remember it for change detection.
    fn observe(&mut self, spec: ReadSpec) -> Raw {
        self.scan();
        let result = self.answer_now(&spec);
        if !self.sealed && !self.reads.contains_key(&spec) {
            let completion = matches!(spec, ReadSpec::Complete(_));
            let last = comparison(&spec, &result);
            self.reads.insert(spec, Read { completion, last });
        }
        result
    }

    fn answer_now(&mut self, spec: &ReadSpec) -> Raw {
        match spec {
            ReadSpec::Closed => Raw::Bool(self.closed_now()),
            ReadSpec::Keys => Raw::Keys(self.keys_now()),
            ReadSpec::Has(key) => Raw::Bool(self.has_now(key)),
            ReadSpec::Complete(key) => Raw::Bool(self.complete_now(key)),
            ReadSpec::Length { key, .. } => Raw::Length(self.length_now(key, usize::MAX)),
            ReadSpec::Exceeds { key, limit } => {
                Raw::Bool(self.length_now(key, limit.saturating_add(1)).unwrap_or(0) > *limit)
            }
            ReadSpec::Text(key) => Raw::Text(self.text_now(key)),
            ReadSpec::Prefix { key, limit } => Raw::Text(self.text_prefix_now(key, *limit)),
            ReadSpec::Value(key) => Raw::Value(self.value_now(key)),
        }
    }

    fn closed_now(&self) -> bool {
        self.sealed || self.mode == Mode::Closed || self.mode == Mode::Invalid
    }

    fn keys_now(&self) -> Vec<String> {
        match &self.object {
            Some(object) => object.keys().cloned().collect(),
            None => self
                .order
                .iter()
                .map(|key| String::from_utf16_lossy(key))
                .collect(),
        }
    }

    fn has_now(&self, key: &str) -> bool {
        match &self.object {
            Some(object) => object.contains_key(key),
            None => self.entries.contains_key(&utf16(key)),
        }
    }

    fn complete_now(&self, key: &str) -> bool {
        if let Some(object) = &self.object {
            return object.contains_key(key);
        }
        match self.entries.get(&utf16(key)) {
            Some(Entry::Str(entry)) => entry.end.is_some() && entry.invalid_at.is_none(),
            Some(Entry::Val(entry)) => entry.end.is_some() && !entry.invalid,
            None => false,
        }
    }

    /// Run `read` on a string entry taken out of the map, so it can use the rest of the view.
    fn with_string_entry<R>(
        &mut self,
        key: &str,
        read: impl FnOnce(&mut Self, &mut StrEntry) -> R,
    ) -> Option<R> {
        let key = utf16(key);
        let mut entry = self.entries.remove(&key)?;
        let result = match &mut entry {
            Entry::Str(string) => Some(read(self, string)),
            Entry::Val(_) => None,
        };
        self.entries.insert(key, entry);
        result
    }

    fn length_now(&mut self, key: &str, limit: usize) -> Option<usize> {
        if let Some(object) = &self.object {
            return match object.get(key) {
                Some(Value::String(text)) => Some(text.encode_utf16().count()),
                _ => None,
            };
        }
        self.with_string_entry(key, |view, entry| {
            if let Some(text) = &entry.text
                && Some(text.at) == entry.end
            {
                return text.length;
            }
            let mut read = entry
                .length
                .take()
                .unwrap_or_else(|| StringRead::starting_at(entry.start));
            if view.read_string(entry, &mut read, limit, false) {
                view.invalid_value = true;
            }
            let length = read.length;
            entry.length = Some(read);
            length
        })
    }

    fn text_now(&mut self, key: &str) -> Option<Vec<u16>> {
        if let Some(object) = &self.object {
            return match object.get(key) {
                Some(Value::String(text)) => Some(utf16(text)),
                _ => None,
            };
        }
        self.with_string_entry(key, |view, entry| {
            if entry.text.is_none()
                && let Some(end) = entry.end
                && entry.needs_decoding
                && entry.invalid_at.is_none()
            {
                view.parse_calls += 1;
                // The incremental reader retains the valid prefix when complete text contains an invalid escape.
                if let Some(text) = decode_json_string(&view.slice(entry.start, end)) {
                    entry.text = Some(StringRead {
                        at: end,
                        length: text.len(),
                        text,
                    });
                }
            }
            let mut read = entry
                .text
                .take()
                .unwrap_or_else(|| StringRead::starting_at(entry.start));
            if view.read_string(entry, &mut read, usize::MAX, true) {
                view.invalid_value = true;
            }
            let text = read.text.clone();
            entry.text = Some(read);
            text
        })
    }

    fn text_prefix_now(&mut self, key: &str, max_length: usize) -> Option<Vec<u16>> {
        if let Some(object) = &self.object {
            return match object.get(key) {
                Some(Value::String(text)) => Some(text.encode_utf16().take(max_length).collect()),
                _ => None,
            };
        }
        self.with_string_entry(key, |view, entry| {
            let mut read = entry
                .prefixes
                .remove(&max_length)
                .unwrap_or_else(|| StringRead::starting_at(entry.start));
            if view.read_string(entry, &mut read, max_length, true) {
                view.invalid_value = true;
            }
            let text = read.text.clone();
            entry.prefixes.insert(max_length, read);
            text
        })
    }

    fn value_now(&mut self, key: &str) -> Option<(Value, u64)> {
        if let Some(object) = &self.object {
            return match object.get(key) {
                Some(Value::String(_)) | None => None,
                Some(field) => Some((field.clone(), 0)),
            };
        }
        let key = utf16(key);
        let mut entry = self.entries.remove(&key)?;
        let result = match &mut entry {
            Entry::Val(value) => match value.end {
                Some(end) if !value.invalid => {
                    if value.parsed.is_none() {
                        self.parse_calls += 1;
                        match parse_json(&self.slice(value.start, end)) {
                            Some(parsed) => value.parsed = Some(parsed),
                            None => {
                                // Unread values retain only their ranges; a requested malformed value has no parsed result.
                                value.invalid = true;
                                self.invalid_value = true;
                            }
                        }
                    }
                    value.parsed.clone().map(|parsed| (parsed, value.serial))
                }
                _ => None,
            },
            Entry::Str(_) => None,
        };
        self.entries.insert(key, entry);
        result
    }

    /// Index of the fragment holding the unit at `at`.
    fn chunk_at(&self, at: usize) -> usize {
        self.ends.partition_point(|end| *end <= at)
    }

    /// Materialize only a requested range, never the cumulative source.
    fn slice(&self, start: usize, end: usize) -> Vec<u16> {
        let end = end.min(self.size);
        if start >= end {
            return Vec::new();
        }
        let first = self.chunk_at(start);
        let last = self.chunk_at(end - 1);
        let base = if first == 0 { 0 } else { self.ends[first - 1] };
        if first == last {
            return self.chunks[first][start - base..end - base].to_vec();
        }
        let mut parts = self.chunks[first][start - base..].to_vec();
        for chunk in &self.chunks[first + 1..last] {
            parts.extend_from_slice(chunk);
        }
        parts.extend_from_slice(&self.chunks[last][..end - self.ends[last - 1]]);
        parts
    }

    /// Advance one content reader. Returns whether it found an invalid escape or control character.
    fn read_string(
        &self,
        entry: &mut StrEntry,
        read: &mut StringRead,
        limit: usize,
        materialize: bool,
    ) -> bool {
        let end = entry
            .end
            .unwrap_or(self.consumed)
            .min(entry.invalid_at.unwrap_or(usize::MAX))
            .min(self.invalid_at.unwrap_or(usize::MAX));
        if !entry.needs_decoding {
            let length = end
                .saturating_sub(read.at)
                .min(limit.saturating_sub(read.length));
            if length == 0 {
                return false;
            }
            if materialize {
                read.text.extend(self.slice(read.at, read.at + length));
            }
            read.at += length;
            read.length += length;
            return false;
        }
        let mut chunk_index = self.chunk_at(read.at);
        while read.at < end && read.length < limit {
            let base = if chunk_index == 0 {
                0
            } else {
                self.ends[chunk_index - 1]
            };
            let Some(chunk) = self.chunks.get(chunk_index) else {
                return false;
            };
            let remaining = &chunk[read.at - base..chunk.len().min(end - base)];
            let boundary = remaining.iter().position(|unit| is_content_escape(*unit));
            let length = boundary.unwrap_or(remaining.len()).min(limit - read.length);
            if length > 0 {
                if materialize {
                    read.text.extend_from_slice(&remaining[..length]);
                }
                read.at += length;
                read.length += length;
                if read.at == base + chunk.len() {
                    chunk_index += 1;
                }
                continue;
            }
            let Some(first) = remaining.first().copied() else {
                return false;
            };
            let kind = if remaining.len() > 1 {
                Some(remaining[1])
            } else if read.at + 1 < end {
                self.chunks
                    .get(chunk_index + 1)
                    .and_then(|next| next.first().copied())
            } else {
                None
            };
            let mut decoded = None;
            let mut width = 2;
            if first == BACKSLASH && kind.is_none() && entry.end.is_none() {
                return false;
            }
            if first == BACKSLASH && kind == Some(b'u' as u16) {
                let hex = self.slice(read.at + 2, end.min(read.at + 6));
                let digits: Option<Vec<u16>> = hex.iter().map(|unit| hex_value(*unit)).collect();
                if let Some(digits) = digits {
                    if digits.len() < 4 && entry.end.is_none() {
                        return false;
                    }
                    if digits.len() == 4 {
                        decoded = Some(digits.iter().fold(0u16, |unit, digit| unit * 16 + digit));
                    }
                }
                width = 6;
            } else if first == BACKSLASH
                && let Some(kind) = kind
            {
                decoded = simple_escape(kind);
            }
            let Some(unit) = decoded else {
                entry.invalid_at = Some(read.at);
                return true;
            };
            if materialize {
                read.text.push(unit);
            }
            read.length += 1;
            read.at += width;
            while chunk_index < self.chunks.len() && read.at >= self.ends[chunk_index] {
                chunk_index += 1;
            }
        }
        false
    }

    /// Locate new field ranges without decoding or parsing their contents.
    fn scan(&mut self) {
        if self.object.is_some() || self.consumed == self.size {
            return;
        }
        let mut chunk_index = self.chunk_at(self.consumed);
        while chunk_index < self.chunks.len() && self.invalid_at.is_none() {
            let pending = Arc::clone(&self.chunks[chunk_index]);
            let base = if chunk_index == 0 {
                0
            } else {
                self.ends[chunk_index - 1]
            };
            let mut index = self.consumed - base;
            while index < pending.len() && self.mode != Mode::Invalid {
                if self.mode == Mode::String || (self.mode == Mode::Nested && self.nested_in_string)
                {
                    let end = self.string_boundary(&pending, index);
                    self.consumed += end - index;
                    index = end;
                    if index == pending.len() {
                        break;
                    }
                }
                self.step(pending[index], self.consumed);
                self.consumed += 1;
                index += 1;
            }
            chunk_index += 1;
        }
    }

    /// Only raw quotes and their preceding backslash runs can terminate a string.
    fn string_boundary(&mut self, fragment: &[u16], start: usize) -> usize {
        let mut at = start;
        loop {
            let quote = fragment[at..]
                .iter()
                .position(|unit| *unit == QUOTE)
                .map(|offset| offset + at);
            let end = quote.unwrap_or(fragment.len());
            if self.mode == Mode::String
                && let Some(Entry::Str(entry)) = self.entries.get_mut(&self.key)
                && !entry.needs_decoding
                && fragment[at..end]
                    .iter()
                    .any(|unit| is_content_escape(*unit))
            {
                entry.needs_decoding = true;
            }
            let mut slash_start = end;
            while slash_start > at && fragment[slash_start - 1] == BACKSLASH {
                slash_start -= 1;
            }
            let escaped = ((end - slash_start) % 2 == 1) != (slash_start == at && self.escape);
            self.escape = quote.is_none() && escaped;
            match quote {
                Some(position) if escaped => at = position + 1,
                _ => return end,
            }
        }
    }

    fn step(&mut self, c: u16, at: usize) {
        match self.mode {
            Mode::Root => {
                if is_whitespace(c) {
                    return;
                }
                if c == b'{' as u16 {
                    self.mode = Mode::KeyOrEnd;
                    return;
                }
                self.fail();
            }
            Mode::KeyOrEnd => {
                if is_whitespace(c) {
                    return;
                }
                if c == b'}' as u16 {
                    self.mode = Mode::Closed;
                    return;
                }
                if c == QUOTE {
                    self.begin_key(at);
                    return;
                }
                self.fail();
            }
            Mode::KeyOnly => {
                if is_whitespace(c) {
                    return;
                }
                if c == QUOTE {
                    self.begin_key(at);
                    return;
                }
                self.fail();
            }
            Mode::Key => self.step_key(c, at),
            Mode::Colon => {
                if is_whitespace(c) {
                    return;
                }
                if c == b':' as u16 {
                    self.mode = Mode::Value;
                    return;
                }
                self.fail();
            }
            Mode::Value => self.begin_value(c, at),
            Mode::String => {
                if let Some(Entry::Str(entry)) = self.entries.get_mut(&self.key) {
                    entry.end = Some(at);
                }
                self.mode = Mode::CommaOrEnd;
            }
            Mode::Scalar => self.step_scalar(c, at),
            Mode::Nested => self.step_nested(c, at),
            Mode::CommaOrEnd => {
                if is_whitespace(c) {
                    return;
                }
                if c == b',' as u16 {
                    self.mode = Mode::KeyOnly;
                    return;
                }
                if c == b'}' as u16 {
                    self.mode = Mode::Closed;
                    return;
                }
                self.fail();
            }
            Mode::Closed => {
                if is_whitespace(c) {
                    return;
                }
                self.fail();
            }
            Mode::Invalid => {}
        }
    }

    fn fail(&mut self) {
        self.invalid_at = Some(self.consumed);
        self.mode = Mode::Invalid;
    }

    fn begin_key(&mut self, at: usize) {
        self.mode = Mode::Key;
        self.key_start = at + 1;
        self.key_escaped = false;
        self.escape = false;
    }

    fn step_key(&mut self, c: u16, at: usize) {
        if c < 0x20 {
            self.fail();
            return;
        }
        if self.escape {
            self.escape = false;
            return;
        }
        if c == BACKSLASH {
            self.escape = true;
            self.key_escaped = true;
            return;
        }
        if c != QUOTE {
            return;
        }
        let raw = self.slice(self.key_start, at);
        if self.key_escaped {
            self.parse_calls += 1;
            match decode_json_string(&raw) {
                Some(key) => self.key = key,
                None => {
                    // Invalid key escapes prevent identifying subsequent fields.
                    self.fail();
                    return;
                }
            }
        } else {
            self.key = raw;
        }
        self.mode = Mode::Colon;
    }

    fn open(&mut self, entry: Entry) {
        if !self.entries.contains_key(&self.key) {
            self.order.push(self.key.clone());
        }
        self.entries.insert(self.key.clone(), entry);
    }

    fn begin_value(&mut self, c: u16, at: usize) {
        if is_whitespace(c) {
            return;
        }
        if c == QUOTE {
            self.open(Entry::Str(StrEntry {
                start: at + 1,
                end: None,
                needs_decoding: false,
                invalid_at: None,
                length: None,
                text: None,
                prefixes: HashMap::new(),
            }));
            self.escape = false;
            self.mode = Mode::String;
            return;
        }
        if b"},:]".iter().any(|closer| c == u16::from(*closer)) {
            self.fail();
            return;
        }
        let serial = self.next_serial;
        self.next_serial += 1;
        self.open(Entry::Val(ValEntry {
            start: at,
            end: None,
            parsed: None,
            invalid: false,
            serial,
        }));
        if c == b'{' as u16 || c == b'[' as u16 {
            self.mode = Mode::Nested;
            self.nested_ends = vec![if c == b'{' as u16 {
                b'}' as u16
            } else {
                b']' as u16
            }];
            self.nested_in_string = false;
            self.escape = false;
            return;
        }
        self.mode = Mode::Scalar;
    }

    fn step_scalar(&mut self, c: u16, at: usize) {
        if c != b',' as u16 && c != b'}' as u16 && !is_whitespace(c) {
            return;
        }
        self.close_value(at);
        self.mode = if c == b',' as u16 {
            Mode::KeyOnly
        } else if c == b'}' as u16 {
            Mode::Closed
        } else {
            Mode::CommaOrEnd
        };
    }

    fn step_nested(&mut self, c: u16, at: usize) {
        if self.nested_in_string {
            self.nested_in_string = false;
            return;
        }
        if c == QUOTE {
            self.nested_in_string = true;
            return;
        }
        if c == b'{' as u16 || c == b'[' as u16 {
            self.nested_ends.push(if c == b'{' as u16 {
                b'}' as u16
            } else {
                b']' as u16
            });
            return;
        }
        if c == b'}' as u16 || c == b']' as u16 {
            if self.nested_ends.pop() != Some(c) {
                self.fail();
                return;
            }
            if self.nested_ends.is_empty() {
                self.close_value(at + 1);
                self.mode = Mode::CommaOrEnd;
            }
        }
    }

    fn close_value(&mut self, end: usize) {
        if let Some(Entry::Val(entry)) = self.entries.get_mut(&self.key) {
            entry.end = Some(end);
        }
    }
}
