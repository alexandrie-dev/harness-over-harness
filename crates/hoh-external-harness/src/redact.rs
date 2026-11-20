//! Masking of secrets in what a harness writes: its stderr, its error texts, and everything it
//! puts into the session log (answers, reasoning, step arguments and outputs). An API key, a
//! token or a password printed by a CLI or read by one of its commands must not reach the
//! session log, the window or the kernel log.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

pub const REDACTED: &str = "[redacted]";

/// The value after a secret's name: quoted (kept quoted) or a bare value up to the next space or
/// the end, `,` and `;` included (`DB_PASSWORD=ab;cd` hides `cd` as well), with an optional
/// `Bearer`/`Basic` scheme. A quote inside a bare value belongs to it; one at its end is kept:
/// it closes the quoting around the whole assignment (`-H 'Authorization: Bearer …'`).
const VALUE: &str =
    r#"("(?:[^"\\]|\\.)*"|'[^']*'|(?:bearer[ \t]+|basic[ \t]+)?[^\s"']+(?:["']+[^\s"']+)*)"#;

/// The words that make a name a secret's: `OPENAI_KEY`, `x-api-key`, `DB_PASSWORD`,
/// `access_token`, `--password`, … (a separator or a word boundary before the word).
const SECRET_WORDS: &str = r"(?:api[_-]?key|key|token|secret|pass(?:word|wd|phrase)?|pwd|credentials?|auth|authorization|cookie|session[_-]?(?:key|id|token))";

/// The words that make a name a secret's wherever they stand in it, glued to a prefix as well:
/// `SECRET_KEY_BASE`, `PASSWORD_PLAIN`, `PRIVATE_KEY_PEM`, `PGPASSWORD`.
const STRONG_WORDS: &str = r"(?:secret|pass(?:word|wd|phrase)|private[_-]?key|credentials?)";

/// `name=value`, `name: value`, `"name": "value"` where the name ends in a secret word or has a
/// strong one anywhere.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)\b((?:[a-z0-9]+[_.-])*(?:[a-z0-9]*{STRONG_WORDS}(?:[_.-][a-z0-9]+)*|{SECRET_WORDS}))("?[ \t]*[:=][ \t]*){VALUE}"#
    ))
    .expect("static pattern")
});

/// The camelCase names of JSON and code: `apiKey`, `clientSecret`, `refreshToken`.
static CAMEL_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\b([a-z][a-zA-Z0-9]*(?:Key|Token|Secret|Password|Passwd|Pwd|Credentials?))("?[ \t]*[:=][ \t]*){VALUE}"#
    ))
    .expect("static pattern")
});

/// The password of a URL: `https://user:password@host`, `/` and `?` in it included, and up to
/// the last `@` before the host (`u:p@ss@h` hides `p@ss`). A part after an `@` that holds `/`,
/// `?` or `#` is the path, not the password (`u:pw@registry/@scope/pkg`).
static URL_PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b([a-z][a-z0-9+.-]*://[^\s/:@'"]+:)[^\s@'"]+(?:@[^\s@/?#'"]+)*@"#)
        .expect("static pattern")
});

/// A private key block, whole; one without its end runs to the end of the text.
static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)-----BEGIN[ A-Z0-9]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END[ A-Z0-9]*PRIVATE KEY(?: BLOCK)?-----|\z)",
    )
    .expect("static pattern")
});

/// The rest of a private key whose start was cut off (the kept tail of a long output).
static PRIVATE_KEY_REST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\A.*?-----END[ A-Z0-9]*PRIVATE KEY(?: BLOCK)?-----").expect("static pattern")
});

/// An assignment whose value opens with a quote: the value runs to the closing quote.
static QUOTED_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[:=][ \t]*["']"#).expect("static pattern"));

/// A closed quoted value at the start of a text (the quoted forms of [`VALUE`]).
static CLOSED_QUOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\A(?:"(?:[^"\\]|\\.)*"|'[^']*')"#).expect("static pattern"));

/// `Bearer <token>` anywhere.
static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}").expect("static pattern")
});

/// Well-known key shapes (`sk-…`, `sk-ant-…`, GitHub, AWS, Google, Slack, JWT).
static KNOWN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{30,}|xox[abprs]-[A-Za-z0-9-]{10,}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
    )
    .expect("static pattern")
});

/// A long run of key-like characters with both letters and digits (an unknown token format).
static OPAQUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9_+=-]{40,}").expect("static pattern"));

/// The masked value of an assignment: a quoted value stays quoted.
fn masked_assignment(caps: &Captures<'_>) -> String {
    let value = &caps[3];
    let masked = match value.chars().next() {
        Some('"') => format!("\"{REDACTED}\""),
        Some('\'') => format!("'{REDACTED}'"),
        _ => REDACTED.to_owned(),
    };
    format!("{}{}{masked}", &caps[1], &caps[2])
}

/// `text` with every secret-looking value replaced by [`REDACTED`].
pub fn redact(text: &str) -> String {
    let text = PRIVATE_KEY.replace_all(text, REDACTED);
    let text = PRIVATE_KEY_REST.replace(&text, REDACTED);
    let text = URL_PASSWORD.replace_all(&text, format!("${{1}}{REDACTED}@").as_str());
    let text = KNOWN.replace_all(&text, REDACTED);
    let text = BEARER.replace_all(&text, format!("$1 {REDACTED}").as_str());
    let text = ASSIGNMENT.replace_all(&text, masked_assignment);
    let text = CAMEL_ASSIGNMENT.replace_all(&text, masked_assignment);
    OPAQUE
        .replace_all(&text, |caps: &Captures<'_>| {
            let run = &caps[0];
            let letters = run.bytes().any(|b| b.is_ascii_alphabetic());
            let digits = run.bytes().any(|b| b.is_ascii_digit());
            if letters && digits {
                REDACTED.to_owned()
            } else {
                run.to_owned()
            }
        })
        .into_owned()
}

/// How far a growing `text` can be shown now, given that it is shown up to byte `shown`
/// (always a cut this function returned). Each shown piece is masked by [`redact`] on its own,
/// so a cut never falls inside anything a pattern may mask: the tail that may still grow into a
/// secret waits for more text or for the end. Held back are the last word (it may be cut short),
/// with the name, separator or `Bearer` it may be the value of; a quoted value with its name up
/// to the closing quote; a private key block up to its end.
pub fn ready_to_show(text: &str, shown: usize) -> usize {
    let mut cut = word_start(text, text.trim_end().len()).max(shown);
    loop {
        let mut next = join_back(text, shown, cut);
        let mut from = shown;
        while let Some(found) = QUOTED_VALUE.find(&text[from..next]) {
            let quote = from + found.end() - 1;
            let end = CLOSED_QUOTE
                .find(&text[quote..])
                .map_or(usize::MAX, |closed| quote + closed.end());
            if end > next {
                next = join_back(text, shown, word_start(text, from + found.start()));
                break;
            }
            from = end;
        }
        let mut from = shown;
        while let Some(begin) = text[from..next].find("-----BEGIN").map(|at| from + at) {
            let end = text[begin..]
                .find("-----END")
                .and_then(|at| {
                    let marker = begin + at + "-----END".len();
                    text[marker..].find("-----").map(|close| marker + close + 5)
                })
                .unwrap_or(usize::MAX);
            if end > next {
                next = word_start(text, begin).max(shown);
                break;
            }
            from = end;
        }
        if next == cut {
            return cut;
        }
        cut = next;
    }
}

/// Start of the word that ends at byte `end` (or right after it when `end` follows a space).
fn word_start(text: &str, end: usize) -> usize {
    text[..end]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(at, c)| at + c.len_utf8())
}

/// `cut` moved back over the words a value held from `cut` may belong to: a name with its
/// separator, a separator alone, a `Bearer`/`Basic` scheme.
fn join_back(text: &str, shown: usize, mut cut: usize) -> usize {
    loop {
        let before = text[..cut].trim_end().len();
        if before <= shown {
            return cut;
        }
        let start = word_start(text, before);
        let word = &text[start..before];
        let joins = word.ends_with([':', '='])
            || text[cut..].starts_with([':', '='])
            || is_scheme_end(word);
        if !joins {
            return cut;
        }
        cut = start;
    }
}

/// `word` ends with the word `Bearer` or `Basic`.
fn is_scheme_end(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    ["bearer", "basic"].iter().any(|scheme| {
        lower
            .strip_suffix(scheme)
            .is_some_and(|head| !head.ends_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

/// `value` with [`redact`] applied to every string in it (object keys stay): masking the JSON
/// text itself could break its escaping.
pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact(text)),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), redact_value(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keys_tokens_and_assignments() {
        let cases = [
            (
                "ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuv",
                "ANTHROPIC_API_KEY=[redacted]",
            ),
            (
                "Authorization: Bearer abcdefgh12345678",
                "Authorization: [redacted]",
            ),
            (
                "got Bearer abcdefgh12345678 from cache",
                "got Bearer [redacted] from cache",
            ),
            (
                r#"{"access_token": "abc.def-1"}"#,
                r#"{"access_token": "[redacted]"}"#,
            ),
            ("token=hunter2 next", "token=[redacted] next"),
            ("key ghp_abcdefghijklmnopqrstuvwxyz012345", "key [redacted]"),
            ("aws AKIAABCDEFGHIJKLMNOP", "aws [redacted]"),
            (
                "id 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 end",
                "id [redacted] end",
            ),
            // Review round 2: credentials in URLs and names that only end in a secret word.
            (
                "fatal: unable to access 'https://bob:S3cretPw@git.example.com/r.git'",
                "fatal: unable to access 'https://bob:[redacted]@git.example.com/r.git'",
            ),
            ("OPENAI_KEY=abc123", "OPENAI_KEY=[redacted]"),
            (
                "export DB_PASSWORD=\"two words\"",
                "export DB_PASSWORD=\"[redacted]\"",
            ),
            ("x-api-key: k1 next", "x-api-key: [redacted] next"),
            (
                "mysql --password=pw1 -h db",
                "mysql --password=[redacted] -h db",
            ),
            (
                r#"{"clientSecret": "s3", "apiKey": "k2"}"#,
                r#"{"clientSecret": "[redacted]", "apiKey": "[redacted]"}"#,
            ),
            // Review round 3: private key blocks, strong words inside a name, `/` and `?` in
            // a URL password.
            (
                "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow/AB+cd\nQ2Fy/ZZ==\n\
                 -----END RSA PRIVATE KEY-----\ndone",
                "key:\n[redacted]\ndone",
            ),
            (
                "$ cat id\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1r/ZXk",
                "$ cat id\n[redacted]",
            ),
            (
                "Zm9v/YmFy\n-----END PRIVATE KEY-----\nexit 0",
                "[redacted]\nexit 0",
            ),
            ("SECRET_KEY_BASE=abc123", "SECRET_KEY_BASE=[redacted]"),
            (
                "clone https://u:p/w@h done",
                "clone https://u:[redacted]@h done",
            ),
            ("https://u:p?w@h/x", "https://u:[redacted]@h/x"),
            // Review 212418: `,` and `;` end no value; `@` inside a URL password.
            ("DB_PASSWORD=ab;cd", "DB_PASSWORD=[redacted]"),
            ("token=a1,b2 next", "token=[redacted] next"),
            ("https://u:p@ss@h", "https://u:[redacted]@h"),
            (
                "fetch https://u:p@s@s@git.example.com/r.git now",
                "fetch https://u:[redacted]@git.example.com/r.git now",
            ),
            (
                "npm https://u:pw@registry.example.com/@scope/pkg",
                "npm https://u:[redacted]@registry.example.com/@scope/pkg",
            ),
            ("PGPASSWORD=a\"b'c run", "PGPASSWORD=[redacted] run"),
        ];
        for (input, expected) in cases {
            assert_eq!(redact(input), expected, "{input}");
        }
    }

    #[test]
    fn keeps_ordinary_diagnostics() {
        for text in [
            "error: file not found: /home/user/project/src/main.rs",
            "warning: retrying in 5 seconds (attempt 2 of 3)",
            "thread 'main' panicked at src/lib.rs:10:5",
            "the token budget is low",
            "monkey=banana tokenizer=fast",
            "see https://git.example.com/r.git and ssh://git@host/repo",
            "Password:\nLogin failed",
            "token_count=5 and http://localhost:3000/api",
            "-----BEGIN CERTIFICATE-----\nMIIBkTCB+w==\n-----END CERTIFICATE-----",
        ] {
            assert_eq!(redact(text), text);
        }
    }

    #[test]
    fn masks_every_string_of_a_json_value() {
        let value = serde_json::json!({
            "command": "export TOKEN=\"abc def\"",
            "items": ["ok", "OPENAI_KEY=abc123"],
            "count": 3,
        });
        assert_eq!(
            redact_value(&value),
            serde_json::json!({
                "command": "export TOKEN=\"[redacted]\"",
                "items": ["ok", "OPENAI_KEY=[redacted]"],
                "count": 3,
            })
        );
    }

    /// `text` streamed in `chunks`: what each chunk shows, then what the end shows.
    fn streamed(chunks: &[&str]) -> Vec<String> {
        let (mut text, mut shown, mut pieces) = (String::new(), 0, Vec::new());
        for chunk in chunks {
            text.push_str(chunk);
            let cut = ready_to_show(&text, shown);
            assert!(cut >= shown && text.is_char_boundary(cut), "{text:?} {cut}");
            pieces.push(redact(&text[shown..cut]));
            shown = cut;
        }
        pieces.push(redact(&text[shown..]));
        pieces
    }

    #[test]
    fn a_stream_never_shows_part_of_a_secret() {
        let text = "use OPENAI_KEY=sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUV and Authorization: \
            Bearer tok3nPARTSECRET then\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow/ABCD+ef/GH\n\
            -----END RSA PRIVATE KEY-----\nexport DB_PASSWORD=\"two words\" and \
            {\"apiKey\" : \"k2 k3\"} x-api-key: k1 https://u:p/w@h SECRET_KEY_BASE=abc123 \
            KEY = Bearer zzzzzzzz9 PGPASSWORD=ab;semi9 https://u:p@ss9@h то есть всё";
        let whole = redact(text);
        let secrets = [
            "ABCDEFGH",
            "MNOPQR",
            "tok3n",
            "PARTSECRET",
            "MIIE",
            "ef/GH",
            "two",
            "words",
            "k2",
            "k3",
            "k1",
            "p/w",
            "abc123",
            "zzzzzzzz9",
            "semi9",
            "ss9",
        ];
        for secret in secrets {
            assert!(!whole.contains(secret), "{secret} in {whole}");
        }
        // Character by character, and in two pieces cut at every place.
        let chars: Vec<String> = text.chars().map(String::from).collect();
        let by_char: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(streamed(&by_char).concat(), whole);
        for (at, _) in text.char_indices().skip(1) {
            assert_eq!(
                streamed(&[&text[..at], &text[at..]]).concat(),
                whole,
                "cut at {at}"
            );
        }
    }

    #[test]
    fn a_stream_shows_ordinary_text_one_word_behind() {
        let pieces = streamed(&["Hel", "lo wor", "ld, how ", "are you"]);
        assert_eq!(pieces, ["", "Hello ", "world, ", "how are ", "you"]);
    }
}
