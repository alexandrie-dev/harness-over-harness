use crate::{
    Result,
    compare::{self, Check, Failure},
    corpus::{self, Scenario},
    normalize::{Context, Normalizer},
    protocol::Rpc,
    read, records, workspace,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[cfg(unix)]
struct ProcessGroup(Option<rustix::process::Pid>);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

#[derive(Clone)]
pub struct Options {
    pub hoh: PathBuf,
    pub refs: PathBuf,
    pub runs: PathBuf,
    pub results: PathBuf,
    pub suites: Vec<String>,
    pub scenarios: Vec<String>,
    pub jobs: usize,
    pub timeout: Duration,
    pub request_timeout: Duration,
    pub wait_timeout: Duration,
    /// `--disable-plugin ID` (repeatable): diagnostic rows disabled on top of the normalization overlay.
    pub disable: Vec<String>,
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut hoh = None;
        let mut refs = None;
        let mut runs = None;
        let mut results = PathBuf::from("conformance/RESULTS.tsv");
        let mut suites = Vec::new();
        let mut scenarios = Vec::new();
        let mut jobs = 1;
        let mut timeout = 30_000;
        let mut request_timeout = 110_000;
        let mut wait_timeout = 10_000;
        let mut disable = Vec::new();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--hoh" => hoh = Some(PathBuf::from(value)),
                "--refs" => refs = Some(PathBuf::from(value)),
                "--runs" => runs = Some(PathBuf::from(value)),
                "--results" => results = PathBuf::from(value),
                "--suite" => {
                    if !["session", "sdk", "acp"].contains(&value.as_str()) {
                        return Err(format!("invalid suite {value}"));
                    }
                    suites.push(value);
                }
                "--scenario" => scenarios.push(value),
                "--disable-plugin" => disable.push(value),
                "--jobs" => jobs = value.parse().map_err(|_| "invalid jobs")?,
                "--timeout-ms" => timeout = value.parse().map_err(|_| "invalid timeout")?,
                "--request-timeout-ms" => {
                    request_timeout = value.parse().map_err(|_| "invalid request timeout")?
                }
                "--wait-timeout-ms" => {
                    wait_timeout = value.parse().map_err(|_| "invalid wait timeout")?
                }
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        if jobs == 0 || timeout == 0 || request_timeout == 0 || wait_timeout == 0 {
            return Err("jobs and timeouts must be positive".into());
        }
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let absolute = |p: PathBuf| if p.is_absolute() { p } else { cwd.join(p) };
        Ok(Self {
            hoh: absolute(hoh.ok_or("--hoh is required")?),
            refs: fs::canonicalize(refs.ok_or("--refs is required")?).map_err(|e| e.to_string())?,
            runs: absolute(runs.ok_or("--runs is required")?),
            results: absolute(results),
            suites,
            scenarios,
            jobs,
            timeout: Duration::from_millis(timeout),
            request_timeout: Duration::from_millis(request_timeout),
            wait_timeout: Duration::from_millis(wait_timeout),
            disable,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub suite: String,
    pub scenario: String,
    pub failure: Option<Failure>,
    pub runner_error: Option<String>,
}
impl Outcome {
    fn failed(s: &Scenario, failure: Failure) -> Self {
        Self {
            suite: s.suite.clone(),
            scenario: s.name.clone(),
            failure: Some(failure),
            runner_error: None,
        }
    }
    fn passed(s: &Scenario) -> Self {
        Self {
            suite: s.suite.clone(),
            scenario: s.name.clone(),
            failure: None,
            runner_error: None,
        }
    }
}

pub fn harvest(root: &Path, suite: &str) -> Result<Vec<String>> {
    fn files(root: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
        if !root.exists() {
            return Ok(());
        }
        for e in corpus::sorted_entries(root)? {
            if e.file_type().map_err(|e| e.to_string())?.is_dir() {
                files(&e.path(), result)?;
            } else {
                result.push(e.path());
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    files(root, &mut paths)?;
    let mut selected: BTreeMap<PathBuf, (u64, PathBuf)> = BTreeMap::new();
    for path in paths {
        let Some(version) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(hoh_session_format::parse_session_format_log_filename)
        else {
            continue;
        };
        let parent = path
            .parent()
            .ok_or("session path has no parent")?
            .to_owned();
        if selected.get(&parent).is_none_or(|(v, _)| version > *v) {
            selected.insert(parent, (version, path));
        }
    }
    let mut logs = Vec::new();
    for (_, (_, path)) in selected {
        let text = read(&path)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        crate::session_files::assert_persisted_session_version(name, &text)?;
        let rows = records(&text)?;
        let header = rows.first().ok_or("empty persisted session")?;
        logs.push((header.clone(), text));
    }
    let catalog: BTreeMap<String, BTreeMap<String, usize>> = logs
        .iter()
        .filter_map(|(h, text)| {
            let entries = records(text)
                .ok()?
                .into_iter()
                .filter(|r| r["type"] == "subagent/catalog")
                .enumerate()
                .filter_map(|(i, r)| Some((r["data"]["childId"].as_str()?.to_owned(), i)))
                .collect();
            Some((h["id"].as_str()?.to_owned(), entries))
        })
        .collect();
    logs.sort_by(|(a, _), (b, _)| {
        let order = a["parentSession"]
            .is_string()
            .cmp(&b["parentSession"].is_string());
        if !order.is_eq() {
            return order;
        }
        if suite == "session"
            && a["parentSession"] == b["parentSession"]
            && let Some(c) = a["parentSession"].as_str().and_then(|p| catalog.get(p))
        {
            let rank = |h: &Value| {
                h["id"]
                    .as_str()
                    .and_then(|id| c.get(id))
                    .copied()
                    .unwrap_or(usize::MAX)
            };
            let order = rank(a).cmp(&rank(b));
            if !order.is_eq() {
                return order;
            }
        }
        let order = a["createdAt"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&b["createdAt"].as_i64().unwrap_or(0));
        // harness.ts:843: `a.id.localeCompare(b.id)` breaks a same-time tie in every suite.
        order.then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    Ok(logs.into_iter().map(|(_, text)| text).collect())
}

fn write(path: &Path, text: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}
fn create(path: &Path) -> Result<fs::File> {
    fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Records comparisons actually executed, independently of a later unavailable capability gate.
fn checked(dir: &Path, name: &str, result: Check) -> Result<Check> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("checks.jsonl"))
        .map_err(|e| e.to_string())?;
    let entry = json!({"check":name,"status":if result.is_ok(){"pass"}else{"fail"}});
    writeln!(file, "{}", hoh_util_values::js_json_stringify(&entry)).map_err(|e| e.to_string())?;
    Ok(result)
}

/// The scenario-owned spill locator root; the Windows spelling is exactly two bytes shorter.
pub fn spill_locator_root(name: &str, windows: bool) -> String {
    let hash = Sha256::digest(name.as_bytes());
    let hex = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!(
        "{}/dsh-acp-snap-{}",
        if windows { "/t" } else { "/tmp" },
        &hex[..9]
    )
}

pub fn environment(
    s: &Scenario,
    fixture_files: &[PathBuf],
    cwd: &Path,
    spill: &Path,
    sessions: &Path,
    route: &Value,
) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    env.insert("DEEPSEEK_API_KEY".into(), "snapshot-replay-key".into());
    env.insert("DSH_SNAPSHOT".into(), "replay".into());
    env.insert(
        "DSH_HOME".into(),
        cwd.join(".dsh").to_string_lossy().into_owned(),
    );
    env.insert(
        "DSH_AGENTS_HOME".into(),
        cwd.join(".agents").to_string_lossy().into_owned(),
    );
    if let Some(file) = fixture_files.first() {
        env.insert(
            "DSH_SNAPSHOT_FILE".into(),
            file.to_string_lossy().into_owned(),
        );
    }
    if fixture_files.len() > 1 {
        // A path holding `:` cannot be listed in the variable; dropping the child fixtures silently
        // would replay a different scenario.
        let joined = std::env::join_paths(&fixture_files[1..])
            .map_err(|e| format!("DSH_SNAPSHOT_CHILD_FILES: {e}"))?;
        env.insert(
            "DSH_SNAPSHOT_CHILD_FILES".into(),
            joined.to_string_lossy().into_owned(),
        );
    }
    if s.manifest["replay"]["override"] == true {
        env.insert(
            "DSH_SNAPSHOT_OVERRIDE".into(),
            s.dir
                .join("replay.override.json")
                .to_string_lossy()
                .into_owned(),
        );
    }
    for (key, field) in [
        ("DSH_SNAPSHOT_PROVIDER", "provider"),
        ("DSH_SNAPSHOT_MODEL", "model"),
    ] {
        if let Some(value) = route[field].as_str() {
            env.insert(key.into(), value.into());
        }
    }
    if s.suite != "sdk" {
        env.insert(
            "DSH_SNAPSHOT_SPILL_ROOT".into(),
            spill.to_string_lossy().into_owned(),
        );
        env.insert(
            "DSH_SNAPSHOT_SPILL_LOCATOR_ROOT".into(),
            spill_locator_root(&s.name, cfg!(windows)),
        );
    }
    if s.suite == "acp" {
        env.insert(
            "DSH_SNAPSHOT_SESSIONS_ROOT".into(),
            sessions.to_string_lossy().into_owned(),
        );
    } else {
        env.insert("DSH_TELEMETRY_DISABLED".into(), "1".into());
        env.insert(
            "NODE_OPTIONS".into(),
            [
                std::env::var("NODE_OPTIONS").unwrap_or_default(),
                "--disable-warning=ExperimentalWarning".into(),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        );
    }
    if let Some(permission) = s.manifest["permission"].as_str() {
        env.insert("DSH_PERMISSION_MODE".into(), permission.into());
    }
    if let Some(overrides) = s.manifest["environment"].as_object() {
        for (k, v) in overrides {
            if let Some(value) = v.as_str() {
                env.insert(k.clone(), value.into());
            }
        }
    }
    if s.suite == "sdk" {
        let permission = match s.name.as_str() {
            "approval" | "plan-review" | "goal-rounds" | "command-attachments" => {
                Some("workspace-write")
            }
            "cua-run-code" | "permission-switch" | "command-identity" => Some("danger-full-access"),
            _ => None,
        };
        if let Some(value) = permission {
            env.insert("DSH_PERMISSION_MODE".into(), value.into());
        }
        if s.name == "subagent-continuable" {
            env.insert("DSH_SNAPSHOT_HUMAN_STEER".into(), "1".into());
        }
        if s.name == "persistent-tools" {
            env.insert(
                "DSH_SYSTEM_PROMPT".into(),
                "You are the environment-selected minimal software engineer.".into(),
            );
        }
        if s.name == "subagent-dsh-sdk-diagnostic" {
            env.insert(
                "DSH_TEST_CHILD_PATCH".into(),
                s.dir
                    .join("child.cordis.yml")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if s.name == "subagent-dsh-sdk-dynamic-route" {
            env.insert(
                "DSH_TEST_PARENT_PROVIDER".into(),
                "deepseek-official".into(),
            );
        }
    }
    Ok(env)
}

fn scenario_checks(s: &Scenario, body: &crate::body::BodyContext<'_>) -> Check {
    // A ported scenario body (f1's `body` module) decides; `None` means this module carries none.
    if let Some(result) = crate::body::body_check(&s.suite, &s.name, body) {
        return crate::body::gate(&s.suite, &s.name, result);
    }
    // This is a deliberate fail-closed gate. The source's scenario bodies are not implied by snapshots.
    if s.suite == "sdk" {
        return Err(Failure::new(
            "scenario-check",
            format!("check sdk.{} not implemented", s.name),
        ));
    }
    if s.suite == "acp" {
        return Ok(());
    }
    let check = match s.name.as_str() {
        "provider-cwd" => Some("verifyProviderCwdResume"),
        "worktree" => Some("verifyWorktree"),
        "working-directory" => Some("verifyWorkingDirectory"),
        "session-query-spill" => Some("verifySessionQuerySpill"),
        "session-reference-spill" => Some("sessionReferenceSpillExpected"),
        "team-tree-messages" => Some("verifyTeamTreeMessage"),
        "team-task-inbox" => Some("verifyTeamTaskInbox"),
        "advanced-toolchain" => Some("verifyAdvancedToolchain"),
        "mcp-resources" | "mcp-resources-ptc" => Some("verifyMcpResources"),
        "mcp-template-error" => Some("verifyMcpTemplateError"),
        "mcp-empty" | "mcp-empty-ptc" => Some("verifyNoMcpServers"),
        "background-confinement-failure" => Some("verifyBackgroundConfinementFailure"),
        "plugin-manager-mcp" => Some("pluginManagerMcp"),
        "tool-call-turn" => Some("verifyToolResultWriterParity"),
        "packed-chunks" => Some("packedWriterParity"),
        "toolcall-unused-args" => Some("toolcallUnusedArgs"),
        "empty-response-retry-current" => Some("emptyResponseRetryCurrent"),
        "office-progressive-native" | "office-progressive-ptc" => Some("officeProgressive"),
        "subagent-depth-two-rejection" => Some("subagentDepthTwoRejection"),
        _ => None,
    };
    if let Some(check) = check {
        return Err(Failure::new(
            "scenario-check",
            format!("check {check} not implemented"),
        ));
    }
    // `strictV4Catalog` is performed in `execute` for every replayed fixture (`compare::strict_catalog`);
    // reaching this point means it passed. Scenarios without a named body check end here.
    Ok(())
}

async fn execute(options: &Options, all: &[Scenario], s: &Scenario, dir: &Path) -> Result<Check> {
    let fixtures = match s.fixtures() {
        Ok(f) => f,
        Err(e) => return Ok(Err(Failure::new("input", e))),
    };
    let raw = fixtures
        .iter()
        .map(|p| read(p))
        .collect::<Result<Vec<_>>>()?;
    let rows = records(&raw[0])?;
    let mut pending = None;
    let fixed = s.name == "compaction-recovery"
        || s.suite == "sdk"
            && ["minimal-image-compaction", "manual-compact"].contains(&s.name.as_str());
    let prefix = if fixed {
        let base = dir.join("cwd-XXXXXX").to_string_lossy().into_owned();
        let length = hoh_util_values::js_json_stringify(&json!(base))
            .encode_utf16()
            .count();
        if length > 192 {
            return Ok(Err(Failure::new(
                "setup",
                "temporary root exceeds fixed cwd text length",
            )));
        }
        format!("cwd-{}", "-".repeat(192 - length))
    } else {
        "cwd-".into()
    };
    let cwd_owner = tempfile::Builder::new()
        .prefix(&prefix)
        .rand_bytes(6)
        .tempdir_in(dir)
        .map_err(|e| e.to_string())?;
    // Persist run data for diagnosis; each invocation gets a distinct directory.
    let cwd = cwd_owner.keep();
    let cwd = fs::canonicalize(cwd).map_err(|e| e.to_string())?;
    if fixed
        && hoh_util_values::js_json_stringify(&json!(cwd.to_string_lossy()))
            .encode_utf16()
            .count()
            != 192
    {
        return Ok(Err(Failure::new("setup", "fixed cwd length mismatch")));
    }
    if s.manifest["workspace"]["parent"] == "outside-temp" {
        let temp = fs::canonicalize(std::env::temp_dir()).map_err(|e| e.to_string())?;
        if cwd.starts_with(&temp) || cwd.starts_with("/tmp") {
            pending = Some(Failure::new(
                "setup",
                "outside-temp workspace falls within temporary write grant",
            ));
        }
    }
    let spill_owner = tempfile::Builder::new()
        .prefix("spill-")
        .tempdir_in(dir)
        .map_err(|e| e.to_string())?;
    let spill = spill_owner.keep();
    let sessions = if s.suite == "acp" {
        dir.join(cwd.file_name().ok_or("cwd has no name")?)
            .join(".dsh/acp-sessions")
    } else {
        cwd.join(".dsh/sessions")
    };
    let setup = workspace::prepare(s, &cwd, &options.refs);
    match setup {
        Ok(Some(check)) => pending = Some(Failure::new("setup", check)),
        Err(e) => pending = Some(Failure::new("setup", e)),
        _ => {}
    }
    let initial = workspace::capture(&cwd, &workspace::ignored(s), false)?;
    let patch_root = cwd.join(if s.suite == "acp" {
        ".dsh-profile-patches"
    } else {
        ".snapshot-patches"
    });
    fs::create_dir_all(&patch_root).map_err(|e| e.to_string())?;
    let sources = corpus::patch_sources(all, s, &options.refs)?;
    let mut patches = Vec::new();
    for (i, path) in sources.iter().enumerate() {
        let target = patch_root.join(format!(
            "{i}-{}",
            path.file_name()
                .ok_or("patch has no basename")?
                .to_string_lossy()
        ));
        write(
            &target,
            crate::yaml::materialize(&read(path)?, path, &options.refs)?,
        )?;
        patches.push(target);
    }
    // N-BOOT-FOREIGN-PLUGINS: the last overlay disables the rows outside the 118 (NORMALIZATION.md).
    let exclusions = patch_root.join(format!("{}-v1-exclusions.cordis.yml", patches.len()));
    write(
        &exclusions,
        crate::normalize::v1::exclusion_patch_with(&options.disable),
    )?;
    patches.push(exclusions);
    if s.name == "plugin-manager-mcp" {
        let target = cwd.join(".dsh/profiles/headless/cordis.patch.yml");
        fs::create_dir_all(target.parent().ok_or("profile path has no parent")?)
            .map_err(|e| e.to_string())?;
        write(
            &target,
            crate::yaml::materialize(
                &read(&s.dir.join("profile.patch.yml"))?,
                &s.dir.join("profile.patch.yml"),
                &options.refs,
            )?,
        )?;
    }
    let mut replay_files = fixtures.clone();
    if s.suite == "sdk" {
        let root = cwd.join(".replay-fixtures");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        replay_files.clear();
        for (i, file) in fixtures.iter().enumerate() {
            let target = root.join(file.file_name().ok_or("fixture has no name")?);
            write(&target, raw[i].replace("{{cwd}}", &cwd.to_string_lossy()))?;
            replay_files.push(target);
        }
    }
    let mut route = rows
        .iter()
        .find(|r| r["type"] == "request/header")
        .map(|r| r["data"]["header"]["config"].clone())
        .unwrap_or(Value::Null);
    if route.is_null()
        && let Ok(pin) = corpus::pin(all, s)
    {
        let pin_files = pin.fixtures()?;
        let pin_rows = records(&read(&pin_files[0])?)?;
        route = pin_rows
            .iter()
            .find(|r| r["type"] == "request/header")
            .map(|r| r["data"]["header"]["config"].clone())
            .unwrap_or(Value::Null);
    }
    let mut env = environment(s, &replay_files, &cwd, &spill, &sessions, &route)?;
    if s.name == "subagent-dsh-sdk-dynamic-route" {
        let child_home = cwd.join(".child-dsh");
        fs::create_dir_all(&child_home).map_err(|e| e.to_string())?;
        let mut child_patches = Vec::new();
        for (i, source) in [
            options.refs.join("snapshots/sdk/legacy-context.patch.yml"),
            options
                .refs
                .join("packages/subagent/subagent-dsh-sdk/tests/fixtures/loader/child.patch.yml"),
        ]
        .iter()
        .enumerate()
        {
            let target = patch_root.join(format!("child-{i}.yml"));
            write(
                &target,
                crate::yaml::materialize(&read(source)?, source, &options.refs)?,
            )?;
            child_patches.push(target.to_string_lossy().into_owned());
        }
        env.insert(
            "DSH_TEST_CHILD_PATCHES".into(),
            hoh_util_values::js_json_stringify(&json!(child_patches)),
        );
        env.insert(
            "DSH_TEST_CHILD_HOME".into(),
            child_home.to_string_lossy().into_owned(),
        );
    }
    if ["mcp-resources", "mcp-resources-ptc"].contains(&s.name.as_str()) {
        env.insert(
            "DSH_MCP_RESOURCES_FIXTURE".into(),
            options
                .refs
                .join("packages/mcp/mcp-client/tests/fixtures/resources-server.ts")
                .to_string_lossy()
                .into_owned(),
        );
    }
    let mut command = tokio::process::Command::new(&options.hoh);
    #[cfg(unix)]
    command.process_group(0);
    command
        .current_dir(&cwd)
        .kill_on_drop(true)
        .arg("--profile")
        .arg(s.profile());
    for path in &patches {
        command.arg("--patch").arg(path);
    }
    if s.suite == "session" {
        if let Some(task) =
            compare::task(&rows).or_else(|| s.manifest["input"]["task"].as_str().map(str::to_owned))
        {
            command.arg(task);
        } else {
            return Ok(Err(Failure::new(
                "input",
                "no accepted or exceptional task input",
            )));
        }
    }
    // clearedProxyEnv(), including lower-case spellings and ALL/NO proxy.
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        command.env_remove(key);
    }
    command.envs(&env);
    command.stderr(Stdio::from(create(&dir.join("stderr.txt"))?));
    if s.suite == "session" {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(create(&dir.join("stdout.txt"))?));
    } else {
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
    }
    let mut description = json!({"executable":options.hoh.to_string_lossy(),"cwd":cwd.to_string_lossy(),"profile":s.profile(),"patches":patches.iter().map(|p|p.to_string_lossy()).collect::<Vec<_>>(),"environment":env});
    description["task"] = compare::task(&rows).map(Value::from).unwrap_or(Value::Null);
    write(
        &dir.join("launch.json"),
        hoh_util_values::js_json_stringify_pretty(&description, 2),
    )?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Ok(Err(Failure::new("launch", e.to_string()))),
    };
    #[cfg(unix)]
    let _process_group = ProcessGroup(
        child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw),
    );
    let mut rpc_error = None;
    let mut initialized = false;
    let mut notifications = Vec::new();
    let mut raw_stdout = String::new();
    let mut sdk_results = Vec::new();
    if s.suite != "session" {
        let script = if s.suite == "acp" {
            hoh_util_values::js_json_parse(&read(&s.dir.join("input.json"))?)
                .map_err(|e| e.to_string())?
        } else {
            json!({})
        };
        let request_timeout = if s.suite == "sdk" {
            options.request_timeout
        } else {
            options.wait_timeout
        };
        let mut rpc = Rpc::new(&mut child, s, &script, request_timeout)?;
        let controller = async {
            if s.suite == "sdk" {
                crate::protocol::sdk(&mut rpc, s, &rows, &cwd).await
            } else {
                crate::protocol::acp(&mut rpc, &script, &cwd, &sessions, options.wait_timeout).await
            }
        };
        // SDK: every request carries its own timeout (`Rpc::finish`/`until`), so the controller as a
        // whole is unbounded; ACP keeps the process-wide limit.
        let controlled = if s.suite == "sdk" {
            controller.await
        } else {
            tokio::time::timeout(options.timeout, controller)
                .await
                .unwrap_or_else(|_| Err("controller timeout".into()))
        };
        if let Err(e) = controlled {
            rpc_error = Some(e);
        }
        initialized = rpc.initialized;
        if rpc_error.is_none() {
            if let Err(e) = tokio::time::timeout(options.timeout, rpc.close())
                .await
                .unwrap_or(Err("protocol close timeout".into()))
            {
                rpc_error = Some(e);
            }
        } else {
            let _ = child.start_kill();
        }
        raw_stdout = rpc.raw;
        notifications = rpc.notifications;
        sdk_results = rpc.results;
        write(&dir.join("stdout.txt"), &raw_stdout)?;
    }
    let status = match tokio::time::timeout(options.timeout, child.wait()).await {
        Ok(result) => result.map_err(|e| e.to_string())?,
        Err(_) => {
            let _ = child.kill().await;
            return Ok(Err(Failure::new("exit", "process timeout")));
        }
    };
    write(&dir.join("exit.txt"), format!("{:?}\n", status.code()))?;
    let mut actual = match harvest(&sessions, &s.suite) {
        Ok(logs) => logs,
        Err(e) => return Ok(Err(Failure::new("log", e))),
    };
    if s.name == "subagent-dsh-sdk-dynamic-route" {
        actual.extend(harvest(&cwd.join(".child-dsh/sessions"), "sdk")?);
    }
    // suite.ts: raw parent and child catalog clocks must agree before normalization can erase them.
    if let Err(e) = crate::refresh::reconcile_catalog_creation_times(
        &actual,
        crate::refresh::ClockPolicy::Validate,
    ) {
        return Ok(Err(Failure::new("log", e)));
    }
    let expected_code = compare::expected_exit(&rows);
    // Headless asserts its exit code. SDK client.ts:263–267,494–510 and dispose.ts:82–98,
    // like ACP launcher.ts:271–294,445–448, only await termination after protocol completion.
    if s.suite == "session"
        && (status.code() != Some(expected_code) || !status.success() && actual.is_empty())
    {
        return Ok(Err(Failure::new(
            "exit",
            format!(
                "exit {:?}; expected {expected_code}; persisted sessions {}",
                status.code(),
                actual.len()
            ),
        )));
    }
    if let Some(mut error) = rpc_error {
        // harness.ts: a connection that closes during startup reports what the agent wrote to stderr.
        if !initialized
            && let Ok(stderr) = fs::read_to_string(dir.join("stderr.txt"))
            && !stderr.trim().is_empty()
        {
            error = format!("{error}\nagent stderr:\n{}", stderr.trim_end());
        }
        return Ok(Err(Failure::new(
            if initialized { "frames" } else { "launch" },
            error,
        )));
    }
    let has_model = rows.iter().any(|r| r["type"] == "request/header");
    // A missing decoder is an input failure, even when the process emitted a comparison diff.
    // Preserve launch/exit precedence above; ACP without a model turn does not restore a log.
    for raw in raw.iter().filter(|_| s.suite != "acp" || has_model) {
        if let Err(f) = checked(dir, "strict-catalog", compare::strict_catalog(raw))? {
            return Ok(Err(f));
        }
    }
    let notifications_path = s.dir.join(if s.manifest.get("sessionFormat").is_some() {
        "notifications.current.expected.jsonl"
    } else {
        "notifications.expected.jsonl"
    });
    let has_wire_goldens =
        notifications_path.exists() || s.dir.join("result.expected.json").exists();
    if s.suite == "sdk" && has_wire_goldens {
        if let Err(f) = checked(
            dir,
            "wire-generation-actual",
            compare::sdk_wire_generation(&actual, "actual").map(|_| ()),
        )? {
            return Ok(Err(f));
        }
        if let Err(f) = checked(
            dir,
            "wire-generation-expected",
            compare::sdk_wire_generation(&s.expected_logs()?, "expected").map(|_| ()),
        )? {
            return Ok(Err(f));
        }
    }
    if s.suite != "acp"
        && let Err(f) = checked(
            dir,
            "writer-generation",
            compare::current_writer_generation(&actual),
        )?
    {
        return Ok(Err(f));
    }
    let n = Normalizer::new()?;
    let ctx = compare::context(&actual, &cwd.to_string_lossy())?;
    if s.suite != "session"
        && let Err(f) = compare::catalog_creation_times(&actual)
    {
        return Ok(Err(f));
    }
    if s.suite == "sdk"
        && actual
            .iter()
            .any(|log| log.contains("llm-replay: script exhausted"))
    {
        return Ok(Err(Failure::new("log", "replay script exhausted")));
    }
    if s.suite == "acp"
        && actual
            .iter()
            .map(|log| crate::storage::unknown_tool_call_ids(log))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .any(|ids| !ids.is_empty())
    {
        return Ok(Err(Failure::new(
            "log",
            "ACP snapshot contains UNKNOWN_TOOL",
        )));
    }
    if s.suite != "acp" || has_model {
        if let Err(f) = checked(
            dir,
            "log",
            compare::logs(&n, &s.expected_logs()?, &actual, &ctx),
        )? {
            return Ok(Err(f));
        }
        if let Err(f) = checked(dir, "headers", compare::headers(&n, all, s, &actual, &ctx))? {
            return Ok(Err(f));
        }
    }
    let mut final_tree = workspace::capture(&cwd, &workspace::ignored(s), false)?;
    if s.name == "worktree" {
        final_tree.retain(|e| e["path"] != ".agents/worktrees/isolated/.git");
    }
    let expected_tree = if s.manifest["workspace"]["final"] == true {
        workspace::capture(&s.dir.join("workspace.expected"), &[], true)?
    } else {
        initial
    };
    if let Err(f) = checked(
        dir,
        "tree",
        compare::equal(
            "tree",
            "working tree",
            &json!(expected_tree),
            &json!(final_tree),
        ),
    )? {
        return Ok(Err(f));
    }
    if s.suite == "session" {
        if let Err(f) = checked(
            dir,
            "stdout",
            compare::headless_stdout(
                &raw[0],
                &read(&dir.join("stdout.txt"))?,
                &read(&dir.join("stderr.txt"))?,
            ),
        )? {
            return Ok(Err(f));
        }
    } else if s.suite == "acp" {
        let expected = n.frames(
            &read(&s.dir.join("stdout.expected.jsonl"))?,
            &Context::default(),
        );
        let actual = n.frames(&raw_stdout, &ctx);
        match (expected, actual) {
            (Ok(e), Ok(a)) => {
                if let Err(f) = checked(
                    dir,
                    "ACP stdout",
                    compare::equal("frames", "ACP stdout", &json!(e), &json!(a)),
                )? {
                    return Ok(Err(f));
                }
            }
            (Err(e), _) | (_, Err(e)) => return Ok(Err(Failure::new("frames", e))),
        }
    } else {
        write(
            &dir.join("notifications.jsonl"),
            crate::jsonl(&notifications),
        )?;
        let expected_path = s.dir.join(if s.manifest.get("sessionFormat").is_some() {
            "notifications.current.expected.jsonl"
        } else {
            "notifications.expected.jsonl"
        });
        let result_path = s.dir.join("result.expected.json");
        if expected_path.exists() || result_path.exists() {
            let source_version = actual
                .first()
                .and_then(|raw| records(raw).ok())
                .and_then(|rows| rows.first().and_then(|h| h["version"].as_u64()));
            let actual_notifications = match n.notifications(&notifications, &ctx, source_version) {
                Ok(v) => v,
                Err(e) => return Ok(Err(Failure::new("frames", e))),
            };
            // Goldens have already passed normalizeNotifications in dsh. Only delivery metadata is generation-relative.
            let mut expected_notifications = records(&read(&expected_path)?)?;
            let expected_logs = s.expected_logs()?;
            let expected_version = records(&expected_logs[0])?
                .first()
                .and_then(|h| h["version"].as_u64());
            for notification in &mut expected_notifications {
                if notification["method"] == "session.event"
                    && notification["params"]["event"]["type"]
                        == "session-log-deepseek/delivery-accepted"
                    && expected_version.is_some()
                    && notification["params"]["event"]["data"]["sessionFormatVersion"].as_u64()
                        == expected_version
                {
                    notification["params"]["event"]["data"]["sessionFormatVersion"] =
                        json!("{{sourceSessionFormatVersion}}");
                }
            }
            if let Err(f) = checked(
                dir,
                "SDK notifications",
                compare::equal(
                    "frames",
                    "SDK notifications",
                    &json!(expected_notifications),
                    &json!(actual_notifications),
                ),
            )? {
                return Ok(Err(f));
            }
            let Some(last) = sdk_results.last() else {
                return Ok(Err(Failure::new(
                    "frames",
                    "SDK wire golden has no run result",
                )));
            };
            let result = crate::jsonl(&n.frames(&crate::jsonl(std::slice::from_ref(last)), &ctx)?);
            if let Err(f) = checked(
                dir,
                "SDK result",
                compare::equal(
                    "frames",
                    "SDK result",
                    &json!(read(&result_path)?),
                    &json!(result),
                ),
            )? {
                return Ok(Err(f));
            }
        }
    }
    // Missing setup/schema support cannot yield pass.
    if let Some(f) = pending {
        return Ok(Err(f));
    }
    let expected_logs = s.expected_logs()?;
    let harvest_again = || harvest(&sessions, &s.suite);
    let task = compare::task(&rows)
        .or_else(|| s.manifest["input"]["task"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let body = crate::body::BodyContext {
        dir: &s.dir,
        manifest: &s.manifest,
        actual: &actual,
        expected: &expected_logs,
        notifications: &notifications,
        cwd: &cwd,
        harvest: &harvest_again,
        launch: crate::body::Launch {
            hoh: &options.hoh,
            profile: s.profile(),
            patches: &patches,
            env: &env,
            task: &task,
            timeout: options.timeout,
        },
    };
    // The body checks are synchronous (files, Git, a second `hoh` launch).
    let verdict = off_the_workers(|| scenario_checks(s, &body))?;
    checked(dir, "scenario-check", verdict)
}

/// Run synchronous, possibly slow work (the body checks) without stalling the async workers that
/// drive the other scenarios and their timeouts. Only a multi-thread runtime can give the work up
/// its worker (`block_in_place`); on a current-thread runtime it would freeze every timer, so it is
/// refused instead of being run in place.
pub fn off_the_workers<T>(work: impl FnOnce() -> T) -> Result<T> {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current().map(|h| h.runtime_flavor()) {
        Ok(RuntimeFlavor::MultiThread) => Ok(tokio::task::block_in_place(work)),
        _ => Err(
            "scenario bodies need a multi-thread tokio runtime: on a current-thread runtime \
                  they would stall the timers of the other scenarios"
                .into(),
        ),
    }
}

pub async fn run(options: &Options, all: &[Scenario], s: &Scenario) -> Outcome {
    let dir = options.runs.join(&s.suite).join(&s.name);
    let perform = async {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // The oracle compares paths the child sees (cwd is canonical, the fixed-length cwd text and
        // the spill root's parent) with paths built here: a symlinked or non-normalised `--runs`
        // (a target dir that is a link into tmpfs, `..`) would make them differ for no reason.
        let dir = fs::canonicalize(&dir).map_err(|e| e.to_string())?;
        let attempt = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(&dir)
            .map_err(|e| e.to_string())?
            .keep();
        write(
            &dir.join("latest-run.txt"),
            format!("{}\n", attempt.display()),
        )?;
        match execute(options, all, s, &attempt).await? {
            Ok(()) => {
                write(&attempt.join("diff.txt"), "")?;
                write(&dir.join("diff.txt"), "")?;
                Ok(Outcome::passed(s))
            }
            Err(f) => {
                write(&attempt.join("diff.txt"), &f.diff)?;
                write(&dir.join("diff.txt"), &f.diff)?;
                Ok(Outcome::failed(s, f))
            }
        }
    };
    match perform.await {
        Ok(outcome) => outcome,
        Err(e) => Outcome {
            suite: s.suite.clone(),
            scenario: s.name.clone(),
            failure: None,
            runner_error: Some(e),
        },
    }
}

pub fn report(options: &Options, outcomes: &[Outcome]) -> Result<String> {
    let mut rows = String::new();
    let mut errors = String::new();
    let mut passed = 0;
    let mut failed = 0;
    let mut runner_errors = 0;
    for outcome in outcomes {
        let clean = |s: &str| s.lines().next().unwrap_or("").replace(['\t', '\r'], " ");
        if let Some(error) = &outcome.runner_error {
            runner_errors += 1;
            errors.push_str(&format!(
                "{}\t{}\t{}\n",
                outcome.suite,
                outcome.scenario,
                clean(error)
            ));
        } else if let Some(f) = &outcome.failure {
            failed += 1;
            rows.push_str(&format!(
                "{}\t{}\tfail\t{}\t{}\n",
                outcome.suite,
                outcome.scenario,
                f.phase,
                clean(&f.reason)
            ));
        } else {
            passed += 1;
            rows.push_str(&format!(
                "{}\t{}\tpass\tscenario-check\t\n",
                outcome.suite, outcome.scenario
            ));
        }
    }
    if let Some(parent) = options.results.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write(&options.results, rows)?;
    write(&options.results.with_extension("runner-errors.tsv"), errors)?;
    Ok(format!(
        "scenarios: {passed} of {} pass, {failed} fail, {runner_errors} runner errors",
        outcomes.len()
    ))
}
