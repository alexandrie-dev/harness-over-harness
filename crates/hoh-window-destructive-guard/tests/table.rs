//! Command → ask / allow, against a real folder: `a.txt b.txt notes.md dir/x.txt backup/a.txt`.

use std::fs;
use std::path::Path;

use hoh_window_destructive_guard::{Risk, assess, escalation_question, question};

fn folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for file in ["a.txt", "b.txt", "notes.md", "dir/x.txt", "backup/a.txt"] {
        let path = dir.path().join(file);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, "old").expect("write");
    }
    dir
}

fn risks(dir: &Path, command: &str) -> Vec<Risk> {
    assess(command, Some(dir))
}

#[track_caller]
fn asks(dir: &Path, command: &str) {
    assert!(
        !risks(dir, command).is_empty(),
        "must ask, but passes: {command}"
    );
}

#[track_caller]
fn passes(dir: &Path, command: &str) {
    let found = risks(dir, command);
    assert!(
        found.is_empty(),
        "must pass, but asks ({found:?}): {command}"
    );
}

#[test]
fn deleting_commands_ask() {
    let dir = folder();
    for command in [
        "rm a.txt",
        "rm -rf dir",
        "rm -f nothing.txt",
        "/bin/rm a.txt",
        "\\rm a.txt",
        "'rm' a.txt",
        "rmdir dir",
        "unlink a.txt",
        "shred -u a.txt",
        "truncate -s 0 a.txt",
        "ls && rm a.txt",
        "ls; rm a.txt",
        "false || rm a.txt",
        "echo hi | rm a.txt",
        "sudo rm a.txt",
        "env FOO=1 rm a.txt",
        "FOO=1 rm a.txt",
        "timeout 5 rm a.txt",
        "command rm a.txt",
        "nohup rm a.txt",
        "if true; then rm a.txt; fi",
        "for f in a b; do rm $f; done",
        "rm \"a.txt\"",
        "rm -- -weird",
        "find . -name '*.log' -delete",
        "find . -type f -exec rm {} \\;",
        "find . -type f -exec rm -f {} +",
        "find . -execdir rm {} \\;",
        "ls | xargs rm",
        "ls | xargs -n1 rm -f",
        "find . -print0 | xargs -0 rm",
        "echo $(rm a.txt)",
        "echo `rm a.txt`",
        "echo \"$(rm a.txt)\"",
        "diff <(rm a.txt) b.txt",
        "x=$(rm a.txt)",
        "cat <<EOF\n$(rm a.txt)\nEOF",
    ] {
        asks(dir.path(), command);
    }
}

#[test]
fn deleting_commands_name_the_files_and_say_why_it_hurts() {
    let dir = folder();
    let found = risks(dir.path(), "rm old1.txt old2.txt old3.txt");
    let ask = question(&found, "rm old1.txt old2.txt old3.txt", Some(dir.path()));
    assert!(
        ask.headline
            .starts_with("Удалить 3 файла: old1.txt, old2.txt, old3.txt (папка "),
        "{}",
        ask.headline
    );
    assert!(ask.headline.contains("не попадут в корзину"));
    assert_eq!(ask.details, "Команда: rm old1.txt old2.txt old3.txt");
    assert!(
        !ask.headline.contains("rm old1"),
        "the raw command stays in the details"
    );
    // A-03: the window's explanation is words and the files, never the raw command.
    assert!(
        ask.explanation
            .contains("Удалить 3 файла: old1.txt, old2.txt, old3.txt")
            && ask.explanation.contains("не попадут в корзину"),
        "{}",
        ask.explanation
    );
    assert!(
        !ask.explanation.contains("rm ") && !ask.explanation.contains("Команда:"),
        "{}",
        ask.explanation
    );
}

#[test]
fn overwrite_asks_only_when_the_target_exists_or_cannot_be_checked() {
    let dir = folder();
    for command in [
        "echo hi > a.txt",
        "echo hi >a.txt",
        "echo hi 2> a.txt",
        "echo hi &> a.txt",
        "echo hi >| a.txt",
        "echo hi > \"a.txt\"",
        ": > a.txt",
        "> a.txt",
        "cat notes.md > a.txt",
        "echo hi > $OUT",
        "echo hi > ~/x",
        "echo hi > dir/../a.txt",
        "cp b.txt a.txt",
        "mv b.txt a.txt",
        "cp notes.md backup/a.txt",
        "cp notes.md a.txt",
        "cp a.txt backup",
        "cp a.txt backup/",
        "cp -r a.txt backup",
        "mv a.txt backup",
        "cp -t backup a.txt",
        "cp --target-directory=backup a.txt",
        "cp *.txt backup",
        "cp a.txt $DEST",
        "cp $SRC backup",
        "cp a.txt b.txt notes.md",
        "ls | xargs mv -t backup",
        "dd if=/dev/zero of=a.txt bs=1 count=1",
        "echo x | tee a.txt",
        "echo x | tee b.txt a.txt",
        "cd dir && echo x > x.txt",
        "cd dir; cp ../a.txt x.txt",
        "cd $SOMEWHERE && echo x > new.txt",
        "(cd dir); echo x > a.txt",
    ] {
        asks(dir.path(), command);
    }
}

/// A-03: the explanation lists every file (up to twenty) where the one-line headline stops at four,
/// and for every kind of question (delete, overwrite, in-place edit) it carries no raw command.
#[test]
fn the_explanation_lists_the_files_and_never_the_raw_command() {
    let dir = folder();
    let command = "rm f1.txt f2.txt f3.txt f4.txt f5.txt f6.txt";
    let ask = question(&risks(dir.path(), command), command, Some(dir.path()));
    assert!(
        ask.headline
            .contains("f1.txt, f2.txt, f3.txt, f4.txt и ещё 2")
    );
    assert!(
        ask.explanation
            .contains("f1.txt, f2.txt, f3.txt, f4.txt, f5.txt, f6.txt"),
        "{}",
        ask.explanation
    );
    for command in [
        command,
        "echo hi > a.txt",
        "sed -i s/old/new/ a.txt",
        "cp -f b.txt a.txt",
    ] {
        let ask = question(&risks(dir.path(), command), command, Some(dir.path()));
        assert!(
            !ask.explanation.contains("Команда:")
                && !ask.explanation.contains(command)
                && !ask.explanation.contains("echo")
                && !ask.explanation.contains("sed -i"),
            "{command}: {}",
            ask.explanation
        );
        assert!(ask.details.starts_with("Команда: "), "{command}");
        assert!(
            ask.explanation.contains("a.txt") || command.starts_with("rm"),
            "{command}: {}",
            ask.explanation
        );
    }
}

/// A-03: the explanation of an escalation is built by the guard alone; not one word of the model's
/// justification enters it, however it quotes the command. Command and justification sit together in
/// the details, within the protocol's size bound.
#[test]
fn an_escalation_explanation_has_no_word_of_the_justification_and_the_details_have_both() {
    let dir = folder();
    for (command, justification) in [
        ("rm old.txt", "нужно запустить rm old.txt"),
        ("rm /tmp/old.txt", "нужно запустить RM \"/tmp/old.txt\""),
        ("rm  old.txt", "run `rm old.txt` to clean up"),
        ("rm -rf dir", "очистить dir через Rm -Rf, он устарел"),
        ("rm a", "perform a cleanup"),
        ("ls", "show details about the directory"),
        ("rm /tmp/archive/old.txt", "rm old.txt"),
    ] {
        let ask = escalation_question(&risks(dir.path(), command), command, justification);
        let words: Vec<&str> = justification
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() > 3)
            .collect();
        for word in words {
            assert!(
                !ask.explanation
                    .to_lowercase()
                    .contains(&word.to_lowercase()),
                "{justification:?}: {word:?} in {}",
                ask.explanation
            );
        }
        assert!(
            ask.explanation
                .ends_with("Обоснование Помощника — в подробностях"),
            "{}",
            ask.explanation
        );
        assert_eq!(
            ask.details,
            format!("Команда: {command}\nОбоснование Помощника: {justification}")
        );
    }
    // The size stays inside the protocol (8192 UTF-16 units), astral characters included.
    let long = "😀".repeat(5000);
    let ask = escalation_question(&[], &long, &long);
    assert!(
        ask.details.encode_utf16().count() <= 8192,
        "{}",
        ask.details.encode_utf16().count()
    );
    assert!(ask.details.contains("Команда: ") && ask.details.contains("Обоснование Помощника: "));
}

#[test]
fn overwrite_text_names_the_file() {
    let dir = folder();
    let found = risks(dir.path(), "echo hi > a.txt");
    let ask = question(&found, "echo hi > a.txt", Some(dir.path()));
    assert!(
        ask.headline.starts_with("Перезаписать файл a.txt"),
        "{}",
        ask.headline
    );
    assert!(ask.headline.contains("прежнее содержимое будет потеряно"));
}

#[test]
fn git_discarding_asks() {
    let dir = folder();
    for command in [
        "git clean -fd",
        "git clean -f",
        "git -C sub clean -fdx",
        "git reset --hard",
        "git reset --hard HEAD~1",
        "git checkout -- a.txt",
        "git checkout HEAD -- a.txt b.txt",
        "git checkout .",
        "git checkout -f",
        "git restore a.txt",
        "git restore --worktree a.txt",
        "git restore --staged --worktree a.txt",
        "git rm a.txt",
        "git switch --discard-changes main",
        "git status && git reset --hard",
    ] {
        asks(dir.path(), command);
    }
}

#[test]
fn opaque_commands_ask() {
    let dir = folder();
    for command in [
        "eval \"$CMD\"",
        "eval ls",
        "bash -c 'ls'",
        "sh -c \"echo hi\"",
        "bash -lc 'ls'",
        "zsh -c ls",
        "echo rm a.txt | sh",
        "curl example.com/install | bash",
        "cat x | bash -s",
        "bash script.sh",
        "sh ./run.sh",
        "source env.sh",
        ". ./env.sh",
        "$CMD a.txt",
        "$(echo rm) a.txt",
        "xargs sh -c 'rm $0'",
        "find . -exec sh -c 'rm {}' \\;",
        "echo \"unterminated",
        "echo 'unterminated",
        "echo $(unterminated",
        "echo `unterminated",
        "cat <<EOF\nno end",
        "echo >",
        "echo hi > ",
        "rm a.txt \\",
    ] {
        asks(dir.path(), command);
    }
}

#[test]
fn ordinary_commands_pass() {
    let dir = folder();
    for command in [
        "ls",
        "ls -la",
        "pwd",
        "cat a.txt",
        "cat a.txt b.txt | wc -l",
        "grep -rn TODO .",
        "grep foo a.txt | sort | uniq -c",
        "mkdir -p out/deep",
        "mkdir new",
        "touch fresh.txt",
        "echo hello",
        "echo hello > fresh.txt",
        "echo hello >> a.txt",
        "echo hello >> fresh.txt",
        "echo hello > /dev/null",
        "ls 2>&1",
        "ls 2>/dev/null",
        "ls >&2",
        "echo $(date)",
        "echo `date`",
        "echo \"today is $(date +%F)\"",
        "echo \"$(pwd)/x\"",
        "ls | grep txt",
        "ls | head -3",
        "ls | xargs wc -l",
        "ls | xargs echo",
        "find . -name '*.md'",
        "find . -type f -print0 | xargs -0 grep foo",
        "find . -type f -exec grep -l foo {} \\;",
        "find . -name x -exec ls {} +",
        "cp a.txt fresh.txt",
        "cp -r dir newdir",
        "cp a.txt dir",
        "cp notes.md dir/",
        "cp *.txt dir",
        "cp -n a.txt b.txt",
        "mv a.txt moved.txt",
        "mv dir olddir",
        "mv -n a.txt b.txt",
        "wc -l a.txt",
        "head -n 3 notes.md",
        "tail -f a.txt",
        "echo x | tee -a a.txt",
        "echo x | tee fresh.txt",
        "echo x | tee /dev/null",
        "dd if=a.txt of=copy.bin",
        "dd if=a.txt of=/dev/null",
        "git status",
        "git log --oneline -5",
        "git diff",
        "git add .",
        "git commit -m 'msg'",
        "git checkout main",
        "git checkout -b feature",
        "git clean -n",
        "git clean --dry-run -fd",
        "git reset --soft HEAD~1",
        "git reset",
        "git restore --staged a.txt",
        "git rm --cached a.txt",
        "git stash",
        "git switch main",
        "git pull --rebase",
        "cd dir && ls",
        "cd dir && echo x > fresh.txt",
        "cd dir && cat x.txt",
        "cd .. && ls",
        "sed -n 1,5p a.txt",
        "sort a.txt | uniq > fresh.txt",
        "diff a.txt b.txt",
        "diff <(sort a.txt) <(sort b.txt)",
        "cargo build --release",
        "python3 -V",
        "npm test",
        "echo 'rm a.txt'",
        "echo \"rm -rf /\"",
        "echo rm",
        "grep -n 'rm ' notes.md",
        "cat <<EOF\nrm a.txt\nEOF",
        "cat <<'EOF'\n$(rm a.txt)\nEOF",
        "cat > fresh.txt <<EOF\nhello\nEOF",
        "ls # rm a.txt",
        "bash --version",
        "command -v rm",
        "which rm",
        "type rm",
        "FOO=1 ls",
        "env",
        "env FOO=1 ls",
        "sudo ls",
        "timeout 5 ls",
        "true && false || echo done",
        "ls & wait",
        "for f in a b; do echo $f; done",
        "while read l; do echo \"$l\"; done < a.txt",
        "echo hello | cat > fresh.txt",
        "printf 'a\\n' > fresh.txt",
        "echo a b c",
        "tar czf out.tgz dir",
        "{ echo a; echo b; } > fresh.txt",
        "ls $(pwd)",
        "echo ${HOME}",
        "echo $((1 + 2))",
        "ls -la | sort -k5 | head",
    ] {
        passes(dir.path(), command);
    }
}

#[test]
fn quoting_hides_nothing_and_adds_nothing() {
    let dir = folder();
    asks(dir.path(), "r\\m a.txt");
    asks(dir.path(), "\"r\"\"m\" a.txt");
    asks(dir.path(), "echo ok; rm 'a.txt'");
    asks(dir.path(), "echo \"a;b\"; rm a.txt");
    passes(dir.path(), "echo \"a;rm a.txt\"");
    passes(dir.path(), "echo 'a && rm a.txt'");
    passes(dir.path(), "echo a\\;rm");
}

#[test]
fn working_folder_is_the_session_folder() {
    let dir = folder();
    // `> a.txt` exists here, does not in the empty folder.
    asks(dir.path(), "echo x > a.txt");
    let empty = tempfile::tempdir().expect("tempdir");
    passes(empty.path(), "echo x > a.txt");
    // Unknown folder: a relative target cannot be checked.
    assert!(!assess("echo x > a.txt", None).is_empty());
    assert!(assess("echo x > /dev/null", None).is_empty());
    assert!(assess("ls", None).is_empty());
}

#[test]
fn text_is_one_clean_line_with_a_bounded_list() {
    let dir = folder();
    let command = "rm f1 f2 f3 f4 f5 f6 f7";
    let ask = question(&risks(dir.path(), command), command, Some(dir.path()));
    assert!(
        ask.headline
            .contains("Удалить 7 файлов: f1, f2, f3, f4 и ещё 3"),
        "{}",
        ask.headline
    );

    let command = "rm 'evil\n\u{202e}name' x";
    let ask = question(&risks(dir.path(), command), command, Some(dir.path()));
    assert!(
        hoh_protocol::approval::validate_human_text(&ask.headline).is_ok(),
        "{:?}",
        ask.headline
    );
    assert!(!ask.headline.contains('\n'));

    let long = format!("rm {}", "n".repeat(5000));
    let ask = question(&risks(dir.path(), &long), &long, Some(dir.path()));
    assert!(hoh_protocol::approval::validate_human_text(&ask.headline).is_ok());
    assert!(ask.explanation.chars().count() <= 2000);
    assert!(ask.details.chars().count() <= 2000);
}

#[test]
fn plural_forms() {
    let dir = folder();
    for (count, word) in [
        (1, "1 файл:"),
        (2, "2 файла:"),
        (5, "5 файлов:"),
        (11, "11 файлов:"),
        (21, "21 файл:"),
        (22, "22 файла:"),
    ] {
        let names: Vec<String> = (0..count).map(|i| format!("f{i}")).collect();
        let command = format!("rm {}", names.join(" "));
        let ask = question(&risks(dir.path(), &command), &command, Some(dir.path()));
        assert!(ask.headline.contains(word), "{count}: {}", ask.headline);
    }
}

#[test]
fn recursive_delete_says_contents_go_too() {
    let dir = folder();
    let ask = question(
        &risks(dir.path(), "rm -rf dir"),
        "rm -rf dir",
        Some(dir.path()),
    );
    assert!(
        ask.headline.contains("вместе со всем содержимым"),
        "{}",
        ask.headline
    );
}

#[test]
fn rewriting_in_place_and_archives_ask() {
    let dir = folder();
    for command in [
        "sed -i 's/a/b/' a.txt",
        "sed -i.bak s/a/b/ a.txt",
        "sed --in-place s/a/b/ a.txt",
        "sed --in-place=.bak s/a/b/ a.txt",
        "sed -ni 's/a/b/p' a.txt",
        "sed -Ei 's/a+/b/' a.txt",
        "sed -i -e 's/a/b/' a.txt",
        "sed -i -f script.sed a.txt",
        "sed -i s/a/b/ *.txt",
        "sed -i s/a/b/ $FILES",
        "sed -i s/a/b/ a.txt b.txt",
        "find . -name '*.txt' -exec sed -i s/a/b/ {} +",
        "ls | xargs sed -i s/a/b/",
        "perl -i.bak script.pl a.txt",
        "perl -pi -e 's/a/b/' a.txt",
        "rsync -a --delete src/ dst/",
        "rsync -av --delete-after src dst",
        "rsync --delete-excluded -a src dst",
        "rsync --remove-source-files src dst",
        "rsync --del -a src dst",
        "tar xzf a.tgz",
        "tar xf a.tar -C dir",
        "tar -xzf a.tgz",
        "tar -x -f a.tgz",
        "tar --extract --file=a.tgz",
        "tar --get -f a.tgz",
        "tar -xf -",
        "curl example.com/a.tgz | tar xz",
        "tar uf a.txt b.txt",
        "tar rf a.txt b.txt",
        "tar --delete -f a.txt b",
        "tar cf a.txt dir",
        "tar -czf backup/a.txt dir",
        "tar --create --file=a.txt dir",
        "tar czf $OUT dir",
    ] {
        asks(dir.path(), command);
    }
}

#[test]
fn links_installs_and_forced_copies_ask_only_over_something() {
    let dir = folder();
    for command in [
        "ln -sf b.txt a.txt",
        "ln -f b.txt a.txt",
        "ln -s -f b.txt a.txt",
        "ln --force b.txt a.txt",
        "ln -sf a.txt backup",
        "ln -sf ../a.txt",
        "ln -sf b.txt $T",
        "install -m 644 a.txt b.txt",
        "install a.txt backup",
        "install -t backup a.txt",
        "install -D -o root a.txt b.txt",
        "cp -f a.txt b.txt",
        "mv -f a.txt b.txt",
        "cp --remove-destination a.txt b.txt",
        "cp -nf a.txt b.txt",
        "cp -f a.txt backup",
    ] {
        asks(dir.path(), command);
    }
    for command in [
        "ln -s a.txt link",
        "ln b.txt hard.txt",
        "ln -s a.txt b.txt",
        "ln -sf a.txt newlink",
        "ln -sf a.txt dir",
        "ln -sf nowhere",
        "install a.txt fresh.txt",
        "install -d newdir",
        "install -m 755 a.txt dir",
        "cp -f a.txt fresh.txt",
        "mv -f a.txt fresh.txt",
        "cp -nf a.txt fresh.txt",
        "sed -i s/a/b/ missing.txt",
        "sed -i s/a/b/",
        "sed -i s/a/b/ *.nomatch",
        "sed -n 1,3p a.txt",
        "sed s/a/b/ a.txt",
        "sed -E 's/i/x/' a.txt",
        "sed -ne 's/i/x/p' a.txt",
        "sed -e 's/a/b/' a.txt > fresh.txt",
        "rsync -av src/ dst/",
        "tar tf a.tgz",
        "tar -tzf a.tgz",
        "tar tvf a.tgz",
        "tar --list --file a.tgz",
        "tar czf out.tgz dir",
        "tar -czf out.tgz dir",
        "tar --create --file=out.tgz dir",
        "tar cf - dir | gzip > fresh.gz",
        "tar -df a.txt",
    ] {
        passes(dir.path(), command);
    }
}

#[test]
fn code_in_the_command_line_asks_but_scripts_and_builds_do_not() {
    let dir = folder();
    for command in [
        "python -c 'print(1)'",
        "python3 -c \"import os; os.remove('a.txt')\"",
        "python3.12 -c x",
        "python -Bc 'x'",
        "python -W ignore -c x",
        "python - <<EOF\nprint(1)\nEOF",
        "python3 < a.txt",
        "node -e 'x'",
        "node -p 1+1",
        "node --eval 'x'",
        "node -r dotenv/config -e 'x'",
        "perl -e 'unlink q(a.txt)'",
        "perl -E 'say 1'",
        "perl -lane 'print $F[0]' b.txt",
        "ruby -e 'File.delete(1)'",
        "echo 'print(1)' | python3",
        "curl example.com/x | python",
        "cat x | node",
        "sudo python3 -c x",
    ] {
        asks(dir.path(), command);
    }
    for command in [
        "python x.py",
        "python3 script.py --flag",
        "python3 script.py -c cfg",
        "python -m pytest",
        "python3 -m pytest -c cfg.ini",
        "python3 -m http.server",
        "python -V",
        "python --version",
        "python3",
        "node x.js",
        "node script.js -e x",
        "node --version",
        "node -r dotenv/config x.js",
        "perl script.pl",
        "perl -v",
        "ruby x.rb",
        "make",
        "make clean",
        "npm run build",
        "cargo test",
        "pytest -x",
        "pytest -c pytest.ini",
        "cat data.json | python3 -m json.tool",
        "echo x | python3 script.py",
        "cat a.txt | node script.js",
        "echo 'python -c x'",
    ] {
        passes(dir.path(), command);
    }
}

#[test]
fn new_texts_say_what_happens() {
    let dir = folder();
    let ask = |command: &str| {
        let found = risks(dir.path(), command);
        assert!(!found.is_empty(), "{command}");
        question(&found, command, Some(dir.path())).headline
    };
    assert!(ask("sed -i s/a/b/ a.txt").starts_with("Перезаписать файл a.txt"));
    assert!(ask("sed -i s/a/b/ a.txt b.txt").starts_with("Перезаписать файлы a.txt, b.txt"));
    assert!(ask("tar xzf a.tgz").starts_with("Распаковать архив a.tgz и перезаписать файлы"));
    assert!(ask("tar xf -").starts_with("Распаковать архив и перезаписать файлы"));
    let rsync = ask("rsync -a --delete src/ dst/");
    assert!(
        rsync.contains("rsync --delete") && rsync.contains("не попадут в корзину"),
        "{rsync}"
    );
    assert!(ask("python -c x").contains("встроенный код"));
    assert!(ask("ln -sf b.txt a.txt").starts_with("Перезаписать файл a.txt"));
    assert!(ask("install a.txt backup").contains("backup/a.txt"));
}

#[test]
fn downloads_unpacking_patches_and_awk_ask_only_over_something() {
    let dir = folder();
    for command in [
        "curl -o a.txt https://example.com/x",
        "curl -sSo a.txt https://example.com/x",
        "curl -oa.txt https://example.com/x",
        "curl --output a.txt https://example.com/x",
        "curl --output=a.txt https://example.com/x",
        "curl -L -H 'A: b' -o notes.md https://example.com/x",
        "curl -o fresh.txt -o a.txt https://example.com/x https://example.com/y",
        "curl -o $OUT https://example.com/x",
        "curl --output-dir backup -o a.txt https://example.com/x",
        "curl -O https://example.com/dl/a.txt",
        "curl -sSLO https://example.com/dl/notes.md?x=1",
        "curl -O $URL",
        "wget -O a.txt https://example.com/x",
        "wget -qO a.txt https://example.com/x",
        "wget -Oa.txt https://example.com/x",
        "wget --output-document=a.txt https://example.com/x",
        "wget --output-document a.txt https://example.com/x",
        "unzip a.zip",
        "unzip -o a.zip",
        "unzip -q a.zip -d dir",
        "unzip a.zip -d $OUT",
        "unzip -d . a.zip",
        "patch -p1 < fix.diff",
        "patch notes.md fix.diff",
        "patch -p1 -i fix.diff",
        "patch -R -p1 < fix.diff",
        "patch -o a.txt orig.txt fix.diff",
        "git apply fix.patch",
        "git apply --index fix.patch",
        "git apply --reject fix.patch",
        "awk -i inplace '{print}' a.txt",
        "awk -iinplace '{print}' a.txt",
        "gawk --include=inplace '{print}' a.txt b.txt",
        "awk -i inplace -f prog.awk a.txt",
        "awk -i inplace -v x=1 '{print x}' *.txt",
        "ls | xargs awk -i inplace '{print}'",
    ] {
        asks(dir.path(), command);
    }
    for command in [
        "curl https://example.com/x",
        "curl -s https://example.com/x | head",
        "curl -o fresh.txt https://example.com/x",
        "curl -o - https://example.com/x",
        "curl -o /dev/null https://example.com/x",
        "curl --output-dir dir -o fresh.txt https://example.com/x",
        "curl -O https://example.com/dl/fresh.txt",
        "curl -X POST -d o -H 'A: a.txt' https://example.com/x",
        "wget https://example.com/x",
        "wget -O fresh.txt https://example.com/x",
        "wget -O - https://example.com/x",
        "wget -q -P a.txt https://example.com/x",
        "unzip -l a.zip",
        "unzip -lv a.zip",
        "unzip -t a.zip",
        "unzip -n a.zip",
        "unzip -p a.zip x.txt",
        "unzip -Z a.zip",
        "unzip a.zip -d fresh_dir",
        "patch --dry-run -p1 < fix.diff",
        "patch -C notes.md fix.diff",
        "patch -o fresh.txt orig.txt fix.diff",
        "patch fresh.txt fix.diff",
        "git apply --check fix.patch",
        "git apply --stat fix.patch",
        "git apply --cached fix.patch",
        "awk '{print $1}' a.txt",
        "awk -F: '{print $1}' a.txt > fresh.txt",
        "awk -i inplace '{print}' missing.txt",
        "gawk -i other '{print}' a.txt",
        "awk -v inplace=1 '{print}' a.txt",
    ] {
        passes(dir.path(), command);
    }
}

#[test]
fn escalated_questions_are_worded_like_the_guards() {
    use hoh_window_destructive_guard::escalation_question;
    let dir = folder();
    let command = "rm -rf dir";
    let found = risks(dir.path(), command);
    let ask = escalation_question(&found, command, "clean the build output");
    assert!(
        ask.headline.starts_with(
            "Помощник просит разрешения работать за пределами своей папки: удалить 1 элемент вместе со всем содержимым: dir"
        ),
        "{}",
        ask.headline
    );
    assert!(ask.headline.contains("Файлы не попадут в корзину"));
    assert_eq!(
        ask.details,
        "Команда: rm -rf dir\nОбоснование Помощника: clean the build output"
    );
    // Everything the headline says is not said again: only where the reasons are.
    assert_eq!(ask.explanation, "Обоснование Помощника — в подробностях");
    assert!(
        !ask.explanation.contains("rm -rf")
            && !ask.explanation.contains("clean")
            && !ask.explanation.contains("Команда:"),
        "{}",
        ask.explanation
    );
    let plain = escalation_question(&[], "ls /", "look around");
    assert_eq!(
        plain.headline,
        "Помощник просит разрешения работать за пределами своей папки: он сможет читать и менять любые файлы на этом компьютере"
    );
}

#[test]
fn new_download_and_patch_texts() {
    let dir = folder();
    let ask = |command: &str| {
        let found = risks(dir.path(), command);
        assert!(!found.is_empty(), "{command}");
        question(&found, command, Some(dir.path())).headline
    };
    assert!(ask("curl -o a.txt https://e.com/x").starts_with("Перезаписать файл a.txt"));
    assert!(ask("unzip a.zip").starts_with("Распаковать архив a.zip и перезаписать файлы"));
    assert!(
        ask("patch notes.md fix.diff").starts_with("Применить патч и перезаписать файл notes.md")
    );
    assert!(
        ask("git apply fix.patch")
            .starts_with("Применить патч и перезаписать файлы, которые он меняет")
    );
    assert!(ask("awk -i inplace '{print}' a.txt").starts_with("Перезаписать файл a.txt"));
}

#[test]
fn tar_mode_does_not_depend_on_the_order_of_options() {
    let dir = folder();
    for command in [
        "tar --file=a.txt --create dir",
        "tar --create --file=a.txt dir",
        "tar --file a.txt --create dir",
        "tar -f a.txt -c dir",
        "tar -cfa.txt dir",
        "tar --file=a.tar --extract",
        "tar --extract --file=a.tar",
        "tar --file=a.tar -x",
        "tar -f a.tar --get",
        "tar --file=a.txt --update b.txt",
        "tar --file=a.txt --delete b",
        "tar -vf a.txt -c dir",
    ] {
        asks(dir.path(), command);
    }
    for command in [
        "tar --file=fresh.tar --create dir",
        "tar --create --file=fresh.tar dir",
        "tar -f fresh.tar -c dir",
        "tar --file=a.tgz --list",
        "tar --list --file=a.tgz",
        "tar -f a.tgz -t",
    ] {
        passes(dir.path(), command);
    }
}

#[test]
fn wrappers_that_change_folder_move_the_checks_with_them() {
    let dir = folder();
    // `x.txt` exists only in `dir`; `a.txt` only here.
    for command in [
        "env -C dir cp ../a.txt x.txt",
        "env -C dir cp ../b.txt x.txt",
        "env --chdir dir cp ../a.txt x.txt",
        "env --chdir=dir cp ../a.txt x.txt",
        "env -Cdir cp ../a.txt x.txt",
        "env -i -C dir FOO=1 cp ../a.txt x.txt",
        "sudo -D dir cp ../a.txt x.txt",
        "sudo --chdir=dir mv ../a.txt x.txt",
        "env -C dir sed -i s/a/b/ x.txt",
        "env -C dir tar --file=x.txt --create .",
        "env -C $WHERE cp a.txt b.txt",
        "env -C dir -C $WHERE cp a.txt b.txt",
        "env -S 'rm a.txt'",
        "env -S\"cp a b\"",
        "env --split-string='cp a b'",
    ] {
        asks(dir.path(), command);
    }
    for command in [
        "env -C dir cp ../a.txt fresh.txt",
        "env --chdir=dir cp ../a.txt fresh.txt",
        // The folder change belongs to the wrapped command only.
        "env -C dir ls && echo x > x.txt",
        "env -C dir ls; cp a.txt fresh.txt",
        // A redirection is the shell's: it is opened in the folder the command line runs in.
        "env -C dir echo hi > x.txt",
        "env -C dir echo hi",
        "env FOO=1 cp a.txt fresh.txt",
    ] {
        passes(dir.path(), command);
    }
}

/// A target — or an option that carries one — with an expansion in it (`$OUT`, `$(…)`, a mask)
/// cannot be checked, so a command that may overwrite or delete asks. One group per branch of the
/// classifier; the same command with a plain name for a new file passes.
#[test]
fn a_target_the_shell_fills_in_is_unverifiable_and_asks() {
    let dir = folder();
    let groups: &[(&str, &[&str], &[&str])] = &[
        (
            "tar",
            &[
                "tar --create --file=$OUT dir",
                "tar --file=$OUT --update x",
                "tar --delete --file=$OUT x",
                "tar --file=$OUT --create dir",
                "tar --file=\"$DIR/out.tar\" --create dir",
                "tar --create --file=$(echo a.txt) dir",
                "tar --create --file=`echo a.txt` dir",
                "tar -cf $OUT dir",
                "tar -c -f$OUT dir",
                "tar -cf\"$OUT\" dir",
                "tar cf $OUT dir",
                "tar $OPTS dir",
                "tar c$X out.tar dir",
                "tar -c --$X dir",
                "tar -$FLAGS dir",
                "tar --create --file=*.nomatch dir",
            ],
            &[
                "tar --create --file=fresh.tar dir",
                "tar --list --file=$OUT",
                "tar -tf $OUT",
                "tar -tzf $(ls)",
                "tar -cf - dir",
                "tar --create --file=- dir",
            ],
        ),
        (
            "cp / mv / install / ln",
            &[
                "cp --target-directory=$D a.txt",
                "cp -t$D a.txt",
                "cp -t $D a.txt",
                "cp -vt$D a.txt",
                "mv -t$D a.txt",
                "install -t$D a.txt",
                "ln -sf --target-directory=$D a.txt",
                "cp -$F a.txt fresh.txt",
                "cp -n -t$D a.txt",
                "cp -n -$F a.txt fresh.txt",
                "ln -s -$F a.txt link",
                "cp a.txt $(echo b.txt)",
                "cp a.txt `echo b.txt`",
                "cp a.txt *.nomatch",
                // an attached literal folder is read too
                "cp -tbackup a.txt",
                "mv -tbackup a.txt",
            ],
            &[
                "cp -tdir a.txt",
                "cp -vt dir a.txt",
                "cp --target-directory=dir a.txt",
            ],
        ),
        (
            "sed / perl / awk in place",
            &[
                "sed -i$SUF s/a/b/ a.txt",
                "sed --in-place=$SUF s/a/b/ a.txt",
                "sed -$F s/a/b/ a.txt",
                "sed --$F s/a/b/ a.txt",
                "perl -i$SUF script.pl a.txt",
                "perl -$F script.pl a.txt",
                "awk -i $LIB '{print}' a.txt",
                "awk -i$LIB '{print}' a.txt",
                "awk --include=$LIB '{print}' a.txt",
                "awk -$F '{print}' a.txt",
                "awk -i inplace '{print}' $FILE",
                "sed -i s/a/b/ $(ls)",
            ],
            &[
                "sed -n$X s/a/b/ fresh.txt",
                "sed -i s/a/b/ fresh.txt",
                "awk -F$SEP '{print}' fresh.txt",
                "awk -v x=$X '{print}' fresh.txt",
                "awk -i inplace '{print}' fresh.txt",
                // a mask with no match is not a file that exists: nothing to rewrite
                "sed -i s/a/b/ *.nomatch",
            ],
        ),
        (
            "rsync",
            &[
                "rsync -a --delete$X src dst",
                "rsync -a --delete-$WHEN src dst",
                "rsync -a --$OPT src dst",
                "rsync -a -$OPT src dst",
            ],
            &["rsync -a --exclude=$PAT src dst", "rsync -a $SRC dst"],
        ),
        (
            "python / node / perl / ruby",
            &[
                "node --eval=$CODE",
                "node -e$CODE",
                "node -$FLAGS x.js",
                "python -c$CODE",
                "python -$FLAGS x.py",
                "python -m$X",
                "perl -e$CODE",
                "ruby -e$CODE",
            ],
            &[
                "python $SCRIPT",
                "python x.py $ARGS",
                "node x.js --eval=$X",
                "node $SCRIPT",
                "python x.py -c$X",
            ],
        ),
        (
            "curl",
            &[
                "curl --output=$OUT https://e.com/x",
                "curl -o$OUT https://e.com/x",
                "curl -sSo$OUT https://e.com/x",
                "curl -o $OUT https://e.com/x",
                "curl --output $OUT https://e.com/x",
                "curl -$F https://e.com/x",
                "curl --$F https://e.com/x",
                "curl --output-dir=$D -o fresh.txt https://e.com/x",
                "curl -o *.nomatch https://e.com/x",
                "curl -o $(echo a.txt) https://e.com/x",
            ],
            &[
                "curl -o fresh.txt https://e.com/x",
                "curl -H 'A: $X' https://e.com/x",
                "curl -s $URL",
                "curl -d $DATA https://e.com/x",
                "curl --header=$H https://e.com/x",
            ],
        ),
        (
            "wget",
            &[
                "wget --output-document=$OUT https://e.com/x",
                "wget -O$OUT https://e.com/x",
                "wget -qO$OUT https://e.com/x",
                "wget -O $OUT https://e.com/x",
                "wget -$F https://e.com/x",
                "wget --$F https://e.com/x",
                "wget -O *.nomatch https://e.com/x",
            ],
            &[
                "wget -O fresh.txt https://e.com/x",
                "wget -P $DIR https://e.com/x",
                "wget $URL",
            ],
        ),
        (
            "unzip",
            &[
                "unzip -d$D a.zip",
                "unzip $OPTS a.zip",
                "unzip a.zip -d $(pwd)",
            ],
            &["unzip -l $ARCHIVE"],
        ),
        (
            "patch / git apply",
            &[
                "patch -o$OUT orig.txt fix.diff",
                "patch --output=$OUT orig.txt fix.diff",
                "patch -o $OUT orig.txt fix.diff",
                "patch --output=a.txt orig.txt fix.diff",
                "patch -oa.txt orig.txt fix.diff",
                "patch $TARGET fix.diff",
                "git apply $PATCH",
            ],
            &[
                "patch --output=fresh.txt orig.txt fix.diff",
                "patch -ofresh.txt orig.txt fix.diff",
                "patch --dry-run $TARGET fix.diff",
                "git apply --check $PATCH",
            ],
        ),
        (
            "git",
            &[
                "git $SUB clean",
                "git -C $D $SUB",
                "git reset --$X",
                "git checkout --$X a.txt",
                "git switch -$F main",
                "git clean -$F",
            ],
            &[
                "git checkout $BRANCH",
                "git reset $COMMIT",
                "git status $X",
                "git log --format=$F",
                "git -C $D status",
                "git clean -n$X",
            ],
        ),
        (
            "find",
            &[
                "find . -del$X",
                "find . -$ACTION",
                "find . -fprint a.txt",
                "find . -fprint $OUT",
                "find . -fls a.txt",
                "find . -fprintf a.txt %p",
            ],
            &[
                "find . -fprint fresh.txt",
                "find . -name $PAT",
                "find $DIR -type f",
                "find . -newer $REF",
            ],
        ),
        (
            "redirections / dd / tee",
            &[
                "echo x > $OUT",
                "echo x >| $OUT",
                "echo x &> $OUT",
                "echo x 2> $ERR",
                "echo x > *.nomatch",
                "echo x > $(echo a.txt)",
                "echo x 2>&$N",
                "dd if=a.txt of=$OUT",
                "dd if=a.txt of=\"$OUT\"",
                "dd if=a.txt of=$(echo a.txt)",
                "echo x | tee $OUT",
                "echo x | tee -- $OUT",
                "echo x | tee fresh.txt $OUT",
            ],
            &[
                "echo x >> $OUT",
                "dd if=$IN of=fresh.bin",
                "echo x | tee -a $OUT",
                "echo x > fresh.txt",
                "echo x 2>&1",
            ],
        ),
    ];
    for (branch, asking, passing) in groups {
        for command in asking.iter() {
            assert!(
                !risks(dir.path(), command).is_empty(),
                "[{branch}] must ask (unverifiable target), but passes: {command}"
            );
        }
        for command in passing.iter() {
            let found = risks(dir.path(), command);
            assert!(
                found.is_empty(),
                "[{branch}] must pass, but asks ({found:?}): {command}"
            );
        }
    }
}

#[test]
fn a_word_that_starts_with_an_expansion_is_not_an_option() {
    let dir = folder();
    // `$X-n` is not `-n`: the shell decides what it starts with.
    for command in [
        "git clean $X-n",
        "cp $X-n a.txt b.txt",
        "tar $X-tf a.tgz",
        "bash $X-c",
    ] {
        asks(dir.path(), command);
    }
    passes(dir.path(), "git clean -n$X");
    // an option we cannot read may be `-t DIR`
    asks(dir.path(), "cp -n$X a.txt fresh.txt");
}
