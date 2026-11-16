use crate::EvalEnv;
use hoh_util_values::js_string;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const EXPR_MAX_TOKENS: usize = 1024;

/// A parsed term of the closed configuration language (§5.5).
#[derive(Clone, Debug)]
pub enum Expr {
    #[doc(hidden)]
    Parsed(ParsedExpr),
}
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct ParsedExpr {
    source: String,
    term: Term,
}

/// Positions are one-based line/column and a zero-based UTF-16 offset.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{line}:{column}: {message}")]
pub struct ExprError {
    pub position: usize,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[derive(Clone, Debug)]
struct Term {
    at: usize,
    kind: Kind,
}
#[derive(Clone, Debug)]
enum Kind {
    Literal(Option<Value>),
    Parenthesized(Box<Term>),
    Env(String),
    Cwd,
    Exe,
    Platform,
    Home(Vec<String>),
    Service {
        name: String,
        fields: Vec<String>,
        optional: bool,
    },
    Array(Vec<Term>),
    Includes(Box<Term>, Box<Term>),
    Not(Box<Term>),
    Binary(Op, Box<Term>, Box<Term>),
    Conditional(Box<Term>, Box<Term>, Box<Term>),
    Number(Box<Term>),
    Json(Box<Term>),
    Join(Vec<Term>),
    Resolve(Box<Term>),
    Resource(String),
    Url(String, Box<Term>),
}
#[derive(Clone, Copy, Debug)]
enum Op {
    Or,
    Nullish,
    And,
    Equal,
    Unequal,
    Add,
}
#[derive(Clone, Debug)]
struct Token {
    text: String,
    string: Option<String>,
    at: usize,
}
struct Reader<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    next: usize,
    depth: usize,
}

fn error(source: &str, at: usize, message: impl Into<String>) -> ExprError {
    let prefix = &source[..at.min(source.len())];
    ExprError {
        position: prefix.encode_utf16().count(),
        line: prefix.bytes().filter(|c| *c == b'\n').count() + 1,
        column: prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count()
            + 1,
        message: message.into(),
    }
}

fn lex(source: &str) -> Result<Vec<Token>, ExprError> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let rest = &source[i..];
        let c = rest.chars().next().unwrap_or('\0');
        if js_whitespace(c) {
            i += c.len_utf8();
            continue;
        }
        if result.len() >= EXPR_MAX_TOKENS {
            return Err(error(
                source,
                i,
                "expression size limit exceeded (1024 tokens)",
            ));
        }
        let at = i;
        if c == '\'' || c == '"' {
            i += 1;
            let mut out = String::new();
            let mut closed = false;
            while i < source.len() {
                let ch = source[i..].chars().next().unwrap_or('\0');
                i += ch.len_utf8();
                if ch == c {
                    closed = true;
                    break;
                }
                if ch == '\n' || ch == '\r' {
                    return Err(error(source, i - ch.len_utf8(), "newline in string"));
                }
                if ch != '\\' {
                    out.push(ch);
                    continue;
                }
                let esc = source[i..]
                    .chars()
                    .next()
                    .ok_or_else(|| error(source, i, "unfinished escape"))?;
                i += esc.len_utf8();
                match esc {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'v' => out.push('\u{b}'),
                    '0' => out.push('\0'),
                    '\\' | '\'' | '"' | '/' => out.push(esc),
                    'x' | 'u' => {
                        let digits = if esc == 'x' { 2 } else { 4 };
                        let hex = source
                            .get(i..i + digits)
                            .ok_or_else(|| error(source, i, "unfinished unicode escape"))?;
                        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return Err(error(source, i, "invalid unicode escape"));
                        }
                        let mut code = u32::from_str_radix(hex, 16)
                            .map_err(|_| error(source, i, "invalid unicode escape"))?;
                        i += digits;
                        if (0xd800..=0xdbff).contains(&code) && source[i..].starts_with("\\u") {
                            let low = source
                                .get(i + 2..i + 6)
                                .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
                                .and_then(|s| u32::from_str_radix(s, 16).ok())
                                .filter(|low| (0xdc00..=0xdfff).contains(low))
                                .ok_or_else(|| error(source, i, "invalid surrogate pair"))?;
                            code = 0x10000 + ((code - 0xd800) << 10) + low - 0xdc00;
                            i += 6;
                        }
                        out.push(
                            char::from_u32(code)
                                .ok_or_else(|| error(source, at, "unpaired surrogate"))?,
                        );
                    }
                    _ => return Err(error(source, i - esc.len_utf8(), "unsupported escape")),
                }
            }
            if !closed {
                return Err(error(source, at, "unterminated string"));
            }
            result.push(Token {
                text: source[at..i].into(),
                string: Some(out),
                at,
            });
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            i += 1;
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_alphanumeric()
                    || matches!(source.as_bytes()[i], b'_' | b'$'))
            {
                i += 1;
            }
        } else if c.is_ascii_digit()
            || c == '-'
            || (c == '.' && source.as_bytes().get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            if c == '-' {
                i += 1;
            }
            while i < source.len() && source.as_bytes()[i].is_ascii_digit() {
                i += 1;
            }
            if source.as_bytes().get(i) == Some(&b'.') {
                i += 1;
                while i < source.len() && source.as_bytes()[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if matches!(source.as_bytes().get(i), Some(b'e' | b'E')) {
                i += 1;
                if matches!(source.as_bytes().get(i), Some(b'+' | b'-')) {
                    i += 1;
                }
                while i < source.len() && source.as_bytes()[i].is_ascii_digit() {
                    i += 1;
                }
            }
        } else if let Some(op) = ["===", "!==", "||", "??", "&&", "?."]
            .into_iter()
            .find(|op| rest.starts_with(op))
        {
            i += op.len();
        } else if "()[],.?!:+".contains(c) {
            i += 1;
        } else {
            return Err(error(source, at, "unexpected character"));
        }
        result.push(Token {
            text: source[at..i].into(),
            string: None,
            at,
        });
    }
    result.push(Token {
        text: String::new(),
        string: None,
        at: source.len(),
    });
    Ok(result)
}
impl Expr {
    pub fn parse(source: &str) -> Result<Self, ExprError> {
        let mut r = Reader {
            source,
            tokens: lex(source)?,
            next: 0,
            depth: 0,
        };
        let term = r.expr()?;
        if !r.peek("") {
            return Err(r.err("unexpected token after expression"));
        }
        Ok(Self::Parsed(ParsedExpr {
            source: source.into(),
            term,
        }))
    }
}
impl Reader<'_> {
    fn at(&self) -> usize {
        self.tokens[self.next].at
    }
    fn err(&self, message: &str) -> ExprError {
        error(self.source, self.at(), message)
    }
    fn peek(&self, token: &str) -> bool {
        self.tokens[self.next].string.is_none() && self.tokens[self.next].text == token
    }
    fn eat(&mut self, token: &str) -> bool {
        if self.peek(token) {
            self.next += 1;
            true
        } else {
            false
        }
    }
    fn need(&mut self, token: &str) -> Result<(), ExprError> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.err(&format!("expected {token}")))
        }
    }
    fn ident(&mut self) -> Result<String, ExprError> {
        let token = &self.tokens[self.next];
        if token.string.is_none()
            && token
                .text
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        {
            self.next += 1;
            Ok(token.text.clone())
        } else {
            Err(self.err("expected field name"))
        }
    }
    fn string(&mut self) -> Result<String, ExprError> {
        let value = self.tokens[self.next]
            .string
            .clone()
            .ok_or_else(|| self.err("expected string literal"))?;
        self.next += 1;
        Ok(value)
    }
    fn expr(&mut self) -> Result<Term, ExprError> {
        self.depth += 1;
        if self.depth > 128 {
            return Err(self.err("expression nesting limit exceeded"));
        }
        let mut term = self.binary(0)?;
        if self.eat("?") {
            let yes = self.expr()?;
            self.need(":")?;
            let no = self.expr()?;
            term = Term {
                at: term.at,
                kind: Kind::Conditional(Box::new(term), Box::new(yes), Box::new(no)),
            };
        }
        self.depth -= 1;
        Ok(term)
    }
    fn binary(&mut self, level: usize) -> Result<Term, ExprError> {
        if level == 4 {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        let mut nullish = false;
        let mut logical = matches!(left.kind, Kind::Binary(Op::And, _, _));
        loop {
            let operator_at = self.at();
            let op = match level {
                0 if self.eat("||") => Op::Or,
                0 if self.eat("??") => Op::Nullish,
                1 if self.eat("&&") => Op::And,
                2 if self.eat("===") => Op::Equal,
                2 if self.eat("!==") => Op::Unequal,
                3 if self.eat("+") => Op::Add,
                _ => break,
            };
            let right = self.binary(level + 1)?;
            if level == 0 {
                nullish |= matches!(op, Op::Nullish);
                logical |=
                    matches!(op, Op::Or) || matches!(right.kind, Kind::Binary(Op::And, _, _));
                if nullish && logical {
                    return Err(error(
                        self.source,
                        operator_at,
                        "?? cannot be mixed with || or && without parentheses",
                    ));
                }
            }
            left = Term {
                at: left.at,
                kind: Kind::Binary(op, Box::new(left), Box::new(right)),
            };
            if level == 2 {
                break;
            }
        }
        Ok(left)
    }
    fn unary(&mut self) -> Result<Term, ExprError> {
        let at = self.at();
        if self.eat("!") {
            self.depth += 1;
            if self.depth > 128 {
                return Err(self.err("expression nesting limit exceeded"));
            }
            let term = self.unary()?;
            self.depth -= 1;
            return Ok(Term {
                at,
                kind: Kind::Not(Box::new(term)),
            });
        }
        self.primary()
    }
    fn args(&mut self) -> Result<Vec<Term>, ExprError> {
        self.need("(")?;
        let mut args = vec![self.expr()?];
        while self.eat(",") {
            args.push(self.expr()?);
        }
        self.need(")")?;
        Ok(args)
    }
    fn builtin(&mut self, name: &str) -> Result<(), ExprError> {
        self.need("process")?;
        self.need(".")?;
        self.need("getBuiltinModule")?;
        self.need("(")?;
        let at = self.at();
        let actual = self.string()?;
        if actual != name {
            return Err(error(self.source, at, format!("expected {name}")));
        }
        self.need(")")?;
        self.need(".")
    }
    fn primary(&mut self) -> Result<Term, ExprError> {
        let at = self.at();
        let kind = if self.eat("(") {
            let term = self.expr()?;
            self.need(")")?;
            return Ok(Term {
                at,
                kind: Kind::Parenthesized(Box::new(term)),
            });
        } else if self.tokens[self.next].string.is_some() {
            Kind::Literal(Some(Value::String(self.string()?)))
        } else if self.eat("true") {
            Kind::Literal(Some(json!(true)))
        } else if self.eat("false") {
            Kind::Literal(Some(json!(false)))
        } else if self.eat("null") {
            Kind::Literal(Some(Value::Null))
        } else if self.eat("undefined") {
            Kind::Literal(None)
        } else if self.eat("[") {
            let mut items = Vec::new();
            if !self.eat("]") {
                items.push(self.expr()?);
                while self.eat(",") {
                    items.push(self.expr()?);
                }
                self.need("]")?;
            }
            let array = Term {
                at,
                kind: Kind::Array(items),
            };
            if self.eat(".") {
                self.need("includes")?;
                self.need("(")?;
                let item = self.expr()?;
                self.need(")")?;
                Kind::Includes(Box::new(array), Box::new(item))
            } else {
                return Ok(array);
            }
        } else if self.eat("dshHomePath") {
            self.need("(")?;
            let mut parts = vec![self.string()?];
            while self.eat(",") {
                parts.push(self.string()?);
            }
            self.need(")")?;
            Kind::Home(parts)
        } else if self.eat("Number") {
            self.need("(")?;
            let term = self.expr()?;
            self.need(")")?;
            Kind::Number(Box::new(term))
        } else if self.eat("JSON") {
            self.need(".")?;
            self.need("parse")?;
            self.need("(")?;
            let term = self.expr()?;
            self.need(")")?;
            Kind::Json(Box::new(term))
        } else if self.eat("ctx") {
            self.need(".")?;
            let name = self.ident()?;
            if name == "get" && self.eat("(") {
                let name = self.string()?;
                self.need(")")?;
                let mut fields = Vec::new();
                if self.eat("?.") {
                    fields.push(self.ident()?);
                    while self.eat(".") {
                        fields.push(self.ident()?);
                    }
                }
                Kind::Service {
                    name,
                    fields,
                    optional: true,
                }
            } else {
                self.need(".")?;
                let mut fields = vec![self.ident()?];
                while self.eat(".") {
                    fields.push(self.ident()?);
                }
                Kind::Service {
                    name,
                    fields,
                    optional: false,
                }
            }
        } else if self.peek("decodeURIComponent") {
            self.need("decodeURIComponent")?;
            self.need("(")?;
            self.need("new")?;
            self.need("URL")?;
            self.need("(")?;
            let rel = self.string()?;
            self.need(",")?;
            let base = self.expr()?;
            self.need(")")?;
            self.need(".")?;
            self.need("pathname")?;
            self.need(")")?;
            Kind::Url(rel, Box::new(base))
        } else if self.peek("process") {
            if self
                .tokens
                .get(self.next + 2)
                .is_some_and(|t| t.text == "getBuiltinModule")
            {
                self.builtin("node:path")?;
                if self.eat("join") {
                    Kind::Join(self.args()?)
                } else if self.eat("resolve") {
                    self.need("(")?;
                    let e = self.expr()?;
                    self.need(")")?;
                    Kind::Resolve(Box::new(e))
                } else {
                    self.need("dirname")?;
                    self.need("(")?;
                    self.builtin("node:module")?;
                    self.need("createRequire")?;
                    self.need("(")?;
                    self.need("baseUrl")?;
                    self.need(")")?;
                    self.need(".")?;
                    self.need("resolve")?;
                    self.need("(")?;
                    let package = self.string()?;
                    if !package.ends_with("/package.json") || package.len() == "/package.json".len()
                    {
                        return Err(error(
                            self.source,
                            at,
                            "resource lookup requires <package>/package.json",
                        ));
                    }
                    self.need(")")?;
                    self.need(")")?;
                    Kind::Resource(package[..package.len() - "/package.json".len()].into())
                }
            } else {
                self.need("process")?;
                self.need(".")?;
                if self.eat("env") {
                    self.need(".")?;
                    Kind::Env(self.ident()?)
                } else if self.eat("cwd") {
                    self.need("(")?;
                    self.need(")")?;
                    Kind::Cwd
                } else if self.eat("execPath") {
                    Kind::Exe
                } else {
                    self.need("platform")?;
                    Kind::Platform
                }
            }
        } else {
            let token = &self.tokens[self.next];
            let number = token
                .text
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| self.err("term is outside the configuration grammar"))?;
            self.next += 1;
            Kind::Literal(Some(number_value(number)))
        };
        Ok(Term { at, kind })
    }
}

fn number_value(number: f64) -> Value {
    crate::yaml::finite_number(number)
}
fn truthy(value: &Option<Value>) -> bool {
    value.as_ref().is_some_and(crate::is_truthy)
}
fn strict_equal(a: &Option<Value>, b: &Option<Value>) -> bool {
    match (a, b) {
        (None, None) | (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(a)), Some(Value::Bool(b))) => a == b,
        (Some(Value::String(a)), Some(Value::String(b))) => a == b,
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        _ => false,
    }
}

/// One evaluator shared by the future kernel adapter and the pure expression tests.
#[allow(dead_code)] // Part B calls this adapter after adding the kernel dependency.
pub(crate) fn evaluate(
    expr: &Expr,
    env: &EvalEnv,
    base_dir: &Path,
    service: &dyn Fn(&str) -> Option<Value>,
    resource: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Option<Value>, ExprError> {
    let Expr::Parsed(parsed) = expr;
    let scope = Scope {
        source: &parsed.source,
        env,
        _base_dir: base_dir,
        service,
        resource,
    };
    scope.eval(&parsed.term)
}
struct Scope<'a> {
    source: &'a str,
    env: &'a EvalEnv,
    _base_dir: &'a Path,
    service: &'a dyn Fn(&str) -> Option<Value>,
    resource: &'a dyn Fn(&str) -> Option<PathBuf>,
}
impl Scope<'_> {
    fn fail(&self, term: &Term, message: impl Into<String>) -> ExprError {
        error(self.source, term.at, message)
    }
    fn string(&self, term: &Term) -> Result<String, ExprError> {
        match self.eval(term)? {
            Some(Value::String(s)) => Ok(s),
            _ => Err(self.fail(term, "expected a string")),
        }
    }
    fn eval(&self, term: &Term) -> Result<Option<Value>, ExprError> {
        let value = match &term.kind {
            Kind::Literal(v) => return Ok(v.clone()),
            Kind::Parenthesized(inner) => return self.eval(inner),
            Kind::Env(name) => return Ok((self.env.var)(name).map(Value::String)),
            Kind::Cwd => Value::String(self.env.cwd.to_string_lossy().into()),
            Kind::Exe => Value::String(self.env.exe.to_string_lossy().into()),
            Kind::Platform => json!(self.env.platform),
            Kind::Home(parts) => json!(platform_join(
                &std::iter::once(self.env.home.to_string_lossy().into_owned())
                    .chain(parts.iter().cloned())
                    .collect::<Vec<_>>(),
                self.env.platform
            )),
            Kind::Service {
                name,
                fields,
                optional,
            } => {
                let mut current = (self.service)(name);
                if *optional
                    && (current.is_none() || (!fields.is_empty() && current == Some(Value::Null)))
                {
                    return Ok(None);
                }
                if current.is_none() {
                    return Err(self.fail(
                        term,
                        format!("service {name} is absent; declare inject: [{name}]"),
                    ));
                }
                for field in fields {
                    let object = current
                        .ok_or_else(|| self.fail(term, "cannot read a field of undefined"))?;
                    if object.is_null() {
                        return Err(self.fail(term, "cannot read a field of null"));
                    }
                    current = object.get(field).cloned();
                }
                return Ok(current);
            }
            Kind::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.eval(item).map(|v| v.unwrap_or(Value::Null)))
                    .collect::<Result<_, _>>()?,
            ),
            Kind::Includes(items, needle) => {
                // Evaluate original elements so an undefined slot is not confused with null.
                let Kind::Array(items) = &items.kind else {
                    return Err(self.fail(term, "includes requires an array literal"));
                };
                let elements = items
                    .iter()
                    .map(|item| self.eval(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let needle = self.eval(needle)?;
                json!(elements.iter().any(|item| strict_equal(item, &needle)))
            }
            Kind::Not(e) => json!(!truthy(&self.eval(e)?)),
            Kind::Binary(op, a, b) => {
                let a = self.eval(a)?;
                match op {
                    Op::Or => return if truthy(&a) { Ok(a) } else { self.eval(b) },
                    Op::And => return if truthy(&a) { self.eval(b) } else { Ok(a) },
                    Op::Nullish => {
                        return if a.is_none() || a == Some(Value::Null) {
                            self.eval(b)
                        } else {
                            Ok(a)
                        };
                    }
                    Op::Equal | Op::Unequal => {
                        json!(strict_equal(&a, &self.eval(b)?) == matches!(op, Op::Equal))
                    }
                    Op::Add => {
                        let b = self.eval(b)?;
                        let a =
                            a.ok_or_else(|| self.fail(term, "undefined in string concatenation"))?;
                        let b =
                            b.ok_or_else(|| self.fail(term, "undefined in string concatenation"))?;
                        if a.is_string() || b.is_string() {
                            json!(js_string(&a) + &js_string(&b))
                        } else if let (Some(a), Some(b)) = (a.as_f64(), b.as_f64()) {
                            let sum = a + b;
                            if !sum.is_finite() {
                                return Err(self.fail(term, "non-finite sum"));
                            }
                            number_value(sum)
                        } else {
                            return Err(
                                self.fail(term, "+ requires a string operand or two numbers")
                            );
                        }
                    }
                }
            }
            Kind::Conditional(test, yes, no) => {
                return self.eval(if truthy(&self.eval(test)?) { yes } else { no });
            }
            Kind::Number(e) => {
                let v = self.eval(e)?;
                let number = match v {
                    Some(Value::Number(n)) => n.as_f64(),
                    Some(Value::String(s)) => js_number(&s),
                    _ => None,
                };
                number_value(
                    number
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| self.fail(term, "Number argument is not a number"))?,
                )
            }
            Kind::Json(e) => hoh_util_values::js_json_parse(&self.string(e)?)
                .map_err(|e| self.fail(term, e.to_string()))?,
            Kind::Join(parts) => {
                let parts = parts
                    .iter()
                    .map(|e| self.string(e))
                    .collect::<Result<Vec<_>, _>>()?;
                json!(platform_join(&parts, self.env.platform))
            }
            Kind::Resolve(e) => {
                let path = self.string(e)?;
                json!(resolve_path(&path, self.env))
            }
            Kind::Resource(name) => {
                let dir = (self.resource)(name).ok_or_else(|| {
                    self.fail(
                        term,
                        format!("package resource directory {name} is not registered"),
                    )
                })?;
                json!(dir.to_string_lossy())
            }
            Kind::Url(rel, base) => {
                let base = self.string(base)?;
                let base = url::Url::parse(&base)
                    .map_err(|e| self.fail(term, format!("invalid URL: {e}")))?;
                let url = base
                    .join(rel)
                    .map_err(|e| self.fail(term, format!("invalid URL: {e}")))?;
                json!(decode_uri(url.path()).map_err(|message| self.fail(term, message))?)
            }
        };
        Ok(Some(value))
    }
}
fn js_number(text: &str) -> Option<f64> {
    let s = text.trim_matches(js_whitespace);
    if s.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = s.strip_prefix(prefix) {
            if digits.is_empty() {
                return None;
            }
            return digits.chars().try_fold(0.0, |n, c| {
                c.to_digit(radix)
                    .map(|d| n * f64::from(radix) + f64::from(d))
            });
        }
    }
    s.parse().ok()
}
fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
pub(crate) fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            _ => parts.push(part),
        }
    }
    let mut result = format!("{}{}", if absolute { "/" } else { "" }, parts.join("/"));
    if result.is_empty() {
        result.push('.');
    }
    if trailing && result != "/" {
        result.push('/');
    }
    result
}
fn join_path(parts: &[String]) -> String {
    normalize(
        &parts
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("/"),
    )
}
pub(crate) fn platform_join(parts: &[String], platform: &str) -> String {
    if platform == "win32" {
        windows_normalize(
            &parts
                .iter()
                .filter(|p| !p.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("\\"),
        )
    } else {
        join_path(parts)
    }
}
fn drive(path: &str) -> Option<&str> {
    (path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':')
        .then(|| &path[..2])
}
pub(crate) fn windows_normalize(path: &str) -> String {
    let path = path.replace('\\', "/");
    if path.starts_with("//") && !path.starts_with("///") {
        let mut parts = path[2..].split('/').filter(|p| !p.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            let rest = parts.collect::<Vec<_>>().join("/");
            let rest = normalize(&format!("/{rest}"));
            let trailing = path.ends_with('/');
            let mut result = format!("\\\\{server}\\{share}{}", rest.replace('/', "\\"));
            if trailing && !result.ends_with('\\') {
                result.push('\\');
            }
            return result;
        }
    }
    let prefix = drive(&path).unwrap_or("");
    let rest = &path[prefix.len()..];
    let normalized = normalize(rest).replace('/', "\\");
    format!("{prefix}{normalized}")
}
fn resolve_path(path: &str, env: &EvalEnv) -> String {
    let cwd = env.cwd.to_string_lossy();
    if env.platform != "win32" {
        let result = if path.starts_with('/') {
            normalize(path)
        } else {
            join_path(&[cwd.into_owned(), path.into()])
        };
        return if result == "/" {
            result
        } else {
            result.trim_end_matches('/').into()
        };
    }
    let path = path.replace('\\', "/");
    let cwd = cwd.replace('\\', "/");
    let prefix = drive(&path);
    let absolute = path[prefix.map_or(0, str::len)..].starts_with('/');
    let joined = if path.starts_with("//") || (prefix.is_some() && absolute) {
        path
    } else if absolute {
        let device = if let Some(drive) = drive(&cwd) {
            drive.to_owned()
        } else if let Some(unc) = cwd.strip_prefix("//") {
            format!("//{}", unc.split('/').take(2).collect::<Vec<_>>().join("/"))
        } else {
            String::new()
        };
        format!("{device}{path}")
    } else if let Some(prefix) = prefix {
        let base = if drive(&cwd).is_some_and(|d| d.eq_ignore_ascii_case(prefix)) {
            cwd.clone()
        } else {
            (env.var)(&format!("={prefix}")).unwrap_or_else(|| format!("{prefix}/"))
        };
        format!("{base}/{}", &path[2..])
    } else {
        format!("{cwd}/{path}")
    };
    let result = windows_normalize(&joined);
    let unc_root = result
        .strip_prefix("\\\\")
        .is_some_and(|rest| rest.trim_end_matches('\\').split('\\').count() == 2);
    if result == "\\" || (result.len() == 3 && drive(&result).is_some()) || unc_root {
        result
    } else {
        result.trim_end_matches('\\').into()
    }
}
fn decode_uri(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3).ok_or("incomplete URI escape")?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("URI malformed".into());
            }
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "invalid URI escape")?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "invalid UTF-8 in URI".into())
}

#[cfg(test)]
#[path = "../tests/support/expr_eval.rs"]
mod eval_tests;
