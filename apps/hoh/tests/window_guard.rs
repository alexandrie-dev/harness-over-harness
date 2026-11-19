//! Window profile: bash commands that may delete or overwrite an existing file are put to the
//! user first (`hoh-window-destructive-guard`), through the same `approval/request` + mac path
//! as every other host approval. Harness copied from `window_profile.rs` (owned by another writer).
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

async fn send(stdin: &mut tokio::process::ChildStdin, frame: Value) {
    stdin
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
}

async fn receive_until(
    lines: &mut Lines<BufReader<tokio::process::ChildStdout>>,
    seen: &mut Vec<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let line = lines.next_line().await.unwrap().expect("engine stdout EOF");
            let frame: Value = serde_json::from_str(&line).unwrap();
            seen.push(frame.clone());
            if predicate(&frame) {
                return frame;
            }
        }
    })
    .await
    .expect("engine response timeout")
}

/// A user profile that points the model host at the scenario mock; the real key flow is the
/// window end-to-end test's business, the mock needs no key.
fn write_profile(root: &Path, base_url: &str) {
    let dir = root.join("home").join("profiles").join("window");
    std::fs::create_dir_all(&dir).unwrap();
    // The credentials store refuses a home that others can read: the launcher creates it private.
    std::fs::set_permissions(
        root.join("home"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(
        dir.join("package.json"),
        json!({"name":"dsh-profile-window","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@hoh/window"]}}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("cordis.patch.yml"),
        format!("- id: model-host\n  config:\n    baseUrl: {base_url}\n    model: window-mock\n"),
    )
    .unwrap();
}

fn command(root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hoh"));
    command
        .args(["--profile", "window"])
        .env_clear()
        .env("HOME", root)
        .env("DSH_HOME", root.join("home"))
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(root.join("engine.stderr")).unwrap())
        .kill_on_drop(true);
    command
}

const APPROVAL_KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn host_initialize(root: &Path) -> Value {
    json!({
        "jsonrpc":"2.0",
        "id":"initialize",
        "method":"initialize",
        "params":{
            "cwd":root,
            "provider":"openai-compatible",
            "model":"window-mock",
            "hoh":{"protocolVersion":1,"approvalKey":APPROVAL_KEY},
            "capabilities":{"approvals":"host"}
        }
    })
}

fn scenario(
    responses: Vec<hoh_llm_openai_mock::ScenarioResponse>,
) -> hoh_llm_openai_mock::Scenario {
    hoh_llm_openai_mock::Scenario {
        model: "window-mock".to_owned(),
        responses,
    }
}

fn say(text: &str) -> hoh_llm_openai_mock::ScenarioResponse {
    hoh_llm_openai_mock::ScenarioResponse {
        text: vec![text.to_owned()],
        tool_calls: vec![],
        usage: hoh_llm_openai_mock::ScenarioUsage {
            prompt_tokens: 3,
            completion_tokens: 2,
        },
    }
}

/// One bash call of the scripted model; `n` keeps the call ids of a scenario apart.
fn bash(n: usize, command: &str) -> hoh_llm_openai_mock::ScenarioResponse {
    hoh_llm_openai_mock::ScenarioResponse {
        text: vec![],
        tool_calls: vec![hoh_llm_openai_mock::ScenarioToolCall {
            id: format!("call-bash-{n}"),
            name: "bash".to_owned(),
            arguments: json!({"command": command, "description": "work in the folder"}),
        }],
        usage: hoh_llm_openai_mock::ScenarioUsage {
            prompt_tokens: 3,
            completion_tokens: 2,
        },
    }
}

/// A scratch folder outside `/tmp`: `workspace-write` always leaves `/tmp` writable, so a folder
/// there would make "outside the workspace" meaningless.
fn sandboxed_root() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

struct Engine {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: Lines<BufReader<tokio::process::ChildStdout>>,
    seen: Vec<Value>,
    mock: hoh_llm_openai_mock::MockServer,
    work: std::path::PathBuf,
}

/// An engine in host-approval mode over a mock that plays `responses`; `work` is the workspace
/// the session is opened in.
async fn start_engine(
    root: &Path,
    responses: Vec<hoh_llm_openai_mock::ScenarioResponse>,
) -> Engine {
    let mock = hoh_llm_openai_mock::MockServer::start(hoh_llm_openai_mock::MockOptions::new(
        scenario(responses),
    ))
    .await
    .unwrap();
    write_profile(root, &mock.base_url);
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut child = command(root).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    let mut init = host_initialize(&work);
    init["params"]["cwd"] = json!(work);
    send(&mut stdin, init).await;
    let ready = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    assert!(ready.get("result").is_some(), "{ready}");
    Engine {
        child,
        stdin,
        stdout,
        seen,
        mock,
        work,
    }
}

impl Engine {
    async fn prompt(&mut self, text: &str) {
        send(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt",
                "params":{"sessionId":"window-main","contentBlocks":[{"type":"text","text":text}]}}),
        )
        .await;
        receive_until(&mut self.stdout, &mut self.seen, |f| f["id"] == "prompt").await;
    }

    /// The next frame is either the engine asking the host or the turn ending.
    async fn next_approval(&mut self) -> Option<hoh_protocol::HostApprovalRequest> {
        let frame = receive_until(&mut self.stdout, &mut self.seen, |f| {
            f["method"] == "approval/request"
                || (f["method"] == "session.status" && f["params"]["status"] == "idle")
        })
        .await;
        (frame["method"] == "approval/request").then(|| {
            self.seen.pop();
            self.seen.push(frame.clone());
            serde_json::from_value(frame["params"].clone()).unwrap()
        })
    }

    async fn answer(&mut self, outcome: &str) {
        let asked = self
            .seen
            .iter()
            .rev()
            .find(|f| f["method"] == "approval/request")
            .cloned()
            .expect("no approval/request to answer");
        let request: hoh_protocol::HostApprovalRequest =
            serde_json::from_value(asked["params"].clone()).unwrap();
        let key = hoh_protocol::approval::key_from_hex(APPROVAL_KEY).unwrap();
        send(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","id":asked["id"],"result":{
                "approvalId":request.approval_id,
                "outcome":outcome,
                "mac":hoh_protocol::approval::mac(&key, &request, outcome)
            }}),
        )
        .await;
    }

    async fn finish(mut self) {
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(30), self.child.wait())
            .await
            .expect("engine exit timeout")
            .unwrap();
        assert_eq!(status.code(), Some(0));
        self.mock.close().await;
    }
}

fn write_files(dir: &Path, names: &[&str]) {
    for name in names {
        std::fs::write(dir.join(name), "old content").unwrap();
    }
}

fn all_exist(dir: &Path, names: &[&str]) -> bool {
    names.iter().all(|name| dir.join(name).exists())
}

/// The assistant deletes three files in the session folder: the host gets an `approval/request`
/// (mac-checked by the engine) worded for a person, and a rejection leaves every file in place.
#[tokio::test]
async fn deleting_files_asks_with_a_human_text_and_a_rejection_keeps_them() {
    let root = sandboxed_root();
    let command = "rm old1.txt old2.txt old3.txt";
    let mut engine = start_engine(root.path(), vec![bash(1, command), say("done")]).await;
    let names = ["old1.txt", "old2.txt", "old3.txt"];
    write_files(&engine.work, &names);
    engine.prompt("clean up the old files").await;

    let asked = engine
        .next_approval()
        .await
        .expect("deleting files must reach the host as approval/request");
    assert_eq!(asked.tool_name, "bash");
    assert!(
        asked.human_text.starts_with("Выполнить команду в папке "),
        "{}",
        asked.human_text
    );
    assert!(
        asked
            .human_text
            .contains("удалить 3 файла: old1.txt, old2.txt, old3.txt"),
        "{}",
        asked.human_text
    );
    assert!(asked.human_text.contains("Файлы не попадут в корзину"));
    assert!(
        !asked.human_text.contains("rm "),
        "the raw command is for the details"
    );
    assert!(
        asked
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains(command)),
        "{:?}",
        asked.reason
    );
    assert!(
        all_exist(&engine.work, &names),
        "the command ran before the answer"
    );

    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert!(
        all_exist(&engine.work, &names),
        "a rejected delete removed files"
    );
    engine.finish().await;
}

/// `allowed-once` deletes — and the next identical call asks again: the answer is not remembered.
#[tokio::test]
async fn an_approved_delete_runs_once_and_the_next_one_asks_again() {
    let root = sandboxed_root();
    let command = "rm old1.txt old2.txt old3.txt";
    let mut engine = start_engine(
        root.path(),
        vec![bash(1, command), bash(2, command), say("done")],
    )
    .await;
    let names = ["old1.txt", "old2.txt", "old3.txt"];
    write_files(&engine.work, &names);
    engine.prompt("clean up the old files").await;

    engine.next_approval().await.expect("first delete must ask");
    assert!(all_exist(&engine.work, &names));
    engine.answer("allowed-once").await;

    engine
        .next_approval()
        .await
        .expect("the same command must ask again");
    assert!(
        !names.iter().any(|name| engine.work.join(name).exists()),
        "the approved delete did not run"
    );
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    engine.finish().await;
}

/// `>` into an existing file replaces it: asked; the same redirect into a new file is not.
#[tokio::test]
async fn overwriting_an_existing_file_asks_and_a_rejection_keeps_its_content() {
    let root = sandboxed_root();
    let mut engine = start_engine(
        root.path(),
        vec![
            bash(1, "echo replaced > keep.txt"),
            bash(2, "echo fresh > new.txt"),
            say("done"),
        ],
    )
    .await;
    write_files(&engine.work, &["keep.txt"]);
    engine.prompt("write the notes").await;

    let asked = engine.next_approval().await.expect("an overwrite must ask");
    assert!(
        asked.human_text.starts_with("Перезаписать файл keep.txt"),
        "{}",
        asked.human_text
    );
    engine.answer("rejected").await;
    assert!(
        engine.next_approval().await.is_none(),
        "a new file needs no question"
    );
    assert_eq!(
        std::fs::read_to_string(engine.work.join("keep.txt")).unwrap(),
        "old content"
    );
    assert_eq!(
        std::fs::read_to_string(engine.work.join("new.txt"))
            .unwrap()
            .trim(),
        "fresh"
    );
    engine.finish().await;
}

/// Ordinary work in the folder never reaches the host: twelve commands, no `approval/request`.
#[tokio::test]
async fn ordinary_commands_run_without_a_single_question() {
    let root = sandboxed_root();
    let commands = [
        "ls",
        "cat keep.txt",
        "grep -n old keep.txt",
        "mkdir made",
        "echo hello > fresh.txt",
        "echo year $(date +%Y) > stamp.txt",
        "cat keep.txt | wc -l",
        "cp keep.txt copy.txt",
        "wc -l keep.txt",
        "echo done",
        "ls | head -3",
        "echo more >> keep.txt",
    ];
    let mut responses: Vec<_> = commands
        .iter()
        .enumerate()
        .map(|(n, command)| bash(n, command))
        .collect();
    responses.push(say("all done"));
    let mut engine = start_engine(root.path(), responses).await;
    write_files(&engine.work, &["keep.txt"]);
    engine.prompt("do the usual work").await;

    assert!(
        engine.next_approval().await.is_none(),
        "an ordinary command was put to the user"
    );
    assert!(
        !engine
            .seen
            .iter()
            .any(|frame| frame["method"] == "approval/request"),
        "an ordinary command produced approval/request"
    );
    for made in ["made", "fresh.txt", "stamp.txt", "copy.txt"] {
        assert!(engine.work.join(made).exists(), "{made} was not created");
    }
    assert!(
        std::fs::read_to_string(engine.work.join("keep.txt"))
            .unwrap()
            .ends_with("more\n")
    );
    engine.finish().await;
}

fn escalated_bash(n: usize, command: &str, mode: &str) -> hoh_llm_openai_mock::ScenarioResponse {
    let mut call = bash(n, command);
    call.tool_calls[0].arguments["sandbox_permissions"] = json!(mode);
    call.tool_calls[0].arguments["justification"] = json!("the user asked for it");
    call
}

/// A call that escalates to `danger-full-access` is one question, worded by the guard in Russian
/// like every other (what it does, why it hurts; the command only in the details), not a second
/// English one from the bash tool.
#[tokio::test]
async fn a_full_access_escalation_is_one_russian_question() {
    let root = sandboxed_root();
    let mut engine = start_engine(
        root.path(),
        vec![
            escalated_bash(1, "rm old.txt", "danger-full-access"),
            escalated_bash(2, "ls /", "danger-full-access"),
            say("done"),
        ],
    )
    .await;
    let old = engine.work.join("old.txt");
    write_files(&engine.work, &["old.txt"]);
    engine.prompt("clean up").await;

    let asked = engine
        .next_approval()
        .await
        .expect("the escalation must ask");
    assert!(
        asked.human_text.starts_with(
            "Помощник просит выполнить команду без ограничений песочницы: удалить 1 файл: old.txt"
        ),
        "{}",
        asked.human_text
    );
    assert!(asked.human_text.contains("Файлы не попадут в корзину"));
    assert!(
        !asked.human_text.contains("rm old.txt"),
        "the command is for the details"
    );
    let details = asked.reason.as_deref().unwrap_or_default();
    assert!(
        details.contains("Команда: rm old.txt") && details.contains("the user asked for it"),
        "{details}"
    );
    assert!(old.exists());
    engine.answer("allowed-once").await;

    // The second call has nothing to delete, but runs unconfined: still one question.
    let asked = engine
        .next_approval()
        .await
        .expect("the unconfined call must ask");
    assert!(
        asked
            .human_text
            .starts_with("Помощник просит выполнить команду без ограничений песочницы: она сможет"),
        "{}",
        asked.human_text
    );
    engine.answer("rejected").await;
    assert!(
        engine.next_approval().await.is_none(),
        "the bash tool asked a second question for the same call"
    );
    assert!(!old.exists(), "the allowed command did not run");
    engine.finish().await;
}

/// A rejected escalation runs nothing and does not leave a question behind.
#[tokio::test]
async fn a_rejected_full_access_escalation_runs_nothing_and_asks_once() {
    let root = sandboxed_root();
    let mut engine = start_engine(
        root.path(),
        vec![
            escalated_bash(1, "rm old.txt", "danger-full-access"),
            say("done"),
        ],
    )
    .await;
    let old = engine.work.join("old.txt");
    write_files(&engine.work, &["old.txt"]);
    engine.prompt("clean up").await;
    engine
        .next_approval()
        .await
        .expect("the escalation must ask");
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert!(old.exists());
    engine.finish().await;
}

/// Asking for the mode the sandbox already runs in is not an escalation the tool asks about, so it
/// must not switch the guard off.
#[tokio::test]
async fn naming_the_standing_mode_does_not_skip_the_guard() {
    let root = sandboxed_root();
    let mut engine = start_engine(
        root.path(),
        vec![
            escalated_bash(1, "rm old.txt", "workspace-write"),
            say("done"),
        ],
    )
    .await;
    let old = engine.work.join("old.txt");
    write_files(&engine.work, &["old.txt"]);
    engine.prompt("clean up").await;
    let asked = engine.next_approval().await.expect("the guard must ask");
    assert!(
        asked.human_text.contains("удалить 1 файл: old.txt"),
        "{}",
        asked.human_text
    );
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert!(old.exists());
    engine.finish().await;
}

/// In-place edits, `rsync --delete`, unpacking and code in the command line are asked; running a
/// script file or listing/creating an archive is not (the sandbox is the second line there).
#[tokio::test]
async fn in_place_edits_archives_and_inline_code_ask_but_scripts_do_not() {
    let root = sandboxed_root();
    let mut engine = start_engine(
        root.path(),
        vec![
            bash(1, "sed -i s/old/new/ keep.txt"),
            bash(2, "python3 -c 'print(1)'"),
            bash(3, "tar xzf backup.tgz"),
            bash(4, "rsync -a --delete src/ dst/"),
            bash(5, "python3 run.py"),
            bash(6, "tar czf fresh.tgz keep.txt"),
            say("done"),
        ],
    )
    .await;
    write_files(&engine.work, &["keep.txt"]);
    std::fs::write(engine.work.join("run.py"), "print('ok')\n").unwrap();
    engine.prompt("tidy up").await;
    for expected in [
        "Перезаписать файл keep.txt",
        "встроенный код",
        "Распаковать архив backup.tgz",
        "rsync --delete",
    ] {
        let asked = engine
            .next_approval()
            .await
            .unwrap_or_else(|| panic!("no question for: {expected}"));
        assert!(
            asked.human_text.contains(expected),
            "{expected} not in {}",
            asked.human_text
        );
        engine.answer("rejected").await;
    }
    assert!(
        engine.next_approval().await.is_none(),
        "running a script or creating a new archive was put to the user"
    );
    assert_eq!(
        std::fs::read_to_string(engine.work.join("keep.txt")).unwrap(),
        "old content"
    );
    engine.finish().await;
}

/// Downloads over an existing file, unpacking, patches and `awk -i inplace` are asked; listing an
/// archive and downloading into a new file are not.
#[tokio::test]
async fn downloads_unpacking_and_patches_ask_but_listing_and_new_files_do_not() {
    let root = sandboxed_root();
    let mut engine = start_engine(
        root.path(),
        vec![
            bash(1, "curl -o keep.txt http://127.0.0.1:9/x"),
            bash(2, "unzip fixture.zip"),
            bash(3, "patch -p1 < fix.diff"),
            bash(4, "git apply fix.patch"),
            bash(5, "awk -i inplace '{print}' keep.txt"),
            bash(6, "unzip -l fixture.zip"),
            bash(7, "curl -o fresh.txt http://127.0.0.1:9/x"),
            say("done"),
        ],
    )
    .await;
    write_files(&engine.work, &["keep.txt"]);
    engine.prompt("fetch and apply").await;
    for expected in [
        "Перезаписать файл keep.txt",
        "Распаковать архив fixture.zip",
        "Применить патч и перезаписать файлы",
        "Применить патч и перезаписать файлы",
        "Перезаписать файл keep.txt",
    ] {
        let asked = engine
            .next_approval()
            .await
            .unwrap_or_else(|| panic!("no question for: {expected}"));
        assert!(
            asked.human_text.starts_with(expected),
            "{expected} / {}",
            asked.human_text
        );
        engine.answer("rejected").await;
    }
    assert!(
        engine.next_approval().await.is_none(),
        "listing an archive or downloading into a new file was put to the user"
    );
    assert_eq!(
        std::fs::read_to_string(engine.work.join("keep.txt")).unwrap(),
        "old content"
    );
    engine.finish().await;
}

/// A write that escalates to `danger-full-access`, under a call id the scenario chooses (a call id
/// may be reused between steps).
fn escalated_write(call_id: &str, target: &Path) -> hoh_llm_openai_mock::ScenarioResponse {
    hoh_llm_openai_mock::ScenarioResponse {
        text: vec![],
        tool_calls: vec![hoh_llm_openai_mock::ScenarioToolCall {
            id: call_id.to_owned(),
            name: "write".to_owned(),
            arguments: json!({
                "file_path": target,
                "content": "written outside",
                "sandbox_permissions": "danger-full-access",
                "justification": "the user asked for it",
            }),
        }],
        usage: hoh_llm_openai_mock::ScenarioUsage {
            prompt_tokens: 3,
            completion_tokens: 2,
        },
    }
}

/// The question the user rejected makes no grant: a `write` that reuses the call id and escalates
/// goes to the host with its own question, and a rejection there writes nothing.
#[tokio::test]
async fn a_rejected_bash_escalation_leaves_nothing_for_a_write_with_the_same_call_id() {
    let root = sandboxed_root();
    let target = root.path().join("outside/new.txt");
    let mut engine = start_engine(
        root.path(),
        vec![
            escalated_bash(1, "rm old.txt", "danger-full-access"),
            escalated_write("call-bash-1", &target),
            say("done"),
        ],
    )
    .await;
    write_files(&engine.work, &["old.txt"]);
    engine.prompt("clean up").await;

    let first = engine
        .next_approval()
        .await
        .expect("the bash escalation must ask");
    assert_eq!(first.tool_name, "bash");
    engine.answer("rejected").await;

    let second = engine
        .next_approval()
        .await
        .expect("the write took a grant that was never given: it did not ask the host");
    assert_eq!(second.tool_name, "write");
    assert!(!target.exists());
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert!(!target.exists(), "a write the host rejected landed");
    assert!(engine.work.join("old.txt").exists());
    engine.finish().await;
}

/// An allowed bash question is spent by the bash escalation of that call only: the next step's
/// `write` under the same id still asks.
#[tokio::test]
async fn an_allowed_bash_escalation_is_not_spent_by_a_write_with_the_same_call_id() {
    let root = sandboxed_root();
    let target = root.path().join("outside/new.txt");
    let mut engine = start_engine(
        root.path(),
        vec![
            escalated_bash(1, "rm old.txt", "danger-full-access"),
            escalated_write("call-bash-1", &target),
            say("done"),
        ],
    )
    .await;
    let old = engine.work.join("old.txt");
    write_files(&engine.work, &["old.txt"]);
    engine.prompt("clean up").await;

    let first = engine
        .next_approval()
        .await
        .expect("the bash escalation must ask");
    assert_eq!(first.tool_name, "bash");
    engine.answer("allowed-once").await;

    let second = engine
        .next_approval()
        .await
        .expect("the write was waved through on the bash grant");
    assert_eq!(second.tool_name, "write");
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert!(!old.exists(), "the allowed bash command did not run");
    assert!(!target.exists());
    engine.finish().await;
}
