//! A small shell tokenizer: just enough structure to find the commands in a bash string, their
//! words (quotes and escapes resolved), redirections and the command/process substitutions nested
//! inside them. Anything it cannot read is `Err(Unparsable)`; the caller then asks the user.

/// The string is not a shell command line this tokenizer can read (unterminated quote, ...).
#[derive(Debug, PartialEq, Eq)]
pub struct Unparsable(pub &'static str);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Word {
    /// The text with quotes and escapes removed. Expansions are left out of it.
    pub text: String,
    /// Part of the value comes from an expansion (`$x`, `$(..)`, `~`, `{a,b}`, a glob): the real
    /// words are not known before the shell runs.
    pub dynamic: bool,
    /// An unquoted glob character (`*`, `?`, `[..]`) is present.
    pub glob: bool,
    /// Bodies of the `$(..)`, backtick and `<(..)` substitutions inside this word.
    pub subst: Vec<String>,
    /// The first thing in the word is an expansion (`$X-n`, `$(cmd)`): nothing in `text` is known
    /// to stand at the start, so the word cannot be read as an option.
    pub lead_dynamic: bool,
}

impl Word {
    /// A `$…`, `$(…)`, backtick or process substitution that adds an unknown value here.
    fn expand(&mut self) {
        if self.text.is_empty() {
            self.lead_dynamic = true;
        }
        self.dynamic = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirKind {
    /// `>`, `>|`, `&>`: creates or empties the target.
    Truncate,
    /// `>>`, `&>>`.
    Append,
    /// `<`, `<<<`, `<>`: never destroys the target.
    Input,
    /// `<<` here-document.
    Heredoc,
    /// `>&`, `<&`: a file descriptor, unless the target is a file name.
    Dup,
}

#[derive(Debug, Default)]
pub struct Command {
    pub words: Vec<Word>,
    pub redirs: Vec<(RedirKind, Word)>,
    /// The previous command's output is this command's input (`a | b`).
    pub after_pipe: bool,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub commands: Vec<Command>,
    /// Substitutions found in here-document bodies that the shell would expand.
    pub extra_subst: Vec<String>,
    /// A `(` or `)` group is present, so a `cd` inside it may not reach the rest of the line.
    pub grouped: bool,
}

enum Unit {
    Word(Word),
    Sep,
    Pipe,
    Redir(RedirKind),
}

struct Pending {
    delimiter: String,
    expands: bool,
    strip_tabs: bool,
}

struct Lexer {
    c: Vec<char>,
    i: usize,
    units: Vec<Unit>,
    heredocs: Vec<Pending>,
    extra: Vec<String>,
    grouped: bool,
}

pub fn parse(src: &str) -> Result<Parsed, Unparsable> {
    let mut lexer = Lexer {
        c: src.chars().collect(),
        i: 0,
        units: Vec::new(),
        heredocs: Vec::new(),
        extra: Vec::new(),
        grouped: false,
    };
    lexer.run()?;
    if !lexer.heredocs.is_empty() {
        return Err(Unparsable("here-document without a body"));
    }
    let mut parsed = Parsed {
        commands: Vec::new(),
        extra_subst: lexer.extra,
        grouped: lexer.grouped,
    };
    let mut current = Command::default();
    let mut pending: Option<RedirKind> = None;
    let mut piped = false;
    for unit in lexer.units {
        match unit {
            Unit::Word(word) => match pending.take() {
                Some(kind) => current.redirs.push((kind, word)),
                None => current.words.push(word),
            },
            Unit::Redir(kind) => {
                if pending.is_some() {
                    return Err(Unparsable("redirection without a target"));
                }
                pending = Some(kind);
            }
            Unit::Sep | Unit::Pipe => {
                if pending.is_some() {
                    return Err(Unparsable("redirection without a target"));
                }
                let next_piped = matches!(unit, Unit::Pipe);
                if !current.words.is_empty() || !current.redirs.is_empty() {
                    current.after_pipe = piped;
                    parsed.commands.push(std::mem::take(&mut current));
                }
                piped = next_piped;
            }
        }
    }
    if pending.is_some() {
        return Err(Unparsable("redirection without a target"));
    }
    if !current.words.is_empty() || !current.redirs.is_empty() {
        current.after_pipe = piped;
        parsed.commands.push(current);
    }
    Ok(parsed)
}

const BAD: Unparsable = Unparsable("unterminated quote or substitution");

impl Lexer {
    fn peek(&self, offset: usize) -> Option<char> {
        self.c.get(self.i + offset).copied()
    }

    fn run(&mut self) -> Result<(), Unparsable> {
        let mut word_start = true;
        while let Some(ch) = self.peek(0) {
            match ch {
                ' ' | '\t' => {
                    self.i += 1;
                    word_start = true;
                }
                '\n' => {
                    self.i += 1;
                    self.units.push(Unit::Sep);
                    self.read_heredoc_bodies()?;
                    word_start = true;
                }
                '#' if word_start => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                ';' | '(' | ')' => {
                    self.grouped |= ch != ';';
                    self.i += 1;
                    self.units.push(Unit::Sep);
                    word_start = true;
                }
                '&' => {
                    if self.peek(1) == Some('>') {
                        self.i += 2;
                        let kind = if self.peek(0) == Some('>') {
                            self.i += 1;
                            RedirKind::Append
                        } else {
                            RedirKind::Truncate
                        };
                        self.units.push(Unit::Redir(kind));
                    } else {
                        self.i += if self.peek(1) == Some('&') { 2 } else { 1 };
                        self.units.push(Unit::Sep);
                    }
                    word_start = true;
                }
                '|' => {
                    // `||` separates like `;`; `|` and `|&` are pipes.
                    if self.peek(1) == Some('|') {
                        self.i += 2;
                        self.units.push(Unit::Sep);
                    } else {
                        self.i += if self.peek(1) == Some('&') { 2 } else { 1 };
                        self.units.push(Unit::Pipe);
                    }
                    word_start = true;
                }
                '<' | '>' if self.peek(1) == Some('(') => {
                    let word = self.word()?;
                    self.units.push(Unit::Word(word));
                    word_start = false;
                }
                '<' | '>' => {
                    self.redirect()?;
                    word_start = true;
                }
                _ => {
                    let word = self.word()?;
                    // `2>file`: a number glued to a redirection is its file descriptor.
                    let digits = !word.dynamic
                        && !word.text.is_empty()
                        && word.text.chars().all(|c| c.is_ascii_digit());
                    if digits && matches!(self.peek(0), Some('<' | '>')) {
                        word_start = false;
                        continue;
                    }
                    self.units.push(Unit::Word(word));
                    word_start = false;
                }
            }
        }
        Ok(())
    }

    fn redirect(&mut self) -> Result<(), Unparsable> {
        let first = self.peek(0).unwrap_or('>');
        self.i += 1;
        let second = self.peek(0);
        let kind = if first == '>' {
            match second {
                Some('>') => {
                    self.i += 1;
                    RedirKind::Append
                }
                Some('&') => {
                    self.i += 1;
                    RedirKind::Dup
                }
                Some('|') => {
                    self.i += 1;
                    RedirKind::Truncate
                }
                _ => RedirKind::Truncate,
            }
        } else {
            match second {
                Some('<') => {
                    self.i += 1;
                    if self.peek(0) == Some('<') {
                        self.i += 1;
                        RedirKind::Input // here-string: the target word is data
                    } else {
                        let strip_tabs = self.peek(0) == Some('-');
                        if strip_tabs {
                            self.i += 1;
                        }
                        return self.heredoc_delimiter(strip_tabs);
                    }
                }
                Some('&') => {
                    self.i += 1;
                    RedirKind::Dup
                }
                Some('>') => {
                    self.i += 1;
                    RedirKind::Input // `<>` opens without truncating
                }
                _ => RedirKind::Input,
            }
        };
        self.units.push(Unit::Redir(kind));
        Ok(())
    }

    fn heredoc_delimiter(&mut self, strip_tabs: bool) -> Result<(), Unparsable> {
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.i += 1;
        }
        let start = self.i;
        let word = self.word()?;
        let raw: String = self.c[start..self.i].iter().collect();
        if word.text.is_empty() {
            return Err(Unparsable("here-document without a delimiter"));
        }
        let expands = !raw.contains(['\'', '"', '\\']);
        self.heredocs.push(Pending {
            delimiter: word.text.clone(),
            expands,
            strip_tabs,
        });
        self.units.push(Unit::Redir(RedirKind::Heredoc));
        self.units.push(Unit::Word(Word {
            text: word.text,
            ..Word::default()
        }));
        Ok(())
    }

    fn read_heredoc_bodies(&mut self) -> Result<(), Unparsable> {
        for pending in std::mem::take(&mut self.heredocs) {
            let mut body = String::new();
            loop {
                if self.i >= self.c.len() {
                    return Err(Unparsable("here-document is not closed"));
                }
                let start = self.i;
                while self.peek(0).is_some_and(|c| c != '\n') {
                    self.i += 1;
                }
                let line: String = self.c[start..self.i].iter().collect();
                self.i += 1; // the newline (or the end)
                let check = if pending.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if check == pending.delimiter {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            if pending.expands {
                self.extra.extend(expansions_in(&body)?);
            }
        }
        Ok(())
    }

    /// One shell word, up to unquoted whitespace or an operator.
    fn word(&mut self) -> Result<Word, Unparsable> {
        let mut w = Word::default();
        let first = self.i;
        while let Some(ch) = self.peek(0) {
            match ch {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | ')' => break,
                '(' if self.i == first => {}
                '(' => break,
                '<' | '>' if self.peek(1) != Some('(') || self.i != first => break,
                '\\' => {
                    self.i += 1;
                    match self.peek(0) {
                        None => return Err(BAD),
                        Some('\n') => self.i += 1,
                        Some(c) => {
                            w.text.push(c);
                            self.i += 1;
                        }
                    }
                }
                '\'' => {
                    self.i += 1;
                    loop {
                        match self.peek(0) {
                            None => return Err(BAD),
                            Some('\'') => {
                                self.i += 1;
                                break;
                            }
                            Some(c) => {
                                w.text.push(c);
                                self.i += 1;
                            }
                        }
                    }
                }
                '"' => {
                    self.i += 1;
                    self.double_quoted(&mut w)?;
                }
                '$' => self.dollar(&mut w)?,
                '`' => {
                    let inner = self.backtick()?;
                    w.subst.push(inner);
                    w.expand();
                }
                '<' | '>' => {
                    // `<(cmd)` / `>(cmd)`
                    self.i += 1;
                    let inner = self.balanced()?;
                    w.subst.push(inner);
                    w.expand();
                }
                '~' if self.i == first => {
                    w.dynamic = true;
                    w.text.push('~');
                    self.i += 1;
                }
                '*' | '?' => {
                    w.glob = true;
                    w.dynamic = true;
                    w.text.push(ch);
                    self.i += 1;
                }
                '[' => {
                    w.text.push('[');
                    if self.i != first || self.peek(1).is_some_and(|c| !matches!(c, ' ' | '\t')) {
                        w.glob = true;
                        w.dynamic = true;
                    }
                    self.i += 1;
                }
                '{' => {
                    w.text.push('{');
                    self.i += 1;
                    if brace_expansion_ahead(&self.c, self.i) {
                        w.dynamic = true;
                        w.glob = true;
                    }
                }
                c => {
                    w.text.push(c);
                    self.i += 1;
                }
            }
        }
        Ok(w)
    }

    fn double_quoted(&mut self, w: &mut Word) -> Result<(), Unparsable> {
        loop {
            match self.peek(0) {
                None => return Err(BAD),
                Some('"') => {
                    self.i += 1;
                    return Ok(());
                }
                Some('\\') => {
                    self.i += 1;
                    match self.peek(0) {
                        None => return Err(BAD),
                        Some('\n') => self.i += 1,
                        Some(c @ ('"' | '\\' | '$' | '`')) => {
                            w.text.push(c);
                            self.i += 1;
                        }
                        Some(c) => {
                            w.text.push('\\');
                            w.text.push(c);
                            self.i += 1;
                        }
                    }
                }
                Some('$') => self.dollar(w)?,
                Some('`') => {
                    let inner = self.backtick()?;
                    w.subst.push(inner);
                    w.expand();
                }
                Some(c) => {
                    w.text.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// At a `$`: `$(..)`, `$((..))`, `${..}`, `$name`, `$'..'`; a lone `$` is a literal.
    fn dollar(&mut self, w: &mut Word) -> Result<(), Unparsable> {
        match self.peek(1) {
            Some('(') if self.peek(2) == Some('(') => {
                self.i += 2;
                self.balanced()?; // arithmetic: no command inside is run by the shell's rules
                w.expand();
            }
            Some('(') => {
                self.i += 1;
                let inner = self.balanced()?;
                w.subst.push(inner);
                w.expand();
            }
            Some('{') => {
                self.i += 2;
                let mut depth = 1;
                while depth > 0 {
                    match self.peek(0) {
                        None => return Err(BAD),
                        Some('{') => depth += 1,
                        Some('}') => depth -= 1,
                        Some(_) => {}
                    }
                    self.i += 1;
                }
                w.expand();
            }
            Some('\'') => {
                self.i += 2;
                loop {
                    match self.peek(0) {
                        None => return Err(BAD),
                        Some('\\') => self.i += 2,
                        Some('\'') => {
                            self.i += 1;
                            break;
                        }
                        Some(c) => {
                            w.text.push(c);
                            self.i += 1;
                        }
                    }
                }
                w.expand(); // escape sequences hide the real text
            }
            Some(c)
                if c.is_alphanumeric()
                    || matches!(c, '_' | '?' | '@' | '*' | '#' | '!' | '$' | '-') =>
            {
                self.i += 2;
                while self
                    .peek(0)
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    && !matches!(c, '?' | '@' | '*' | '#' | '!' | '$' | '-')
                {
                    self.i += 1;
                }
                w.expand();
            }
            _ => {
                w.text.push('$');
                self.i += 1;
            }
        }
        Ok(())
    }

    fn backtick(&mut self) -> Result<String, Unparsable> {
        self.i += 1;
        let mut inner = String::new();
        loop {
            match self.peek(0) {
                None => return Err(BAD),
                Some('`') => {
                    self.i += 1;
                    return Ok(inner);
                }
                Some('\\') if matches!(self.peek(1), Some('`' | '\\' | '$')) => {
                    inner.push(self.c[self.i + 1]);
                    self.i += 2;
                }
                Some(c) => {
                    inner.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// At the `(` of a `(...)`; returns the text inside the matching `)` and moves past it.
    fn balanced(&mut self) -> Result<String, Unparsable> {
        debug_assert_eq!(self.peek(0), Some('('));
        self.i += 1;
        let start = self.i;
        let mut depth = 1;
        while depth > 0 {
            match self.peek(0) {
                None => return Err(BAD),
                Some('(') => depth += 1,
                Some(')') => depth -= 1,
                Some('\\') => self.i += 1,
                Some(q @ ('\'' | '"')) => {
                    self.i += 1;
                    while self.peek(0) != Some(q) {
                        if self.peek(0).is_none() {
                            return Err(BAD);
                        }
                        if q == '"' && self.peek(0) == Some('\\') {
                            self.i += 1;
                        }
                        self.i += 1;
                    }
                }
                Some(_) => {}
            }
            self.i += 1;
        }
        Ok(self.c[start..self.i - 1].iter().collect())
    }
}

/// `{a,b}` or `{1..3}` starting after an opening brace at `from - 1`.
fn brace_expansion_ahead(c: &[char], from: usize) -> bool {
    let mut depth = 1;
    let mut comma = false;
    let mut i = from;
    while let Some(&ch) = c.get(i) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return comma;
                }
            }
            ',' => comma = true,
            '.' if c.get(i + 1) == Some(&'.') => comma = true,
            ' ' | '\t' | '\n' | ';' | '&' | '|' => return false,
            _ => {}
        }
        i += 1;
    }
    false
}

/// Command substitutions the shell would run inside expandable text (a here-document body).
fn expansions_in(text: &str) -> Result<Vec<String>, Unparsable> {
    let mut found = Vec::new();
    let mut lexer = Lexer {
        c: text.chars().collect(),
        i: 0,
        units: Vec::new(),
        heredocs: Vec::new(),
        extra: Vec::new(),
        grouped: false,
    };
    while let Some(ch) = lexer.peek(0) {
        match ch {
            '\\' => lexer.i += 2,
            '`' => found.push(lexer.backtick()?),
            '$' if lexer.peek(1) == Some('(') && lexer.peek(2) != Some('(') => {
                lexer.i += 1;
                found.push(lexer.balanced()?);
            }
            _ => lexer.i += 1,
        }
    }
    Ok(found)
}
