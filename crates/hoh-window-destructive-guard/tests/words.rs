//! The window's words for people who never wrote code (HOH-35, audit `ux-noncoder-audit.md`, fixes
//! 15 and 17): the headline starts with the action, the explanation does not repeat it, no
//! "песочница". Only the words changed: what is asked, what passes, and the details are pinned
//! below as they were before the change.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use hoh_window_destructive_guard::{
    GitKind, RealFs, Risk, Source, assess, decide, escalation_question, question,
};

fn folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for file in ["a.txt", "b.txt", "notes.md", "dir/x.txt", "backup/a.txt"] {
        let path = dir.path().join(file);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, "old").expect("write");
    }
    dir
}

/// Command → what the classifier finds, recorded on the code before the wording change. An empty
/// list is a command that runs without a question.
const BEFORE: &[(&str, &str)] = &[
    (
        "rm old1.txt old2.txt",
        r#"[Delete { names: ["old1.txt", "old2.txt"], folders: false, source: Listed }]"#,
    ),
    (
        "rm -rf dir",
        r#"[Delete { names: ["dir"], folders: true, source: Listed }]"#,
    ),
    (
        "rm f1 f2 f3 f4 f5 f6 f7",
        r#"[Delete { names: ["f1", "f2", "f3", "f4", "f5", "f6", "f7"], folders: false, source: Listed }]"#,
    ),
    (
        "rm *.txt",
        r#"[Delete { names: ["*.txt"], folders: false, source: Listed }]"#,
    ),
    (
        "find . -name '*.log' -delete",
        "[Delete { names: [], folders: true, source: Find }]",
    ),
    (
        "ls | xargs rm",
        "[Delete { names: [], folders: false, source: Xargs }]",
    ),
    (
        "rsync -a --delete src/ dst/",
        "[Delete { names: [], folders: true, source: Sync }]",
    ),
    (
        "echo hi > a.txt",
        r#"[Overwrite { names: ["a.txt"], checked: true }]"#,
    ),
    ("echo hi > fresh.txt", "[]"),
    (
        "sed -i s/a/b/ a.txt b.txt",
        r#"[Overwrite { names: ["a.txt", "b.txt"], checked: true }]"#,
    ),
    (
        "cp -f b.txt a.txt",
        r#"[Overwrite { names: ["a.txt"], checked: true }]"#,
    ),
    ("truncate -s 0 a.txt", r#"[Truncate { names: ["a.txt"] }]"#),
    ("tar xzf a.tgz", r#"[Extract { archive: Some("a.tgz") }]"#),
    ("unzip a.zip", r#"[Extract { archive: Some("a.zip") }]"#),
    (
        "patch notes.md fix.diff",
        r#"[Patch { names: ["notes.md"] }]"#,
    ),
    ("git apply fix.patch", "[Patch { names: [] }]"),
    ("git clean -fd", "[Git { kind: Clean, names: [] }]"),
    ("git reset --hard", "[Git { kind: ResetHard, names: [] }]"),
    (
        "git checkout -- a.txt",
        r#"[Git { kind: Checkout, names: ["a.txt"] }]"#,
    ),
    (
        "git restore a.txt",
        r#"[Git { kind: Restore, names: ["a.txt"] }]"#,
    ),
    (
        "git rm a.txt",
        r#"[Git { kind: Remove, names: ["a.txt"] }]"#,
    ),
    (
        "python -c x",
        r#"[Opaque("встроенный код в строке запуска")]"#,
    ),
    ("eval \"$X\"", r#"[Opaque("eval")]"#),
    ("rm 'unclosed", "[Unparsable]"),
    (
        "curl -o a.txt https://e.com/x",
        r#"[Overwrite { names: ["a.txt"], checked: true }]"#,
    ),
    (
        "ln -sf b.txt a.txt",
        r#"[Overwrite { names: ["a.txt"], checked: true }]"#,
    ),
    (
        "install a.txt backup",
        r#"[Overwrite { names: ["backup/a.txt"], checked: true }]"#,
    ),
    (
        "rm a.txt; echo hi > b.txt",
        r#"[Delete { names: ["a.txt"], folders: false, source: Listed }, Overwrite { names: ["b.txt"], checked: true }]"#,
    ),
    ("ls -la", "[]"),
    ("cat a.txt", "[]"),
    ("mkdir new", "[]"),
    ("touch fresh.txt", "[]"),
    ("python3 script.py", "[]"),
    ("cargo build", "[]"),
];

/// Protection is not weakened by the new words: every command is classified, asked or let through
/// exactly as before, and the folded details still carry the command (and the justification).
#[tokio::test]
async fn only_the_words_changed_not_what_is_asked() {
    let dir = folder();
    for (command, before) in BEFORE {
        let risks = assess(command, Some(dir.path()));
        assert_eq!(&format!("{risks:?}"), before, "{command}");
        let asked = decide(command, Some(dir.path().to_path_buf()), Arc::new(RealFs)).await;
        assert_eq!(asked.is_some(), *before != "[]", "{command}");
        if let Some(asked) = asked {
            assert_eq!(asked.details, format!("Команда: {command}"));
        }
        // A call that asks to run outside the folder is always asked, whatever it does.
        let escalated = escalation_question(&risks, command, "why");
        assert!(!escalated.headline.is_empty());
        assert_eq!(
            escalated.details,
            format!("Команда: {command}\nОбоснование Помощника: why")
        );
    }
}

fn delete(names: &[&str], folders: bool) -> Risk {
    Risk::Delete {
        names: names.iter().map(|name| (*name).to_owned()).collect(),
        folders,
        source: Source::Listed,
    }
}

const WORK: &str = "/srv/hoh-work";

#[test]
fn the_headline_starts_with_the_action_and_the_explanation_does_not_repeat_it() {
    let work = Some(Path::new(WORK));
    let ask = question(
        &[delete(&["old-1.txt", "old-2.txt"], false)],
        "rm old-1.txt old-2.txt",
        work,
    );
    assert_eq!(
        ask.headline,
        "Удалить 2 файла: old-1.txt, old-2.txt (папка /srv/hoh-work). Файлы не попадут в \
         корзину: вернуть их будет нельзя"
    );
    // Nothing more to say: the same text, which the window shows once.
    assert_eq!(ask.explanation, ask.headline);
    assert_eq!(ask.details, "Команда: rm old-1.txt old-2.txt");

    let six = ["f1", "f2", "f3", "f4", "f5", "f6"];
    let ask = question(&[delete(&six, false)], "rm f1 f2 f3 f4 f5 f6", work);
    assert_eq!(
        ask.headline,
        "Удалить 6 файлов: f1, f2, f3, f4 и ещё 2 (папка /srv/hoh-work). Файлы не попадут в \
         корзину: вернуть их будет нельзя"
    );
    assert_eq!(
        ask.explanation,
        "Удалить 6 файлов: f1, f2, f3, f4, f5, f6 (папка /srv/hoh-work). Файлы не попадут в \
         корзину: вернуть их будет нельзя"
    );

    let ask = question(&[delete(&["dir"], true)], "rm -rf dir", None);
    assert_eq!(
        ask.headline,
        "Удалить 1 элемент вместе со всем содержимым: dir. Файлы не попадут в корзину: вернуть \
         их будет нельзя"
    );

    let overwrite = Risk::Overwrite {
        names: vec!["b.txt".to_owned()],
        checked: true,
    };
    let ask = question(std::slice::from_ref(&overwrite), "echo hi > b.txt", work);
    assert_eq!(
        ask.headline,
        "Перезаписать файл b.txt: прежнее содержимое будет потеряно (папка /srv/hoh-work)"
    );
    assert_eq!(ask.explanation, ask.headline);

    let ask = question(
        &[delete(&["a.txt"], false), overwrite],
        "rm a.txt; echo hi > b.txt",
        work,
    );
    assert_eq!(
        ask.headline,
        "Удалить 1 файл: a.txt; перезаписать файл b.txt: прежнее содержимое будет потеряно \
         (папка /srv/hoh-work). Файлы не попадут в корзину: вернуть их будет нельзя"
    );

    let reset = Risk::Git {
        kind: GitKind::ResetHard,
        names: Vec::new(),
    };
    assert_eq!(
        question(&[reset], "git reset --hard", work).headline,
        "Сбросить все несохранённые изменения до последнего коммита; изменения пропадут (папка \
         /srv/hoh-work)"
    );
    assert_eq!(
        question(&[Risk::Opaque("eval")], "eval \"$X\"", work).headline,
        "Запустить то, что нельзя проверить заранее (eval); оно может удалить или перезаписать \
         файлы (папка /srv/hoh-work)"
    );
}

#[test]
fn a_call_outside_the_folder_is_asked_without_jargon() {
    let ask = escalation_question(&[], "ls /", "look around");
    assert_eq!(
        ask.headline,
        "Помощник просит разрешения работать за пределами своей папки: он сможет читать и менять \
         любые файлы на этом компьютере"
    );
    assert_eq!(ask.explanation, "Обоснование Помощника — в подробностях");
    assert_eq!(
        ask.details,
        "Команда: ls /\nОбоснование Помощника: look around"
    );

    let ask = escalation_question(&[delete(&["old.txt"], false)], "rm old.txt", "clean up");
    assert_eq!(
        ask.headline,
        "Помощник просит разрешения работать за пределами своей папки: удалить 1 файл: old.txt. \
         Файлы не попадут в корзину: вернуть их будет нельзя"
    );
    assert_eq!(ask.explanation, "Обоснование Помощника — в подробностях");

    let six = ["f1", "f2", "f3", "f4", "f5", "f6"];
    let ask = escalation_question(&[delete(&six, false)], "rm f1 f2 f3 f4 f5 f6", "clean up");
    assert_eq!(
        ask.headline,
        "Помощник просит разрешения работать за пределами своей папки: удалить 6 файлов: f1, f2, \
         f3, f4 и ещё 2. Файлы не попадут в корзину: вернуть их будет нельзя"
    );
    assert_eq!(
        ask.explanation,
        "Помощник просит разрешения работать за пределами своей папки: удалить 6 файлов: f1, f2, \
         f3, f4, f5, f6. Файлы не попадут в корзину: вернуть их будет нельзя. Обоснование \
         Помощника — в подробностях"
    );
}

/// Words the audit's dictionary keeps out of the window, and the old service opening the headline
/// no longer has; none of them in any question or explanation.
#[test]
fn no_question_uses_the_words_the_window_must_not_show() {
    const BANNED: &[&str] = &[
        "песочниц",
        "Выполнить команду",
        "опасн",
        "Высокий риск",
        "Помощник не объяснил",
        "провайдер",
        "движок",
    ];
    let dir = folder();
    for (command, _) in BEFORE {
        let risks = assess(command, Some(dir.path()));
        let mut texts = vec![escalation_question(&risks, command, "why")];
        if !risks.is_empty() {
            texts.push(question(&risks, command, Some(dir.path())));
        }
        for text in texts {
            for shown in [&text.headline, &text.explanation] {
                for word in BANNED {
                    assert!(!shown.contains(word), "{command}: {word:?} in {shown}");
                }
            }
        }
    }
}

/// The string literals of a Rust source; comments (line, block, doc) are left out. Enough of a
/// lexer for this check: raw strings, escapes, char literals and lifetimes are told apart.
fn string_literals(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let at = |index: usize| chars.get(index).copied();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (c, next) = (chars[i], at(i + 1));
        let after_word = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if c == 'r' && matches!(next, Some('"' | '#')) && !after_word {
            let mut j = i + 1;
            let mut hashes = 0;
            while at(j) == Some('#') {
                hashes += 1;
                j += 1;
            }
            if at(j) != Some('"') {
                // A raw identifier (`r#type`).
                i = j;
                continue;
            }
            let start = j + 1;
            let mut end = start;
            while end < chars.len()
                && !(chars[end] == '"' && (1..=hashes).all(|k| at(end + k) == Some('#')))
            {
                end += 1;
            }
            found.push(chars[start..end.min(chars.len())].iter().collect());
            i = end + 1 + hashes;
        } else if c == '"' {
            let mut j = i + 1;
            let mut text = String::new();
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' {
                    j += 1;
                }
                if let Some(c) = at(j) {
                    text.push(c);
                }
                j += 1;
            }
            found.push(text);
            i = j + 1;
        } else if c == '\'' {
            if next == Some('\\') {
                // An escaped char literal: '\n', '\'', '\u{00ad}'.
                let mut j = i + 3;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
            } else if at(i + 2) == Some('\'') {
                i += 3;
            } else {
                // A lifetime.
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    found
}

fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("source folder") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}

/// HOH-35: no text these crates send to a person says "песочница". Every string literal of their
/// sources is checked (a superset of what reaches the window); comments do not count.
#[test]
fn no_string_of_the_window_crates_says_sandbox() {
    // The lexer itself: a comment is skipped, a string is found, raw strings and char literals
    // do not confuse it.
    let sample = "// песочница\n/* песочница */ let a = '\"'; let b = \"без неё\"; \
                  let c = r#\"в \"песочнице\"\"#; fn f<'a>(x: &'a str) {}";
    let literals = string_literals(sample);
    assert_eq!(literals, ["без неё", "в \"песочнице\""]);

    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut checked = 0;
    for name in [
        "hoh-user-approval",
        "hoh-external-harness",
        "hoh-destructive-classifier",
        "hoh-window-destructive-guard",
    ] {
        for file in rust_files(&crates.join(name).join("src")) {
            let source = fs::read_to_string(&file).expect("source");
            for literal in string_literals(&source) {
                checked += 1;
                assert!(
                    !literal.to_lowercase().contains("песочниц"),
                    "{}: {literal:?}",
                    file.display()
                );
            }
        }
    }
    // The escalation wording of the approval service is among what was checked.
    let host = fs::read_to_string(crates.join("hoh-user-approval/src/host.rs")).expect("host.rs");
    assert!(
        string_literals(&host)
            .iter()
            .any(|literal| literal.contains("с дополнительными правами"))
    );
    assert!(checked > 500, "{checked} literals");
}
