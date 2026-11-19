//! Which parts of a bash command can delete or overwrite an existing file. The command is
//! tokenized by [`crate::shell`]; a target is looked up relative to the folder the command runs in.
//! Anything that cannot be decided (a target chosen by the shell at run time, text the tokenizer
//! cannot read, code handed to another shell) is reported as a risk too: the guard asks when it
//! cannot exclude a deletion or an overwrite.

use std::path::{Path, PathBuf};

use std::sync::Arc;

use crate::fs::{Fs, RealFs};
use crate::shell::{self, Command, RedirKind, Word};

/// Substitutions nested deeper than this are not followed: ask instead.
const MAX_DEPTH: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The names are operands of the command itself.
    Listed,
    /// `find ... -delete` / `-exec rm`.
    Find,
    /// `xargs rm`: the names arrive on standard input.
    Xargs,
    /// `rsync --delete`: what the destination has and the source lacks.
    Sync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitKind {
    Clean,
    ResetHard,
    Checkout,
    Restore,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Risk {
    /// Files or folders are removed (`rm`, `rmdir`, `unlink`, `shred`, `find -delete`, ...).
    Delete {
        names: Vec<String>,
        folders: bool,
        source: Source,
    },
    /// `truncate`: the content is cut.
    Truncate { names: Vec<String> },
    /// An existing file is replaced (`>`, `cp`, `mv`, `dd of=`, `tee`). `checked` is false when
    /// the target could not be inspected, so it may exist.
    Overwrite { names: Vec<String>, checked: bool },
    /// `git clean`, `git reset --hard`, `git checkout -- paths`, `git restore`, `git rm`.
    Git { kind: GitKind, names: Vec<String> },
    /// `tar x`: unpacking replaces files that already have the same names. `archive` is `None`
    /// when it comes from standard input or cannot be read from the command.
    Extract { archive: Option<String> },
    /// `patch`, `git apply`: files the patch changes are rewritten. `names` is empty when only the
    /// patch itself says which.
    Patch { names: Vec<String> },
    /// Code the guard cannot see: `eval`, `bash -c '...'`, `| sh`, `source file`, a command name
    /// produced by a substitution.
    Opaque(&'static str),
    /// The text is not a command line the tokenizer can read.
    Unparsable,
}

/// Every risk in `command`, run in `cwd` (`None` when the folder is not known).
pub fn assess(command: &str, cwd: Option<&Path>) -> Vec<Risk> {
    assess_with(&RealFs, command, cwd)
}

/// [`assess`] over the given file system.
pub fn assess_with(fs: &dyn Fs, command: &str, cwd: Option<&Path>) -> Vec<Risk> {
    let mut out = Vec::new();
    scan(fs, command, cwd.map(Path::to_path_buf), 0, &mut out);
    out
}

/// [`assess_with`] on a blocking thread: the file system checks (a slow mount, a huge folder for
/// a mask) never hold an async runtime thread. A panic there asks, like unreadable text.
pub async fn assess_blocking(command: String, cwd: Option<PathBuf>, fs: Arc<dyn Fs>) -> Vec<Risk> {
    tokio::task::spawn_blocking(move || assess_with(&*fs, &command, cwd.as_deref()))
        .await
        .unwrap_or_else(|_| vec![Risk::Unparsable])
}

fn scan(fs: &dyn Fs, src: &str, cwd: Option<PathBuf>, depth: usize, out: &mut Vec<Risk>) {
    if depth > MAX_DEPTH {
        out.push(Risk::Unparsable);
        return;
    }
    let parsed = match shell::parse(src) {
        Ok(parsed) => parsed,
        Err(_) => {
            out.push(Risk::Unparsable);
            return;
        }
    };
    let mut cwd = cwd;
    for body in &parsed.extra_subst {
        scan(fs, body, cwd.clone(), depth + 1, out);
    }
    for command in &parsed.commands {
        for word in command
            .words
            .iter()
            .chain(command.redirs.iter().map(|(_, word)| word))
        {
            for body in &word.subst {
                scan(fs, body, cwd.clone(), depth + 1, out);
            }
        }
        for (kind, target) in &command.redirs {
            redirect(fs, *kind, target, cwd.as_deref(), out);
        }
        let mut run = Run {
            fs,
            cwd: &mut cwd,
            grouped: parsed.grouped,
            out,
        };
        run.simple(&command.words, command, false);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cp,
    Mv,
    Install,
    Ln,
}

struct Run<'a> {
    fs: &'a dyn Fs,
    cwd: &'a mut Option<PathBuf>,
    grouped: bool,
    out: &'a mut Vec<Risk>,
}

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "!", "{", "}", "time",
    "function", "select",
];

fn is_assignment(text: &str) -> bool {
    match text.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// Words that only prefix another command; the options each one takes with a separate argument.
fn wrapper_args(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "sudo" | "doas" => &["-u", "-g", "-C", "-h", "-p", "-U", "-D", "-R"],
        "env" => &["-u", "-C", "-S"],
        "nice" => &["-n"],
        "ionice" => &["-c", "-n", "-p"],
        "timeout" => &["-s", "-k"],
        "stdbuf" => &["-i", "-o", "-e"],
        "command" | "builtin" | "exec" | "nohup" | "setsid" => &[],
        _ => return None,
    })
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn flag_chars(text: &str) -> &str {
    if text.starts_with('-') && !text.starts_with("--") {
        &text[1..]
    } else {
        ""
    }
}

fn has_short(args: &[Word], letter: char) -> bool {
    args.iter()
        .take_while(|w| w.text != "--")
        .any(|w| !w.lead_dynamic && flag_chars(&w.text).contains(letter))
}

fn has_long(args: &[Word], name: &str) -> bool {
    args.iter()
        .any(|w| !w.lead_dynamic && (w.text == name || w.text.starts_with(&format!("{name}="))))
}

/// Not an option: an operand, or a lone `-` (standard input). A word that starts with `-` but has
/// an expansion in it (`-$X`, `-o$OUT`, `--file=$OUT`) is still an option: what is written before
/// the expansion says what it is, the value is only known to the shell.
fn is_operand(word: &Word) -> bool {
    word.lead_dynamic || !word.text.starts_with('-') || (word.text == "-" && !word.dynamic)
}

/// An option with an expansion after its first characters: the shell fills in the rest.
fn is_dynamic_option(word: &Word) -> bool {
    word.dynamic && !is_operand(word)
}

/// The end-of-options marker (a literal `--`).
fn is_dashes(word: &Word) -> bool {
    !word.dynamic && word.text == "--"
}

/// The value of an option whose name ended `word`: inside the word when the shell fills it in
/// (`-o$OUT` is a dynamic word), otherwise the next word.
fn take_value(word: &Word, args: &[Word], i: &mut usize) -> Option<Word> {
    if word.dynamic {
        return Some(Word {
            dynamic: true,
            ..Word::default()
        });
    }
    let next = args.get(*i).cloned();
    *i += 1;
    next
}

/// The value written in the same word as its option (`--file=X`, `-fX`).
fn inline_value(word: &Word, text: &str) -> Word {
    Word {
        text: text.to_owned(),
        ..word.clone()
    }
}

/// A value the shell fills in at run time, to be checked like any other target (it never passes).
fn unknown_value() -> Word {
    Word {
        dynamic: true,
        ..Word::default()
    }
}

fn operands(args: &[Word]) -> Vec<&Word> {
    let mut found = Vec::new();
    let mut options_done = false;
    for word in args {
        if options_done || is_operand(word) {
            found.push(word);
        } else if is_dashes(word) {
            options_done = true;
        }
    }
    found
}

fn shown(word: &Word) -> String {
    if word.dynamic && !word.glob {
        "(имя станет известно при запуске)".to_owned()
    } else {
        word.text.clone()
    }
}

fn resolve(cwd: Option<&Path>, text: &str) -> Option<PathBuf> {
    if text.is_empty() {
        return None;
    }
    let path = Path::new(text);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        cwd.map(|base| base.join(path))
    }
}

/// Devices that every program may write to.
fn harmless_device(text: &str) -> bool {
    matches!(
        text,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/stdin" | "/dev/tty" | "/dev/zero"
    ) || text.starts_with("/dev/fd/")
}

/// Would writing `target` replace a file that is already there?
fn overwrites(fs: &dyn Fs, target: &Word, cwd: Option<&Path>, out: &mut Vec<Risk>) {
    if !target.dynamic && harmless_device(&target.text) {
        return;
    }
    if target.dynamic {
        out.push(Risk::Overwrite {
            names: vec![shown(target)],
            checked: false,
        });
        return;
    }
    match resolve(cwd, &target.text) {
        None => out.push(Risk::Overwrite {
            names: vec![target.text.clone()],
            checked: false,
        }),
        Some(path) if fs.exists(&path) && !fs.is_dir(&path) => out.push(Risk::Overwrite {
            names: vec![target.text.clone()],
            checked: true,
        }),
        Some(_) => {}
    }
}

fn redirect(fs: &dyn Fs, kind: RedirKind, target: &Word, cwd: Option<&Path>, out: &mut Vec<Risk>) {
    match kind {
        RedirKind::Truncate => overwrites(fs, target, cwd, out),
        RedirKind::Dup => {
            let fd = !target.dynamic
                && (target.text == "-" || target.text.chars().all(|c| c.is_ascii_digit()));
            if !fd {
                overwrites(fs, target, cwd, out);
            }
        }
        RedirKind::Append | RedirKind::Input | RedirKind::Heredoc => {}
    }
}

impl Run<'_> {
    /// One command; `listed` is true when its operands arrive from another command (`xargs`,
    /// `find -exec`), so the operands it names are not all of the files it touches.
    fn simple(&mut self, words: &[Word], whole: &Command, listed: bool) {
        let mut rest = words;
        // The folder a wrapper (`env -C`, `sudo -D`) moves the wrapped command to.
        let mut wrapped_dir: Option<Option<PathBuf>> = None;
        loop {
            let Some(first) = rest.first() else { return };
            if first.dynamic {
                break;
            }
            let text = first.text.as_str();
            if KEYWORDS.contains(&text) || is_assignment(text) {
                rest = &rest[1..];
                continue;
            }
            let name = base_name(text);
            let Some(takes) = wrapper_args(name) else {
                break;
            };
            if name == "command" && (has_short(&rest[1..], 'v') || has_short(&rest[1..], 'V')) {
                return;
            }
            let mut i = 1;
            while let Some(word) = rest.get(i) {
                if word.dynamic {
                    break;
                }
                if word.text == "--" {
                    i += 1;
                    break;
                }
                if word.text.starts_with('-') && word.text.len() > 1 {
                    // `env -S 'cmd args'` hands shell-split text to a command we cannot see.
                    if name == "env"
                        && (word.text == "--split-string"
                            || word.text.starts_with("--split-string=")
                            || flag_chars(&word.text).starts_with('S'))
                    {
                        self.out.push(Risk::Opaque("команда в строке для env -S"));
                        return;
                    }
                    // `env -C DIR`, `env --chdir=DIR`, `sudo -D DIR`: the wrapped command runs there.
                    let text = word.text.as_str();
                    let chdir: Option<(Option<Word>, bool)> = match (name, text) {
                        ("env", "-C" | "--chdir") | ("sudo", "-D" | "--chdir") => {
                            Some((rest.get(i + 1).cloned(), true))
                        }
                        ("env" | "sudo", t) if t.starts_with("--chdir=") => Some((
                            Some(Word {
                                text: t["--chdir=".len()..].to_owned(),
                                ..word.clone()
                            }),
                            false,
                        )),
                        ("env", t) if t.starts_with("-C") && !t.starts_with("--") => Some((
                            Some(Word {
                                text: t[2..].to_owned(),
                                ..word.clone()
                            }),
                            false,
                        )),
                        _ => None,
                    };
                    let mut consumed_next = false;
                    if let Some((dir, separate)) = chdir {
                        consumed_next = separate;
                        let base = match &wrapped_dir {
                            Some(dir) => dir.clone(),
                            None => self.cwd.clone(),
                        };
                        wrapped_dir = Some(match dir {
                            Some(dir) if !dir.dynamic => resolve(base.as_deref(), &dir.text),
                            _ => None,
                        });
                    }
                    i += if consumed_next || takes.contains(&word.text.as_str()) {
                        2
                    } else {
                        1
                    };
                } else if name == "env" && is_assignment(&word.text) {
                    i += 1;
                } else {
                    break;
                }
            }
            if name == "timeout" {
                i += 1;
            }
            rest = rest.get(i..).unwrap_or(&[]);
        }
        let Some(head) = rest.first() else { return };
        if head.dynamic {
            self.out
                .push(Risk::Opaque("имя команды задаётся подстановкой"));
            return;
        }
        let name = base_name(&head.text);
        let args = &rest[1..];
        let outer_cwd = wrapped_dir.map(|dir| std::mem::replace(self.cwd, dir));
        match name {
            "rm" | "rmdir" | "unlink" | "shred" => self.delete(name, args),
            "truncate" => self.truncate(args),
            "find" => self.find(args, whole),
            "xargs" => self.xargs(args, whole),
            "mv" => self.copy_move(Kind::Mv, args, listed),
            "cp" => self.copy_move(Kind::Cp, args, listed),
            "install" => self.copy_move(Kind::Install, args, listed),
            "ln" => self.copy_move(Kind::Ln, args, listed),
            "sed" | "perl" => {
                self.in_place(name, args, listed);
                if name == "perl" {
                    self.interpreter(name, args, whole);
                }
            }
            "rsync" => self.rsync(args),
            "curl" => self.curl(args),
            "wget" => self.wget(args),
            "unzip" => self.unzip(args),
            "patch" => self.patch(args),
            "awk" | "gawk" => self.awk(args, listed),
            "tar" | "gtar" | "bsdtar" => self.tar(args),
            "node" | "nodejs" | "ruby" => self.interpreter(name, args, whole),
            _ if name.starts_with("python") => self.interpreter("python", args, whole),
            "dd" => self.dd(args),
            "tee" => self.tee(args),
            "git" => self.git(args),
            "eval" => self.out.push(Risk::Opaque("eval")),
            "source" | "." => self.out.push(Risk::Opaque("source")),
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "fish" | "csh" | "tcsh" => {
                self.shell_interpreter(args, whole);
            }
            "cd" | "pushd" => self.change_dir(args),
            "popd" => *self.cwd = None,
            _ => {}
        }
        if let Some(outer) = outer_cwd {
            *self.cwd = outer;
        }
    }

    fn delete(&mut self, name: &str, args: &[Word]) {
        let recursive = name == "rmdir"
            || has_short(args, 'r')
            || has_short(args, 'R')
            || has_long(args, "--recursive");
        let ops = operands(args);
        let folders = recursive
            || ops.iter().any(|w| {
                !w.dynamic
                    && resolve(self.cwd.as_deref(), &w.text).is_some_and(|p| self.fs.is_dir(&p))
            });
        self.out.push(Risk::Delete {
            names: ops.iter().map(|w| shown(w)).collect(),
            folders,
            source: Source::Listed,
        });
    }

    fn truncate(&mut self, args: &[Word]) {
        let mut names = Vec::new();
        let mut skip = false;
        for word in args {
            if skip {
                skip = false;
            } else if !word.dynamic && matches!(word.text.as_str(), "-s" | "-r") {
                skip = true;
            } else if word.dynamic || !word.text.starts_with('-') {
                names.push(shown(word));
            }
        }
        self.out.push(Risk::Truncate { names });
    }

    /// Run `sub` (the command after `-exec`, `xargs`) and relabel what it deletes.
    fn nested(&mut self, sub: &[Word], whole: &Command, source: Source) {
        let mut found = Vec::new();
        let mut inner = Run {
            fs: self.fs,
            cwd: &mut self.cwd.clone(),
            grouped: self.grouped,
            out: &mut found,
        };
        inner.simple(sub, whole, true);
        for mut risk in found {
            if let Risk::Delete { source: s, .. } = &mut risk {
                *s = source;
            }
            self.out.push(risk);
        }
    }

    fn find(&mut self, args: &[Word], whole: &Command) {
        let mut i = 0;
        while let Some(word) = args.get(i) {
            i += 1;
            if word.dynamic {
                // `-$X`, `-del$X`: an action the shell fills in may be `-delete`.
                if is_dynamic_option(word) {
                    self.out.push(Risk::Delete {
                        names: Vec::new(),
                        folders: true,
                        source: Source::Find,
                    });
                }
                continue;
            }
            match word.text.as_str() {
                "-fprint" | "-fprint0" | "-fprintf" | "-fls" => {
                    // These write (and empty) the named file.
                    if let Some(file) = args.get(i) {
                        overwrites(self.fs, file, self.cwd.as_deref(), self.out);
                    }
                    i += 1;
                }
                "-delete" => self.out.push(Risk::Delete {
                    names: Vec::new(),
                    folders: true,
                    source: Source::Find,
                }),
                "-exec" | "-execdir" | "-ok" | "-okdir" => {
                    let start = i;
                    while args
                        .get(i)
                        .is_some_and(|w| w.dynamic || (w.text != ";" && w.text != "+"))
                    {
                        i += 1;
                    }
                    let sub: Vec<Word> = args[start..i]
                        .iter()
                        .filter(|w| w.dynamic || w.text != "{}")
                        .cloned()
                        .collect();
                    self.nested(&sub, whole, Source::Find);
                    i += 1;
                }
                _ => {}
            }
        }
    }

    fn xargs(&mut self, args: &[Word], whole: &Command) {
        let mut i = 0;
        while let Some(word) = args.get(i) {
            if word.dynamic || !word.text.starts_with('-') || word.text == "-" {
                break;
            }
            i += 1;
            if matches!(
                word.text.as_str(),
                "-I" | "-n" | "-P" | "-L" | "-s" | "-d" | "-E" | "-a" | "-l"
            ) || word.text == "--max-args"
                || word.text == "--delimiter"
                || word.text == "--arg-file"
            {
                i += 1;
            }
            if word.text == "--" {
                break;
            }
        }
        let sub = args.get(i..).unwrap_or(&[]);
        if !sub.is_empty() {
            self.nested(sub, whole, Source::Xargs);
        }
    }

    fn copy_move(&mut self, kind: Kind, args: &[Word], listed: bool) {
        // An option the shell fills in may be any of them (`-t DIR`, `-f`, ...).
        let hidden_options = args.iter().any(is_dynamic_option);
        let force = has_short(args, 'f')
            || has_long(args, "--force")
            || has_long(args, "--remove-destination");
        let no_clobber = has_long(args, "--no-clobber") || has_short(args, 'n');
        match kind {
            Kind::Cp | Kind::Mv if no_clobber && !force && !hidden_options => return,
            // Without `-f` `ln` refuses to replace what is there.
            Kind::Ln if !force && !hidden_options => return,
            Kind::Install if has_short(args, 'd') || has_long(args, "--directory") => return,
            _ => {}
        }
        let mut target_dir: Option<Word> = None;
        let mut ops: Vec<&Word> = Vec::new();
        let mut i = 0;
        let mut options_done = false;
        while let Some(word) = args.get(i) {
            i += 1;
            if options_done || is_operand(word) {
                ops.push(word);
                continue;
            }
            if is_dashes(word) {
                options_done = true;
                continue;
            }
            let text = word.text.as_str();
            if text == "--target-directory" {
                target_dir = take_value(word, args, &mut i);
            } else if let Some(value) = text.strip_prefix("--target-directory=") {
                target_dir = Some(inline_value(word, value));
            } else if let Some(flags) = text.strip_prefix('-').filter(|f| !f.starts_with('-')) {
                // `-t DIR`, `-tDIR`, `-vt DIR`, `-t$DIR`.
                let chars: Vec<char> = flags.chars().collect();
                for (k, ch) in chars.iter().enumerate() {
                    let last = k + 1 == chars.len();
                    if *ch == 't' {
                        target_dir = if last {
                            take_value(word, args, &mut i)
                        } else {
                            let rest: String = chars[k + 1..].iter().collect();
                            Some(inline_value(word, &rest))
                        };
                        break;
                    }
                    let takes = matches!(ch, 'S')
                        || (kind == Kind::Install && matches!(ch, 'm' | 'o' | 'g'));
                    if takes {
                        i += usize::from(last && !word.dynamic);
                        break;
                    }
                }
            } else if !word.dynamic
                && (matches!(text, "--suffix")
                    || (kind == Kind::Install && matches!(text, "--mode" | "--owner" | "--group")))
            {
                i += 1;
            }
        }
        if listed || (hidden_options && target_dir.is_none()) {
            // The files come from another command, or an option we cannot read may name the
            // target folder.
            self.out.push(Risk::Overwrite {
                names: Vec::new(),
                checked: false,
            });
            return;
        }
        let cwd = self.cwd.as_deref();
        let unchecked = |names: Vec<String>, out: &mut Vec<Risk>| {
            out.push(Risk::Overwrite {
                names,
                checked: false,
            });
        };
        let (dest_text, dest_dynamic, sources): (String, bool, Vec<&Word>) = match &target_dir {
            Some(word) => (word.text.clone(), word.dynamic, ops),
            None => {
                let Some((last, rest)) = ops.split_last() else {
                    return;
                };
                if rest.is_empty() {
                    // `ln -sf target`: the link is made in the current folder.
                    if kind != Kind::Ln {
                        return;
                    }
                    (".".to_owned(), false, vec![*last])
                } else {
                    (last.text.clone(), last.dynamic, rest.to_vec())
                }
            }
        };
        if dest_dynamic {
            unchecked(vec![dest_text], self.out);
            return;
        }
        let Some(dest) = resolve(cwd, &dest_text) else {
            unchecked(vec![dest_text], self.out);
            return;
        };
        if target_dir.is_none() && !self.fs.is_dir(&dest) {
            if self.fs.exists(&dest) {
                self.out.push(Risk::Overwrite {
                    names: vec![dest_text],
                    checked: true,
                });
            }
            return;
        }
        let mut hit = Vec::new();
        for source in sources {
            let names = if source.glob && !source.text.contains('[') {
                expand_glob(self.fs, cwd, &source.text)
            } else if source.dynamic {
                None
            } else {
                Path::new(&source.text)
                    .file_name()
                    .map(|n| vec![n.to_string_lossy().into_owned()])
            };
            let Some(names) = names else {
                unchecked(vec![format!("{dest_text}/{}", shown(source))], self.out);
                return;
            };
            for name in names {
                if self.fs.exists(&dest.join(&name)) {
                    hit.push(if dest_text == "." {
                        name
                    } else {
                        format!("{}/{name}", dest_text.trim_end_matches('/'))
                    });
                }
            }
        }
        if !hit.is_empty() {
            self.out.push(Risk::Overwrite {
                names: hit,
                checked: true,
            });
        }
    }

    /// `sed -i` / `perl -i`: the named files are rewritten in place.
    fn in_place(&mut self, name: &str, args: &[Word], listed: bool) {
        let mut in_place = false;
        let mut script_given = false;
        let mut ops: Vec<&Word> = Vec::new();
        let mut i = 0;
        let mut options_done = false;
        while let Some(word) = args.get(i) {
            i += 1;
            if options_done || is_operand(word) {
                ops.push(word);
                continue;
            }
            if is_dashes(word) {
                options_done = true;
                continue;
            }
            // An option the shell fills in may be `-i`.
            in_place |= word.dynamic;
            if let Some(long) = word.text.strip_prefix("--") {
                let (key, inline) = match long.split_once('=') {
                    Some((key, _)) => (key, true),
                    None => (long, false),
                };
                match key {
                    "in-place" => in_place = true,
                    "expression" | "file" => {
                        script_given = true;
                        i += usize::from(!inline && !word.dynamic);
                    }
                    _ => {}
                }
                continue;
            }
            let chars: Vec<char> = word.text[1..].chars().collect();
            for (k, ch) in chars.iter().enumerate() {
                let takes_arg = match (name, ch) {
                    ("sed", 'e' | 'f') => {
                        script_given = true;
                        true
                    }
                    ("perl", 'e' | 'E') => {
                        script_given = true;
                        true
                    }
                    ("perl", 'I') => true,
                    _ => false,
                };
                if *ch == 'i' {
                    in_place = true;
                    break; // the rest of the word is the backup suffix
                }
                if takes_arg {
                    i += usize::from(k + 1 == chars.len() && !word.dynamic);
                    break;
                }
            }
        }
        if !in_place {
            return;
        }
        let files: Vec<&Word> = if script_given {
            ops
        } else {
            ops.into_iter().skip(1).collect()
        };
        self.rewrite(&files, listed);
    }

    /// Files that a command rewrites in place: the ones that exist (a mask is expanded), those that
    /// cannot be checked, and — when the files come from another command — an unnamed set.
    fn rewrite(&mut self, files: &[&Word], listed: bool) {
        if files.is_empty() {
            if listed {
                self.out.push(Risk::Overwrite {
                    names: Vec::new(),
                    checked: false,
                });
            }
            return;
        }
        let cwd = self.cwd.as_deref();
        let mut existing = Vec::new();
        let mut unchecked = Vec::new();
        for word in files.iter().copied() {
            if word.glob && !word.text.contains('[') {
                match expand_glob(self.fs, cwd, &word.text) {
                    Some(names) => {
                        let folder = word.text.rsplit_once('/').map(|(folder, _)| folder);
                        for name in names {
                            let shown = folder.map_or(name.clone(), |f| format!("{f}/{name}"));
                            if resolve(cwd, &shown).is_some_and(|p| !self.fs.is_dir(&p)) {
                                existing.push(shown);
                            }
                        }
                    }
                    None => unchecked.push(shown(word)),
                }
                continue;
            }
            let mut found = Vec::new();
            overwrites(self.fs, word, cwd, &mut found);
            for risk in found {
                if let Risk::Overwrite { names, checked } = risk {
                    if checked {
                        existing.extend(names);
                    } else {
                        unchecked.extend(names);
                    }
                }
            }
        }
        if !existing.is_empty() {
            self.out.push(Risk::Overwrite {
                names: existing,
                checked: true,
            });
        }
        if !unchecked.is_empty() {
            self.out.push(Risk::Overwrite {
                names: unchecked,
                checked: false,
            });
        }
    }

    fn rsync(&mut self, args: &[Word]) {
        let mut deleting = false;
        let mut hidden = false;
        for word in args.iter().take_while(|w| !is_dashes(w)) {
            let text = word.text.as_str();
            if text.starts_with("--delete")
                || text == "--del"
                || text == "--remove-source-files"
                || text == "--remove-sent-files"
            {
                deleting = true;
            } else if is_dynamic_option(word) && matches!(text, "-" | "--") {
                hidden = true;
            }
        }
        if deleting {
            self.out.push(Risk::Delete {
                names: Vec::new(),
                folders: true,
                source: Source::Sync,
            });
        } else if hidden {
            self.out
                .push(Risk::Opaque("параметры rsync задаются при запуске"));
        }
    }

    /// `tar`: only listing and creating a new archive are left alone.
    fn tar(&mut self, args: &[Word]) {
        let mut modes = String::new();
        let mut archive: Option<Word> = None;
        // An option the shell fills in may be any mode or `-f`.
        let mut hidden = false;
        let mut i = 0;
        // The old style: `tar xzf archive.tgz`, letters without a dash in the first word (the
        // first word is always the options then, also when the shell fills it in).
        if let Some(first) = args
            .first()
            .filter(|w| w.lead_dynamic || !w.text.starts_with('-'))
        {
            i = 1;
            hidden |= first.dynamic;
            for ch in first.text.chars() {
                match ch {
                    'f' => {
                        archive = args.get(i).cloned();
                        i += 1;
                    }
                    'b' | 'L' | 'N' | 'V' | 'H' => i += 1,
                    'c' | 't' | 'x' | 'r' | 'u' | 'A' | 'd' => modes.push(ch),
                    _ => {}
                }
            }
        }
        let mut options_done = false;
        while let Some(word) = args.get(i) {
            i += 1;
            if options_done || is_operand(word) {
                continue;
            }
            if is_dashes(word) {
                options_done = true;
                continue;
            }
            if let Some(long) = word.text.strip_prefix("--") {
                let (key, value) = match long.split_once('=') {
                    Some((key, value)) => (key, Some(value)),
                    None => (long, None),
                };
                match key {
                    "create" => modes.push('c'),
                    "list" => modes.push('t'),
                    "extract" | "get" => modes.push('x'),
                    "append" => modes.push('r'),
                    "update" => modes.push('u'),
                    "concatenate" | "catenate" => modes.push('A'),
                    "delete" => modes.push('D'),
                    "diff" | "compare" => modes.push('d'),
                    // The mode is decided after every option, whatever their order.
                    "file" => {
                        archive = match value {
                            Some(value) => Some(inline_value(word, value)),
                            None => take_value(word, args, &mut i),
                        };
                    }
                    _ => {}
                }
                // `--$X`: an option we cannot read.
                hidden |= word.dynamic && key.is_empty();
                continue;
            }
            hidden |= word.dynamic && word.text.len() == 1;
            let chars: Vec<char> = word.text[1..].chars().collect();
            for (k, ch) in chars.iter().enumerate() {
                let last = k + 1 == chars.len();
                match ch {
                    'c' | 't' | 'x' | 'r' | 'u' | 'A' | 'd' => modes.push(*ch),
                    'f' => {
                        archive = if last {
                            take_value(word, args, &mut i)
                        } else {
                            // `-cfa.tar`: the rest of the word is the name.
                            let rest: String = chars[k + 1..].iter().collect();
                            Some(inline_value(word, &rest))
                        };
                        break;
                    }
                    'C' | 'T' | 'X' | 'b' | 'L' | 'N' | 'V' | 'H' | 'I' | 'K' => {
                        i += usize::from(last && !word.dynamic);
                        break;
                    }
                    _ => {}
                }
            }
        }
        if archive.is_none() && hidden {
            // The mode or the archive name is among the words the shell fills in.
            archive = Some(unknown_value());
        }
        // A first word that starts with an expansion (`$OPTS`, `$X-tf`) may hold any mode letter.
        let mode_unknown =
            (hidden && modes.is_empty()) || args.first().is_some_and(|w| w.lead_dynamic);
        self.tar_modes(&modes, archive.as_ref(), mode_unknown);
    }

    fn tar_modes(&mut self, modes: &str, archive: Option<&Word>, mode_unknown: bool) {
        let name = archive.filter(|w| w.text != "-" || w.dynamic).map(shown);
        let stdio = |w: &Word| w.text == "-" && !w.dynamic;
        if modes.contains('x') {
            self.out.push(Risk::Extract { archive: name });
        } else if modes.contains(['r', 'u', 'A', 'D']) {
            // The archive itself is rewritten.
            match archive {
                Some(word) if !stdio(word) => {
                    overwrites(self.fs, word, self.cwd.as_deref(), self.out);
                }
                _ => {}
            }
        } else if modes.contains('c')
            && let Some(word) = archive
            && !stdio(word)
        {
            overwrites(self.fs, word, self.cwd.as_deref(), self.out);
        }
        if mode_unknown && !modes.contains('x') {
            // No mode we can be sure of: it may be any of them.
            self.out.push(Risk::Overwrite {
                names: Vec::new(),
                checked: false,
            });
        }
    }

    /// `python -c`, `node -e`, `perl -e`, `ruby -e`: code in the command line. Running a script
    /// file or a module is left to the sandbox.
    fn interpreter(&mut self, name: &str, args: &[Word], whole: &Command) {
        let mut opaque = false;
        let mut runs_something = false;
        let mut i = 0;
        'words: while let Some(word) = args.get(i) {
            i += 1;
            if is_dashes(word) || word.lead_dynamic {
                runs_something = true;
                break;
            }
            let text = word.text.as_str();
            if text == "-" && !word.dynamic {
                opaque = true; // the program is read from standard input
                break;
            }
            if !text.starts_with('-') {
                runs_something = true;
                break;
            }
            if word.dynamic {
                // `--eval=$CODE`, `-c$CODE`, `-$FLAGS`: the shell fills in an option, which may
                // be the one that carries the code.
                opaque = true;
                break;
            }
            if let Some(long) = text.strip_prefix("--") {
                let key = long.split('=').next().unwrap_or(long);
                match (name, key) {
                    ("node", "eval" | "print") => {
                        opaque = true;
                        break;
                    }
                    (
                        "node",
                        "require" | "import" | "loader" | "experimental-loader" | "conditions",
                    ) if !long.contains('=') => {
                        i += 1;
                    }
                    _ => {}
                }
                continue;
            }
            let chars: Vec<char> = text[1..].chars().collect();
            for (k, ch) in chars.iter().enumerate() {
                let last = k + 1 == chars.len();
                match (name, ch) {
                    ("python", 'c') | ("node", 'e' | 'p') | ("perl", 'e' | 'E') | ("ruby", 'e') => {
                        opaque = true;
                        break 'words;
                    }
                    ("python", 'm') => {
                        runs_something = true;
                        break 'words;
                    }
                    ("python", 'W' | 'X')
                    | ("node", 'r' | 'C')
                    | ("perl", 'I')
                    | ("ruby", 'r' | 'I') => {
                        i += usize::from(last);
                        break;
                    }
                    ("perl", 'i') => break,
                    _ => {}
                }
            }
        }
        if opaque {
            self.out
                .push(Risk::Opaque("встроенный код в строке запуска"));
        } else if !runs_something
            && (whole.after_pipe
                || whole
                    .redirs
                    .iter()
                    .any(|(kind, _)| matches!(kind, RedirKind::Heredoc | RedirKind::Input)))
        {
            self.out.push(Risk::Opaque(
                "код передан интерпретатору через стандартный ввод",
            ));
        }
    }

    /// `curl -o FILE` / `-O` (the name taken from the address).
    fn curl(&mut self, args: &[Word]) {
        let mut outputs: Vec<Word> = Vec::new();
        let mut remote_name = false;
        let mut dir: Option<Word> = None;
        let mut urls: Vec<&Word> = Vec::new();
        let mut i = 0;
        while let Some(word) = args.get(i) {
            i += 1;
            if is_operand(word) {
                urls.push(word);
                continue;
            }
            if let Some(long) = word.text.strip_prefix("--") {
                let (key, inline) = match long.split_once('=') {
                    Some((key, inline)) => (key, Some(inline)),
                    None => (long, None),
                };
                let take = |i: &mut usize| match inline {
                    Some(text) => Some(inline_value(word, text)),
                    None => take_value(word, args, i),
                };
                match key {
                    "output" => outputs.extend(take(&mut i)),
                    "output-dir" => dir = take(&mut i),
                    "remote-name" | "remote-name-all" => remote_name = true,
                    "header" | "data" | "data-raw" | "data-binary" | "data-urlencode"
                    | "request" | "user" | "user-agent" | "referer" | "cookie" | "cookie-jar"
                    | "form" | "max-time" | "connect-timeout" | "retry" | "proxy"
                    | "upload-file" | "write-out" | "range" | "resolve" | "cacert" | "cert"
                    | "key" | "config" => {
                        let _ = take(&mut i);
                    }
                    // `--$X`: an option the shell fills in may be `--output`.
                    "" if word.dynamic => outputs.push(unknown_value()),
                    _ => {}
                }
                continue;
            }
            let chars: Vec<char> = word.text[1..].chars().collect();
            if chars.is_empty() {
                // `-$X`
                outputs.push(unknown_value());
            }
            for (k, ch) in chars.iter().enumerate() {
                let last = k + 1 == chars.len();
                if *ch == 'O' {
                    remote_name = true;
                } else if "HdXuAeEFKmrTUwxyYzbcCDtQo".contains(*ch) {
                    let taken = if last {
                        take_value(word, args, &mut i)
                    } else {
                        let rest: String = chars[k + 1..].iter().collect();
                        Some(inline_value(word, &rest))
                    };
                    if *ch == 'o' {
                        outputs.extend(taken);
                    }
                    break;
                }
            }
        }
        if remote_name {
            for url in urls {
                if url.dynamic {
                    outputs.push(url.clone());
                    continue;
                }
                let clean = url.text.split(['?', '#']).next().unwrap_or_default();
                if let Some(name) = clean.rsplit('/').next().filter(|n| !n.is_empty())
                    && clean.contains("://")
                {
                    outputs.push(Word {
                        text: name.to_owned(),
                        ..Word::default()
                    });
                }
            }
        }
        self.outputs(outputs, dir);
    }

    /// `wget -O FILE`.
    fn wget(&mut self, args: &[Word]) {
        let mut outputs: Vec<Word> = Vec::new();
        let mut i = 0;
        while let Some(word) = args.get(i) {
            i += 1;
            if is_operand(word) {
                continue;
            }
            if let Some(long) = word.text.strip_prefix("--") {
                if long == "output-document" {
                    outputs.extend(take_value(word, args, &mut i));
                } else if let Some(text) = long.strip_prefix("output-document=") {
                    outputs.push(inline_value(word, text));
                } else if long.is_empty() && word.dynamic {
                    // `--$X`: may be `--output-document`.
                    outputs.push(unknown_value());
                }
                continue;
            }
            let chars: Vec<char> = word.text[1..].chars().collect();
            if chars.is_empty() {
                // `-$X`
                outputs.push(unknown_value());
            }
            for (k, ch) in chars.iter().enumerate() {
                if "OoaiPetTwlQUBDIXAR".contains(*ch) {
                    let taken = if k + 1 == chars.len() {
                        take_value(word, args, &mut i)
                    } else {
                        let rest: String = chars[k + 1..].iter().collect();
                        Some(inline_value(word, &rest))
                    };
                    if *ch == 'O' {
                        outputs.extend(taken);
                    }
                    break;
                }
            }
        }
        self.outputs(outputs, None);
    }

    /// Files a downloader will write, relative to `--output-dir` when there is one.
    fn outputs(&mut self, outputs: Vec<Word>, dir: Option<Word>) {
        for mut word in outputs {
            if word.text == "-" && !word.dynamic {
                continue;
            }
            if let Some(dir) = &dir
                && !Path::new(&word.text).is_absolute()
            {
                if dir.dynamic {
                    word.dynamic = true;
                } else {
                    word.text = format!("{}/{}", dir.text.trim_end_matches('/'), word.text);
                }
            }
            overwrites(self.fs, &word, self.cwd.as_deref(), self.out);
        }
    }

    /// `unzip`: without `-n` it replaces the files that already have the same names.
    fn unzip(&mut self, args: &[Word]) {
        let mut harmless = false;
        let mut archive: Option<&Word> = None;
        let mut dest: Option<Word> = None;
        let mut i = 0;
        while let Some(word) = args.get(i) {
            i += 1;
            if word.dynamic || !word.text.starts_with('-') {
                archive.get_or_insert(word);
                continue;
            }
            let chars: Vec<char> = word.text[1..].chars().collect();
            for (k, ch) in chars.iter().enumerate() {
                let last = k + 1 == chars.len();
                match ch {
                    // Listing, testing, printing, zipinfo, comment; `-n` never overwrites.
                    'l' | 't' | 'v' | 'p' | 'c' | 'Z' | 'z' | 'n' => harmless = true,
                    'd' => {
                        dest = if last {
                            i += 1;
                            args.get(i - 1).cloned()
                        } else {
                            Some(Word {
                                text: chars[k + 1..].iter().collect(),
                                ..word.clone()
                            })
                        };
                        break;
                    }
                    'P' => {
                        i += usize::from(last);
                        break;
                    }
                    _ => {}
                }
            }
        }
        if harmless {
            return;
        }
        // Into a folder that is not there yet nothing can be replaced.
        if let Some(dest) = &dest
            && !dest.dynamic
            && resolve(self.cwd.as_deref(), &dest.text).is_some_and(|path| !self.fs.exists(&path))
        {
            return;
        }
        self.out.push(Risk::Extract {
            archive: archive.map(shown),
        });
    }

    /// `patch`: rewrites the files the patch names, or the one given.
    fn patch(&mut self, args: &[Word]) {
        if has_long(args, "--dry-run") || has_short(args, 'C') || has_long(args, "--check") {
            return;
        }
        let mut operands: Vec<&Word> = Vec::new();
        let mut output: Option<Word> = None;
        let mut i = 0;
        while let Some(word) = args.get(i) {
            i += 1;
            if is_operand(word) {
                operands.push(word);
                continue;
            }
            let text = word.text.as_str();
            if text == "--output" || text == "-o" {
                output = take_value(word, args, &mut i);
            } else if let Some(value) = text.strip_prefix("--output=") {
                output = Some(inline_value(word, value));
            } else if let Some(value) = text.strip_prefix("-o").filter(|v| !v.starts_with('-')) {
                // `-ofile`, `-o$FILE`
                output = Some(inline_value(word, value));
            } else if !word.dynamic
                && matches!(
                    text,
                    "-i" | "-d"
                        | "-r"
                        | "-B"
                        | "-z"
                        | "-V"
                        | "-F"
                        | "-D"
                        | "-p"
                        | "--input"
                        | "--directory"
                        | "--reject-file"
                        | "--strip"
                )
            {
                i += 1;
            }
        }
        if let Some(output) = output {
            overwrites(self.fs, &output, self.cwd.as_deref(), self.out);
            return;
        }
        match operands.first() {
            Some(file) => {
                let mut found = Vec::new();
                overwrites(self.fs, file, self.cwd.as_deref(), &mut found);
                for risk in found {
                    if let Risk::Overwrite { names, .. } = risk {
                        self.out.push(Risk::Patch { names });
                    }
                }
            }
            None => self.out.push(Risk::Patch { names: Vec::new() }),
        }
    }

    /// `awk -i inplace` (gawk): the named files are rewritten.
    fn awk(&mut self, args: &[Word], listed: bool) {
        let mut in_place = false;
        let mut script_given = false;
        let mut ops: Vec<&Word> = Vec::new();
        let mut i = 0;
        // `inplace` (or a library the shell names at run time, which may be it).
        let include =
            |word: &Word, value: &str| word.dynamic || value == "inplace" || value == "inplace.awk";
        while let Some(word) = args.get(i) {
            i += 1;
            if is_operand(word) {
                ops.push(word);
                continue;
            }
            let text = word.text.as_str();
            if text == "--include" || text == "-i" {
                let value = if word.dynamic {
                    Some(unknown_value())
                } else {
                    args.get(i).cloned()
                };
                in_place |= value.is_some_and(|w| include(&w, &w.text));
                i += usize::from(!word.dynamic);
            } else if let Some(value) = text.strip_prefix("--include=") {
                in_place |= include(word, value);
            } else if let Some(value) = text.strip_prefix("-i").filter(|v| !v.starts_with('-')) {
                in_place |= include(word, value);
            } else if text == "-f" || text == "--file" || text == "-e" || text == "--source" {
                script_given = true;
                i += usize::from(!word.dynamic);
            } else if text.starts_with("-f") || text.starts_with("--file=") {
                script_given = true;
            } else if !word.dynamic && matches!(text, "-F" | "-v" | "-W") {
                i += 1;
            } else if word.dynamic && matches!(text, "-" | "--") {
                // `-$X`: may be `-i inplace`.
                in_place = true;
            }
        }
        if !in_place {
            return;
        }
        let files: Vec<&Word> = if script_given {
            ops
        } else {
            ops.into_iter().skip(1).collect()
        };
        self.rewrite(&files, listed);
    }

    fn dd(&mut self, args: &[Word]) {
        for word in args {
            let Some(target) = word.text.strip_prefix("of=") else {
                continue;
            };
            let word = Word {
                text: target.to_owned(),
                ..word.clone()
            };
            overwrites(self.fs, &word, self.cwd.as_deref(), self.out);
        }
    }

    fn tee(&mut self, args: &[Word]) {
        if has_short(args, 'a') || has_long(args, "--append") {
            return;
        }
        for word in operands(args) {
            if word.text != "-" {
                overwrites(self.fs, word, self.cwd.as_deref(), self.out);
            }
        }
    }

    fn git(&mut self, args: &[Word]) {
        let mut i = 0;
        while let Some(word) = args.get(i) {
            if is_operand(word) {
                break;
            }
            i += if !word.dynamic
                && matches!(
                    word.text.as_str(),
                    "-C" | "-c" | "--git-dir" | "--work-tree"
                ) {
                2
            } else {
                1
            };
        }
        let Some(sub) = args.get(i) else {
            return;
        };
        if sub.dynamic {
            // The subcommand itself is filled in by the shell.
            self.out
                .push(Risk::Opaque("подкоманда git задаётся подстановкой"));
            return;
        }
        let rest = &args[i + 1..];
        let flag = |text: &str| rest.iter().any(|w| !w.dynamic && w.text == text);
        // An option the shell fills in may be `--hard`, `--force`, `--`.
        let hidden = rest.iter().any(is_dynamic_option);
        let after_dashes = || -> Vec<String> {
            match rest.iter().position(|w| w.text == "--") {
                Some(at) => rest[at + 1..].iter().map(shown).collect(),
                None => Vec::new(),
            }
        };
        let paths = || -> Vec<String> { operands(rest).iter().map(|w| shown(w)).collect() };
        let kind = match sub.text.as_str() {
            "clean" if !(flag("--dry-run") || has_short(rest, 'n')) => {
                Some((GitKind::Clean, Vec::new()))
            }
            "reset" if flag("--hard") || hidden => Some((GitKind::ResetHard, Vec::new())),
            "checkout" if flag("--") || flag(".") || flag("-f") || flag("--force") || hidden => {
                let mut names = after_dashes();
                if names.is_empty() && flag(".") {
                    names.push(".".into());
                }
                Some((GitKind::Checkout, names))
            }
            "switch" if flag("-f") || flag("--force") || flag("--discard-changes") || hidden => {
                Some((GitKind::Checkout, Vec::new()))
            }
            "restore" if !(flag("--staged") && !flag("--worktree") && !has_short(rest, 'W')) => {
                Some((GitKind::Restore, paths()))
            }
            "rm" if !flag("--cached") => Some((GitKind::Remove, paths())),
            "apply"
                if !(flag("--check")
                    || flag("--stat")
                    || flag("--numstat")
                    || flag("--summary")
                    || (flag("--cached") && !flag("--index"))) =>
            {
                self.out.push(Risk::Patch { names: Vec::new() });
                None
            }
            _ => None,
        };
        if let Some((kind, names)) = kind {
            self.out.push(Risk::Git { kind, names });
        }
    }

    fn shell_interpreter(&mut self, args: &[Word], whole: &Command) {
        if args
            .iter()
            .any(|w| is_dynamic_option(w) || (!w.dynamic && flag_chars(&w.text).contains('c')))
        {
            self.out.push(Risk::Opaque("команда в строке для оболочки"));
        } else if !operands(args).is_empty() {
            self.out.push(Risk::Opaque("запуск скрипта"));
        } else if whole.after_pipe {
            self.out
                .push(Risk::Opaque("текст команды передан в оболочку"));
        }
    }

    fn change_dir(&mut self, args: &[Word]) {
        let target = operands(args).into_iter().next();
        *self.cwd = match target {
            Some(word)
                if !self.grouped
                    && !word.dynamic
                    && word.text != "-"
                    && !word.text.starts_with('~') =>
            {
                resolve(self.cwd.as_deref(), &word.text)
            }
            _ => None,
        };
    }
}

/// File names in the folder of `pattern` that match its last component; `None` when the folder
/// part itself has a pattern.
fn expand_glob(fs: &dyn Fs, cwd: Option<&Path>, pattern: &str) -> Option<Vec<String>> {
    let (folder, file) = match pattern.rsplit_once('/') {
        Some((folder, file)) => (if folder.is_empty() { "/" } else { folder }, file),
        None => (".", pattern),
    };
    if folder.contains(['*', '?', '[']) {
        return None;
    }
    let dir = resolve(cwd, folder)?;
    let mut names = fs.list(&dir)?;
    names.retain(|name| (!name.starts_with('.') || file.starts_with('.')) && wildcard(file, name));
    Some(names)
}

/// `*` and `?` against a file name.
fn wildcard(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|skip| go(rest, &n[skip..])),
            Some(('?', rest)) => !n.is_empty() && go(rest, &n[1..]),
            Some((c, rest)) => n.first() == Some(c) && go(rest, &n[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}
