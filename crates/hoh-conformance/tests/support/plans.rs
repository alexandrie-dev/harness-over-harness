//! Fixture oracle for the executable double. No runner launch/environment helpers are used here.
use hoh_conformance::{
    compare,
    corpus::{self, Scenario},
    read, records, workspace, yaml,
};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};

pub fn write(root: &Path, refs: &Path, all: &[Scenario]) {
    std::fs::create_dir_all(root).unwrap();
    for s in all {
        let inputs = s.fixtures().unwrap();
        let contents: Vec<_> = inputs.iter().map(|p| read(p).unwrap()).collect();
        let rows = records(&contents[0]).unwrap();
        let pin = corpus::pin(all, s).unwrap();
        let pin_rows = records(&read(&pin.fixtures().unwrap()[0]).unwrap()).unwrap();
        let route = rows
            .iter()
            .chain(pin_rows.iter())
            .find(|r| r["type"] == "request/header")
            .map(|r| r["data"]["header"]["config"].clone())
            .unwrap_or(Value::Null);
        let logs = compare::hydrate(all, s, &s.expected_logs().unwrap()).unwrap();
        let wire_file = s.dir.join(if s.suite == "acp" {
            "stdout.expected.jsonl"
        } else if s.manifest.get("sessionFormat").is_some() {
            "notifications.current.expected.jsonl"
        } else {
            "notifications.expected.jsonl"
        });
        let wire = wire_file
            .is_file()
            .then(|| records(&read(&wire_file).unwrap()).unwrap());
        let patches:Vec<_>=patch_sources(all,s,refs).iter().map(|source|json!({"source":source.to_string_lossy(),"value":yaml::parse(&read(source).unwrap(),true).unwrap()})).collect();
        let profile = s.dir.join("profile.patch.yml");
        let profile_patch=profile.is_file().then(||json!({"source":profile.to_string_lossy(),"value":yaml::parse(&read(&profile).unwrap(),true).unwrap()}));
        let tree = (s.manifest["workspace"]["final"] == true)
            .then(|| workspace::capture(&s.dir.join("workspace.expected"), &[], true).unwrap());
        let script = (s.suite == "acp").then(|| {
            hoh_util_values::js_json_parse(&read(&s.dir.join("input.json")).unwrap()).unwrap()
        });
        let plan = json!({"suite":s.suite,"name":s.name,"refs":refs.to_string_lossy(),"dir":s.dir.to_string_lossy(),"manifest":s.manifest,"inputs":inputs.iter().map(|p|p.to_string_lossy()).collect::<Vec<_>>(),"inputContents":contents,"logs":logs,"wire":wire,"script":script,"patches":patches,"profilePatch":profile_patch,"tree":tree,"route":route,"actions":actions(&rows,standalone(&s.name),s.name=="serial-created"),"task":task(&rows),"stdout":compare::final_text(&rows)+"\n","stderr":if s.suite=="session"{compare::stderr(&rows).unwrap()}else{String::new()},"exitCode":compare::expected_exit(&rows),"environment":environment(s,refs,&route,&inputs)});
        let suite = root.join(&s.suite);
        std::fs::create_dir_all(&suite).unwrap();
        std::fs::write(
            suite.join(format!("{}.json", s.name)),
            hoh_util_values::js_json_stringify(&plan),
        )
        .unwrap();
    }
}

fn environment(s: &Scenario, refs: &Path, route: &Value, inputs: &[std::path::PathBuf]) -> Value {
    let mut env = serde_json::Map::new();
    for key in [
        "DSH_SNAPSHOT_CHILD_FILES",
        "DSH_SNAPSHOT_OVERRIDE",
        "DSH_SNAPSHOT_PROVIDER",
        "DSH_SNAPSHOT_MODEL",
        "DSH_SNAPSHOT_SPILL_ROOT",
        "DSH_SNAPSHOT_SPILL_LOCATOR_ROOT",
        "DSH_SNAPSHOT_SESSIONS_ROOT",
        "DSH_PERMISSION_MODE",
        "DSH_SNAPSHOT_HUMAN_STEER",
        "DSH_SYSTEM_PROMPT",
        "DSH_TEST_CHILD_PATCH",
        "DSH_TEST_CHILD_PATCHES",
        "DSH_TEST_CHILD_HOME",
        "DSH_TEST_PARENT_PROVIDER",
        "DSH_MCP_DEMO_URL",
        "DSH_MCP_RESOURCES_FIXTURE",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        env.insert(key.into(), Value::Null);
    }
    env.insert(
        "HOME".into(),
        std::env::var("HOME")
            .ok()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    for (key, value) in [
        ("DEEPSEEK_API_KEY", "snapshot-replay-key"),
        ("DSH_SNAPSHOT", "replay"),
        ("DSH_HOME", "{{cwd}}/.dsh"),
        ("DSH_AGENTS_HOME", "{{cwd}}/.agents"),
    ] {
        env.insert(key.into(), json!(value));
    }
    let paths: Vec<_> = inputs
        .iter()
        .map(|p| {
            if s.suite == "sdk" {
                format!(
                    "{{{{cwd}}}}/.replay-fixtures/{}",
                    p.file_name().unwrap().to_string_lossy()
                )
            } else {
                p.to_string_lossy().into_owned()
            }
        })
        .collect();
    env.insert("DSH_SNAPSHOT_FILE".into(), json!(paths[0]));
    if paths.len() > 1 {
        env.insert(
            "DSH_SNAPSHOT_CHILD_FILES".into(),
            json!(paths[1..].join(if cfg!(windows) { ";" } else { ":" })),
        );
    }
    if s.manifest["replay"]["override"] == true {
        env.insert(
            "DSH_SNAPSHOT_OVERRIDE".into(),
            json!(s.dir.join("replay.override.json").to_string_lossy()),
        );
    }
    env.insert("DSH_SNAPSHOT_PROVIDER".into(), route["provider"].clone());
    env.insert("DSH_SNAPSHOT_MODEL".into(), route["model"].clone());
    if s.suite != "sdk" {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(s.name.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        env.insert("DSH_SNAPSHOT_SPILL_ROOT".into(), json!("{{spill}}"));
        env.insert(
            "DSH_SNAPSHOT_SPILL_LOCATOR_ROOT".into(),
            json!(format!(
                "{}/dsh-acp-snap-{}",
                if cfg!(windows) { "/t" } else { "/tmp" },
                &hash[..9]
            )),
        );
    }
    if s.suite == "acp" {
        env.insert(
            "DSH_SNAPSHOT_SESSIONS_ROOT".into(),
            json!("{{cwd}}/.dsh/acp-sessions"),
        );
    }
    env.insert(
        "DSH_TELEMETRY_DISABLED".into(),
        json!(if s.suite == "acp" {
            "inherited-optout"
        } else {
            "1"
        }),
    );
    env.insert(
        "NODE_OPTIONS".into(),
        json!(if s.suite == "acp" {
            "--trace-warnings"
        } else {
            "--trace-warnings --disable-warning=ExperimentalWarning"
        }),
    );
    if let Some(p) = s.manifest.get("permission") {
        env.insert("DSH_PERMISSION_MODE".into(), p.clone());
    }
    if let Some(overrides) = s.manifest["environment"].as_object() {
        env.extend(overrides.clone());
    }
    if s.suite == "sdk" {
        let p = match s.name.as_str() {
            "approval" | "plan-review" | "goal-rounds" | "command-attachments" => {
                Some("workspace-write")
            }
            "cua-run-code" | "permission-switch" | "command-identity" => Some("danger-full-access"),
            _ => None,
        };
        if let Some(p) = p {
            env.insert("DSH_PERMISSION_MODE".into(), json!(p));
        }
        match s.name.as_str() {
            "persistent-tools" => {
                env.insert(
                    "DSH_SYSTEM_PROMPT".into(),
                    json!("You are the environment-selected minimal software engineer."),
                );
            }
            "subagent-continuable" => {
                env.insert("DSH_SNAPSHOT_HUMAN_STEER".into(), json!("1"));
            }
            "subagent-dsh-sdk-diagnostic" => {
                env.insert(
                    "DSH_TEST_CHILD_PATCH".into(),
                    json!(s.dir.join("child.cordis.yml").to_string_lossy()),
                );
            }
            "subagent-dsh-sdk-dynamic-route" => {
                env.insert(
                    "DSH_TEST_PARENT_PROVIDER".into(),
                    json!("deepseek-official"),
                );
                env.insert("DSH_TEST_CHILD_HOME".into(), json!("{{cwd}}/.child-dsh"));
                env.insert("DSH_TEST_CHILD_PATCHES".into(),json!("[\"{{cwd}}/.snapshot-patches/child-0.yml\",\"{{cwd}}/.snapshot-patches/child-1.yml\"]"));
            }
            _ => {}
        }
    }
    if ["mcp-resources", "mcp-resources-ptc"].contains(&s.name.as_str()) {
        env.insert(
            "DSH_MCP_RESOURCES_FIXTURE".into(),
            json!(
                refs.join("packages/mcp/mcp-client/tests/fixtures/resources-server.ts")
                    .to_string_lossy()
            ),
        );
    }
    Value::Object(env)
}

fn task(rows: &[Value]) -> Option<String> {
    let text = |m: &Value| -> Option<String> {
        let blocks = m["content"].as_array()?;
        (m["source"]["kind"] == "user" && blocks.len() == 1 && blocks[0]["type"] == "text")
            .then(|| blocks[0]["text"].as_str().map(str::to_owned))
            .flatten()
    };
    for r in rows.iter().filter(|r| r["type"] == "agent/inbox/spliced") {
        for m in r["data"]["inserted"].as_array().into_iter().flatten() {
            if let Some(t) = text(m) {
                return Some(t);
            }
        }
    }
    rows.iter()
        .filter(|r| r["type"] == "user/message")
        .find_map(|r| text(&r["data"]))
}
fn standalone(name: &str) -> bool {
    [
        "manual-compact",
        "permission-switch",
        "command-identity",
        "plan-review",
        "goal-rounds",
        "command-attachments",
    ]
    .contains(&name)
}
fn actions(rows: &[Value], commands: bool, serial: bool) -> Vec<Value> {
    let mut actions = Vec::new();
    let mut turn: Option<u64> = None;
    let mut content = None;
    let mut message = None;
    let mut command = None;
    let mut command_messages = HashSet::new();
    for row in rows {
        let d = &row["data"];
        match row["type"].as_str().unwrap_or("") {
            "command/run" if commands && turn.is_none() && d["source"]["kind"] == "user" => {
                command = d["commandId"].as_str().map(str::to_owned);
                actions.push(json!({"turn":null,"content":[{"type":"text","text":format!("/{}{}",d["name"].as_str().unwrap(),d["args"].as_str().unwrap())}],"commandId":command}));
            }
            "agent/inbox/spliced" if command.is_some() => {
                for m in d["inserted"].as_array().into_iter().flatten() {
                    if let Some(id) = m["id"].as_str() {
                        command_messages.insert(id.to_owned());
                    }
                }
            }
            "command/done" if d["commandId"].as_str() == command.as_deref() => command = None,
            "turn/start" => {
                turn = d["turn"].as_u64();
                content = None;
                message = None;
            }
            "user/message"
                if turn.is_some()
                    && content.is_none()
                    && d["source"]["kind"] == "user"
                    && !d["id"]
                        .as_str()
                        .is_some_and(|id| command_messages.contains(id)) =>
            {
                content = Some(d["content"].clone());
                message = d["id"].as_str().map(str::to_owned);
            }
            "turn/end" => {
                if let Some(t) = turn.take()
                    && (!serial || content.is_some())
                {
                    actions.push(json!({"turn":t,"content":content,"messageId":message}));
                }
            }
            _ => {}
        }
    }
    actions
}

// sdk.snapshot.ts:142–285,710–745; headless.snapshot.ts:1385–1391; launcher.ts:354–368.
fn patch_sources(all: &[Scenario], s: &Scenario, refs: &Path) -> Vec<std::path::PathBuf> {
    if s.suite == "acp" {
        return vec![
            refs.join("snapshots/acp/escalation-approved/cordis.yml"),
            refs.join(if s.composition() == "image-compaction" {
                "snapshots/acp/image-compaction/cordis.snapshot.yml"
            } else {
                "snapshots/acp/escalation-approved/cordis.snapshot.yml"
            }),
        ];
    }
    let owner = all
        .iter()
        .find(|owner| {
            owner.suite != "acp"
                && owner.composition() == s.composition()
                && owner.dir.join("cordis.yml").exists()
        })
        .unwrap();
    let mut result = Vec::new();
    let legacy = [
        "default",
        "dynamic-tool-updates",
        "image",
        "ptc",
        "sdk-account-provider-signout",
        "sdk-default",
        "sdk-dsh-sdk-diagnostic",
        "sdk-dsh-sdk-dynamic-route",
        "sdk-minimal",
        "sdk-serial-created",
        "sdk-team-session-mode",
        "sdk-upload",
        "session-title",
        "session-title-stability",
        "subagent-activation-limit",
        "subagent-continuable-inheritance",
        "subagent-durability-failure",
        "subagent-send-message",
        "system-prompt-in-history",
    ];
    if s.suite == "sdk" && legacy.contains(&s.composition()) {
        result.push(refs.join("snapshots/sdk/legacy-context.patch.yml"));
    }
    if s.suite == "sdk" && s.composition().starts_with("sdk-") {
        result.extend([
            owner.dir.join("cordis.yml"),
            owner.dir.join("cordis.snapshot.yml"),
        ]);
    } else {
        result.extend([
            refs.join("snapshots/session/text-turn/cordis.yml"),
            owner.dir.join("cordis.snapshot.yml"),
            refs.join("snapshots/session/text-turn/model.cordis.yml"),
        ]);
    }
    if s.suite == "session" {
        if s.dir.join("runtime.cordis.yml").exists() {
            result.push(s.dir.join("runtime.cordis.yml"));
        }
    } else {
        match s.name.as_str() {
            "team-default-enabled" => result.extend([
                refs.join("apps/cli/config/examples/team-mode.yml"),
                s.dir.join("capture.cordis.yml"),
            ]),
            "manual-compact"
            | "permission-switch"
            | "plan-review"
            | "goal-rounds"
            | "command-attachments" => result.push(s.dir.join("capture.cordis.yml")),
            "subagent-fork-in-process" | "subagent-mixed" | "subagent-continuable-inheritance" => {
                result.push(refs.join("snapshots/sdk/subagent-spawn-in-process/runtime.cordis.yml"))
            }
            "approval"
            | "cua-run-code"
            | "command-identity"
            | "tool-scheduler-recovery"
            | "tool-error-details"
            | "ptc-cancel"
            | "ptc-turn"
            | "subagent-spawn-in-process" => result.push(s.dir.join("runtime.cordis.yml")),
            "session-resume" => result.push(s.dir.join("replay.cordis.yml")),
            _ => {}
        }
    }
    result
}
