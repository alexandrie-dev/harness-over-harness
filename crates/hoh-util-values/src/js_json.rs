//! `JSON.stringify` and `JSON.parse` as JavaScript (V8) performs them over [`serde_json::Value`].
//!
//! Both directions work with an explicit stack, so nesting depth is not limited by the call stack.

use std::fmt;

use serde_json::{Map, Number, Value};

use crate::js_number_string;
use crate::partial_json::array_index;

/// Containers nested deeper than this make `js_json_parse` fail with `Maximum call stack size exceeded`.
pub const JS_JSON_MAX_DEPTH: usize = 4096;
const MAX_SAFE: u64 = 1 << 53;
const MAX_INDENT: usize = 10;
/// V8: sources at least this long get a context excerpt in "Unexpected token" messages.
const MIN_SOURCE_FOR_CONTEXT: usize = 21;
const CONTEXT: usize = 10;

/// `JSON.stringify(value)`.
pub fn js_json_stringify(value: &Value) -> String {
    stringify(value, "")
}

/// `JSON.stringify(value, null, indent)`; `indent` is clamped to 10 as JavaScript does; 0 gives the compact form.
pub fn js_json_stringify_pretty(value: &Value, indent: usize) -> String {
    stringify(value, &" ".repeat(indent.min(MAX_INDENT)))
}

fn number_text(number: &Number) -> String {
    if let Some(integer) = number.as_i64() {
        if integer.unsigned_abs() <= MAX_SAFE {
            integer.to_string()
        } else {
            js_number_string(integer as f64)
        }
    } else if let Some(integer) = number.as_u64() {
        if integer <= MAX_SAFE {
            integer.to_string()
        } else {
            js_number_string(integer as f64)
        }
    } else {
        js_number_string(number.as_f64().unwrap_or(f64::NAN))
    }
}

fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32))
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The members of an object in JavaScript property order: integer-like keys ascending, then the rest as stored.
fn ordered_entries(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    if entries.iter().any(|(key, _)| array_index(key).is_some()) {
        entries.sort_by_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
    }
    entries
}

enum Frame<'a> {
    Array {
        items: &'a [Value],
        index: usize,
    },
    Object {
        entries: Vec<(&'a String, &'a Value)>,
        index: usize,
    },
}

fn newline(out: &mut String, gap: &str, level: usize) {
    if !gap.is_empty() {
        out.push('\n');
        for _ in 0..level {
            out.push_str(gap);
        }
    }
}

fn stringify(root: &Value, gap: &str) -> String {
    let mut out = String::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut pending = Some(root);
    loop {
        if let Some(value) = pending.take() {
            match value {
                Value::Null => out.push_str("null"),
                Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
                Value::Number(number) => out.push_str(&number_text(number)),
                Value::String(text) => push_string(&mut out, text),
                Value::Array(items) if items.is_empty() => out.push_str("[]"),
                Value::Array(items) => {
                    out.push('[');
                    stack.push(Frame::Array { items, index: 0 });
                }
                Value::Object(map) if map.is_empty() => out.push_str("{}"),
                Value::Object(map) => {
                    out.push('{');
                    stack.push(Frame::Object {
                        entries: ordered_entries(map),
                        index: 0,
                    });
                }
            }
        }
        let level = stack.len();
        let Some(frame) = stack.last_mut() else { break };
        match frame {
            Frame::Array { items, index } => {
                if let Some(item) = items.get(*index) {
                    if *index > 0 {
                        out.push(',');
                    }
                    newline(&mut out, gap, level);
                    *index += 1;
                    pending = Some(item);
                } else {
                    stack.pop();
                    newline(&mut out, gap, level - 1);
                    out.push(']');
                }
            }
            Frame::Object { entries, index } => {
                if let Some((key, item)) = entries.get(*index) {
                    if *index > 0 {
                        out.push(',');
                    }
                    newline(&mut out, gap, level);
                    push_string(&mut out, key);
                    out.push_str(if gap.is_empty() { ":" } else { ": " });
                    *index += 1;
                    pending = Some(item);
                } else {
                    stack.pop();
                    newline(&mut out, gap, level - 1);
                    out.push('}');
                }
            }
        }
    }
    out
}

/// The `SyntaxError` of `JSON.parse`. `Display` prints the message of V8 (Node 20 wording).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonParseError {
    message: String,
}

impl JsonParseError {
    /// The V8 message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for JsonParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JsonParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Eos,
    String,
    Number,
    LBrace,
    RBrace,
    LBrack,
    RBrack,
    Colon,
    Comma,
    True,
    False,
    Null,
    Whitespace,
    Illegal,
}

fn token_of(unit: u16) -> Token {
    match u8::try_from(unit) {
        Ok(b'"') => Token::String,
        Ok(b'-' | b'0'..=b'9') => Token::Number,
        Ok(b'{') => Token::LBrace,
        Ok(b'}') => Token::RBrace,
        Ok(b'[') => Token::LBrack,
        Ok(b']') => Token::RBrack,
        Ok(b':') => Token::Colon,
        Ok(b',') => Token::Comma,
        Ok(b't') => Token::True,
        Ok(b'f') => Token::False,
        Ok(b'n') => Token::Null,
        Ok(b' ' | b'\t' | b'\n' | b'\r') => Token::Whitespace,
        _ => Token::Illegal,
    }
}

fn is_digit(unit: Option<u16>) -> bool {
    unit.is_some_and(|unit| (u16::from(b'0')..=u16::from(b'9')).contains(&unit))
}

fn hex_digit(unit: Option<u16>) -> Option<u16> {
    let byte = u8::try_from(unit?).ok()?;
    char::from(byte).to_digit(16).map(|digit| digit as u16)
}

fn order_map(map: Map<String, Value>) -> Map<String, Value> {
    if !map.keys().any(|key| array_index(key).is_some()) {
        return map;
    }
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    entries.sort_by_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
    entries.into_iter().collect()
}

enum Open {
    Array(Vec<Value>),
    Object {
        map: Map<String, Value>,
        key: String,
    },
}

/// Take a value apart without recursion, so dropping a deep value cannot overflow the stack.
fn dispose(mut work: Vec<Value>) {
    while let Some(value) = work.pop() {
        match value {
            Value::Array(items) => work.extend(items),
            Value::Object(map) => work.extend(map.into_iter().map(|(_, item)| item)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
}

struct Parser {
    source: Vec<u16>,
    pos: usize,
}

impl Parser {
    fn cur(&self) -> Option<u16> {
        self.source.get(self.pos).copied()
    }

    fn peek(&mut self) -> Token {
        while self
            .cur()
            .is_some_and(|unit| token_of(unit) == Token::Whitespace)
        {
            self.pos += 1;
        }
        self.cur().map_or(Token::Eos, token_of)
    }

    fn check(&mut self, token: Token) -> bool {
        if self.peek() == token {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: Token, message: &str) -> Result<(), JsonParseError> {
        if self.check(token) {
            Ok(())
        } else {
            Err(self.error_at(message))
        }
    }

    fn error_at(&self, message: &str) -> JsonParseError {
        JsonParseError {
            message: format!("{message} at position {}", self.pos),
        }
    }

    fn text(&self, from: usize, to: usize) -> String {
        String::from_utf16_lossy(&self.source[from..to])
    }

    /// The message V8 builds for the token at the cursor when no specific message applies.
    fn unexpected(&self) -> JsonParseError {
        let pos = self.pos;
        let length = self.source.len();
        let token = self.cur().map_or(Token::Eos, token_of);
        let message = match token {
            Token::Eos => "Unexpected end of JSON input".to_owned(),
            Token::Number => format!("Unexpected number in JSON at position {pos}"),
            Token::String => format!("Unexpected string in JSON at position {pos}"),
            _ => {
                let whole = self.text(0, length);
                if ["[object Object]", "NaN", "Infinity", "undefined"].contains(&whole.as_str()) {
                    format!("\"{whole}\" is not valid JSON")
                } else {
                    let unit = self.text(pos, pos + 1);
                    if length < MIN_SOURCE_FOR_CONTEXT {
                        format!("Unexpected token '{unit}', \"{whole}\" is not valid JSON")
                    } else if pos < CONTEXT {
                        format!(
                            "Unexpected token '{unit}', \"{}\"... is not valid JSON",
                            self.text(0, pos + CONTEXT)
                        )
                    } else if pos < length - CONTEXT {
                        let excerpt = self.text(pos - CONTEXT, pos + CONTEXT);
                        format!("Unexpected token '{unit}', ...\"{excerpt}\"... is not valid JSON")
                    } else {
                        format!(
                            "Unexpected token '{unit}', ...\"{}\" is not valid JSON",
                            self.text(pos - CONTEXT, length)
                        )
                    }
                }
            }
        };
        JsonParseError { message }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), JsonParseError> {
        let remaining = self.source.len() - self.pos;
        let matches = |parser: &Parser| {
            word.iter().enumerate().all(|(offset, byte)| {
                parser.source.get(parser.pos + offset) == Some(&u16::from(*byte))
            })
        };
        if remaining >= word.len() && matches(self) {
            self.pos += word.len();
            return Ok(());
        }
        self.pos += 1;
        for byte in &word[1..word.len().min(remaining)] {
            if self.cur() != Some(u16::from(*byte)) {
                return Err(self.unexpected());
            }
            self.pos += 1;
        }
        Err(self.unexpected())
    }

    fn number(&mut self) -> Result<Value, JsonParseError> {
        let start = self.pos;
        if self.cur() == Some(u16::from(b'-')) {
            self.pos += 1;
        }
        if self.cur() == Some(u16::from(b'0')) {
            self.pos += 1;
            if is_digit(self.cur()) {
                return Err(self.unexpected());
            }
        } else {
            let digits = self.pos;
            while is_digit(self.cur()) {
                self.pos += 1;
            }
            if digits == self.pos {
                return Err(self.error_at("No number after minus sign in JSON"));
            }
        }
        if self.cur() == Some(u16::from(b'.')) {
            self.pos += 1;
            if !is_digit(self.cur()) {
                return Err(self.error_at("Unterminated fractional number in JSON"));
            }
            while is_digit(self.cur()) {
                self.pos += 1;
            }
        }
        if matches!(self.cur(), Some(unit) if unit == u16::from(b'e') || unit == u16::from(b'E')) {
            self.pos += 1;
            if matches!(self.cur(), Some(unit) if unit == u16::from(b'+') || unit == u16::from(b'-'))
            {
                self.pos += 1;
            }
            if !is_digit(self.cur()) {
                return Err(self.error_at("Exponent part is missing a number in JSON"));
            }
            while is_digit(self.cur()) {
                self.pos += 1;
            }
        }
        let double: f64 = self.text(start, self.pos).parse().unwrap_or(f64::NAN);
        Ok(if !double.is_finite() {
            // `Value` cannot hold Infinity; JavaScript prints it back as `null`.
            Value::Null
        } else if double.fract() == 0.0
            && double.abs() <= MAX_SAFE as f64
            && !(double == 0.0 && double.is_sign_negative())
        {
            Value::Number(Number::from(double as i64))
        } else {
            Number::from_f64(double).map_or(Value::Null, Value::Number)
        })
    }

    /// Scan a string whose opening quote is at the cursor. An escaped lone surrogate becomes U+FFFD.
    fn string(&mut self) -> Result<String, JsonParseError> {
        self.pos += 1;
        let mut units: Vec<u16> = Vec::new();
        loop {
            let Some(unit) = self.cur() else {
                return Err(self.error_at("Unterminated string in JSON"));
            };
            if unit == u16::from(b'"') {
                self.pos += 1;
                return Ok(String::from_utf16_lossy(&units));
            }
            if unit == u16::from(b'\\') {
                self.pos += 1;
                // Past the end or beyond Latin-1, V8 reports the token itself.
                if self.cur().is_none_or(|unit| unit > 0xff) {
                    return Err(self.unexpected());
                }
                let escape = self.cur().and_then(|unit| u8::try_from(unit).ok());
                match escape {
                    Some(b'"') => units.push(u16::from(b'"')),
                    Some(b'\\') => units.push(u16::from(b'\\')),
                    Some(b'/') => units.push(u16::from(b'/')),
                    Some(b'b') => units.push(0x08),
                    Some(b'f') => units.push(0x0c),
                    Some(b'n') => units.push(u16::from(b'\n')),
                    Some(b'r') => units.push(u16::from(b'\r')),
                    Some(b't') => units.push(u16::from(b'\t')),
                    Some(b'u') => {
                        let mut value = 0u16;
                        for _ in 0..4 {
                            self.pos += 1;
                            let Some(digit) = hex_digit(self.cur()) else {
                                return Err(self.error_at("Bad Unicode escape in JSON"));
                            };
                            value = value * 16 + digit;
                        }
                        units.push(value);
                    }
                    _ => return Err(self.error_at("Bad escaped character in JSON")),
                }
                self.pos += 1;
                continue;
            }
            if unit < 0x20 {
                return Err(self.error_at("Bad control character in string literal in JSON"));
            }
            units.push(unit);
            self.pos += 1;
        }
    }

    fn run(&mut self, stack: &mut Vec<Open>) -> Result<Value, JsonParseError> {
        loop {
            let mut value = match self.peek() {
                Token::String => Value::String(self.string()?),
                Token::Number => self.number()?,
                Token::LBrace => {
                    if stack.len() >= JS_JSON_MAX_DEPTH {
                        return Err(JsonParseError {
                            message: "Maximum call stack size exceeded".to_owned(),
                        });
                    }
                    self.pos += 1;
                    if self.check(Token::RBrace) {
                        Value::Object(Map::new())
                    } else {
                        let key = self.key("Expected property name or '}' in JSON", true)?;
                        stack.push(Open::Object {
                            map: Map::new(),
                            key,
                        });
                        continue;
                    }
                }
                Token::LBrack => {
                    if stack.len() >= JS_JSON_MAX_DEPTH {
                        return Err(JsonParseError {
                            message: "Maximum call stack size exceeded".to_owned(),
                        });
                    }
                    self.pos += 1;
                    if self.check(Token::RBrack) {
                        Value::Array(Vec::new())
                    } else {
                        stack.push(Open::Array(Vec::new()));
                        continue;
                    }
                }
                Token::True => {
                    self.literal(b"true")?;
                    Value::Bool(true)
                }
                Token::False => {
                    self.literal(b"false")?;
                    Value::Bool(false)
                }
                Token::Null => {
                    self.literal(b"null")?;
                    Value::Null
                }
                _ => return Err(self.unexpected()),
            };
            loop {
                match stack.last_mut() {
                    None => {
                        if self.peek() != Token::Eos {
                            return Err(
                                self.error_at("Unexpected non-whitespace character after JSON")
                            );
                        }
                        return Ok(value);
                    }
                    Some(Open::Array(items)) => {
                        items.push(value);
                        if self.check(Token::Comma) {
                            break;
                        }
                        self.expect(
                            Token::RBrack,
                            "Expected ',' or ']' after array element in JSON",
                        )?;
                        value = match stack.pop() {
                            Some(Open::Array(items)) => Value::Array(items),
                            _ => Value::Null,
                        };
                    }
                    Some(Open::Object { map, key }) => {
                        map.insert(std::mem::take(key), value);
                        if self.check(Token::Comma) {
                            let next =
                                self.key("Expected double-quoted property name in JSON", false)?;
                            if let Some(Open::Object { key, .. }) = stack.last_mut() {
                                *key = next;
                            }
                            break;
                        }
                        self.expect(
                            Token::RBrace,
                            "Expected ',' or '}' after property value in JSON",
                        )?;
                        value = match stack.pop() {
                            Some(Open::Object { map, .. }) => Value::Object(order_map(map)),
                            _ => Value::Null,
                        };
                    }
                }
            }
        }
    }

    /// A property name and the colon after it.
    fn key(&mut self, missing: &str, first: bool) -> Result<String, JsonParseError> {
        self.expect_key_start(missing)?;
        let key = self.string()?;
        // V8 names the missing colon only after the first property; later ones report the token itself.
        if first {
            self.expect(Token::Colon, "Expected ':' after property name in JSON")?;
        } else if !self.check(Token::Colon) {
            return Err(self.unexpected());
        }
        Ok(key)
    }

    fn expect_key_start(&mut self, missing: &str) -> Result<(), JsonParseError> {
        if self.peek() == Token::String {
            Ok(())
        } else {
            Err(self.error_at(missing))
        }
    }
}

/// `JSON.parse(text)`.
pub fn js_json_parse(text: &str) -> Result<Value, JsonParseError> {
    let mut parser = Parser {
        source: text.encode_utf16().collect(),
        pos: 0,
    };
    let mut stack: Vec<Open> = Vec::new();
    let result = parser.run(&mut stack);
    if result.is_err() {
        let mut work = Vec::new();
        for open in stack {
            match open {
                Open::Array(items) => work.extend(items),
                Open::Object { map, .. } => work.extend(map.into_iter().map(|(_, item)| item)),
            }
        }
        dispose(work);
    }
    result
}
