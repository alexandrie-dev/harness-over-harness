//! One confirmation by the person per settings action (CEO 19.11): the engine's own approval
//! request for exactly that action is signed by the core without a second window; everything
//! else still gets a window.
use hoh_desktop::approval::{Begun, Broker, GRANT_TTL_MS, Resolution};
use hoh_protocol::{ApprovalOutcome, HostApprovalRequest};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const NOW: u64 = 1_000_000;
const KEY: [u8; 32] = [0x33; 32];

fn broker() -> Broker {
    Broker::new(KEY).unwrap()
}

/// The person presses "Разрешить" (or "Отклонить") in the window the core opened for a window action.
fn human_decides(
    broker: &Broker,
    method: &str,
    params: &Value,
    outcome: ApprovalOutcome,
    now: u64,
) {
    let (open, receiver) = broker.begin_preflight(method, params, now).unwrap();
    broker
        .current(
            &open.label,
            now,
            Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
        )
        .unwrap();
    let allowed = outcome == ApprovalOutcome::AllowedOnce;
    let decided = broker
        .decide(
            &open.label,
            &open.view.approval_id,
            outcome,
            now,
            Instant::now(),
        )
        .unwrap();
    assert!(matches!(decided.resolution, Resolution::Preflight { allowed: a, .. } if a == allowed));
    drop(receiver);
}

/// The outgoing request of that window action leaves for the engine (generation 1, id `native-1-1`).
fn arm(broker: &Broker, method: &str, params: &Value, now: u64) {
    broker.arm_in_flight(method, params, 1, "native-1-1", now);
}

/// The person allows it in the settings window and the action is sent to the engine.
fn confirmed(broker: &Broker, method: &str, params: &Value, now: u64) {
    human_decides(broker, method, params, ApprovalOutcome::AllowedOnce, now);
    arm(broker, method, params, now);
}

/// The approval/request the engine sends for a privileged method (as hoh-user-approval builds it).
fn engine_request(method: &str, params: &Value, id: &str, now: u64) -> Value {
    let params = if method == "hoh/model/key/set" {
        json!({
            "keyHash": hoh_protocol::approval::action_hash(&json!({"apiKey": params["apiKey"]})),
            "baseUrl": params["baseUrl"],
        })
    } else {
        params.clone()
    };
    let action = json!({"method": method, "params": params});
    json!({
        "sessionId": params.get("sessionId").and_then(Value::as_str).unwrap_or("host"), "toolName": method, "approvalId": id, "risk": "high",
        "humanText": "Подтвердите действие движка",
        "actionHash": hoh_protocol::approval::action_hash(&action),
        "expiresAt": now + 300_000, "action": action,
    })
}

fn begin(broker: &Broker, params: Value, now: u64) -> Begun {
    broker
        .begin_engine_or_grant(1, json!(7), "approval/request", params, now)
        .unwrap()
}

fn is_signed(begun: &Begun) -> bool {
    matches!(begun, Begun::Signed(Resolution::Engine { decision: Ok(d), .. }) if d.outcome == ApprovalOutcome::AllowedOnce)
}

fn close_window(broker: &Broker, begun: Begun, now: u64) {
    match begun {
        Begun::Window(open) => drop(broker.reject(&open.label, now)),
        Begun::Signed(_) => panic!("expected a window"),
    }
}

const KEY_SET: &str = "hoh/model/key/set";
fn key_params(key: &str, url: &str) -> Value {
    json!({"apiKey": key, "baseUrl": url})
}

#[test]
fn key_address_folder_and_probe_each_need_one_window_not_two() {
    let broker = broker();
    let cases = [
        (KEY_SET, key_params("sk-first", "http://127.0.0.1:8080/v1")),
        (
            "hoh/settings/set",
            json!({"model":{"baseUrl":"http://127.0.0.1:8080/v1","model":"local"}}),
        ),
        (
            "hoh/model/probe",
            json!({"baseUrl":"http://127.0.0.1:8080/v1","model":"local"}),
        ),
        (
            "hoh/settings/rules/set",
            json!({"internet":"ask","folder":"/home/u/Documents/HoH"}),
        ),
    ];
    for (n, (method, params)) in cases.iter().enumerate() {
        confirmed(&broker, method, params, NOW);
        let begun = begin(
            &broker,
            engine_request(method, params, &format!("eng-{n}"), NOW + 500),
            NOW + 500,
        );
        assert!(
            is_signed(&begun),
            "{method} should be signed without a window"
        );
        if let Begun::Signed(Resolution::Engine {
            decision: Ok(decision),
            ..
        }) = begun
        {
            // The signature is the real one: the engine verifies it with the same key.
            let request: HostApprovalRequest = serde_json::from_value(engine_request(
                method,
                params,
                &format!("eng-{n}"),
                NOW + 500,
            ))
            .unwrap();
            hoh_protocol::approval::verify_decision(&KEY, &request, &decision, NOW + 500).unwrap();
        }
    }
}

#[test]
fn a_foreign_request_in_the_same_gap_is_not_signed() {
    let broker = broker();
    let params = key_params("sk-mine", "http://127.0.0.1:8080/v1");
    confirmed(&broker, KEY_SET, &params, NOW);
    let t = NOW + 100;
    let foreign = [
        // another method
        engine_request(
            "hoh/settings/set",
            &json!({"model":{"baseUrl":"http://127.0.0.1:8080/v1","model":"m"}}),
            "f1",
            t,
        ),
        // same method, another address
        engine_request(
            KEY_SET,
            &key_params("sk-mine", "https://evil.example/v1"),
            "f2",
            t,
        ),
        // same method and address, another key
        engine_request(
            KEY_SET,
            &key_params("sk-other", "http://127.0.0.1:8080/v1"),
            "f3",
            t,
        ),
        // a request of the model's tool, even one that borrows the method name
        json!({"sessionId":"window-main","toolName":"bash","approvalId":"f4","risk":"high",
               "humanText":"Выполнить команду","expiresAt":t+300_000,
               "actionHash":hoh_protocol::approval::action_hash(&json!({"method":"bash","params":{"command":"rm -rf ."}})),
               "action":{"method":"bash","params":{"command":"rm -rf ."}}}),
    ];
    for request in foreign {
        close_window(&broker, begin(&broker, request, t), t);
    }
    // None of those spent the grant: the genuine request still passes, exactly once.
    let genuine = engine_request(KEY_SET, &params, "g1", t);
    assert!(is_signed(&begin(&broker, genuine.clone(), t)));
    let mut again = genuine;
    again["approvalId"] = json!("g2");
    close_window(&broker, begin(&broker, again, t), t);
}

#[test]
fn a_model_action_always_gets_a_window_even_right_after_setup() {
    let broker = broker();
    let rules = json!({"internet":"ask","folder":"/home/u/Documents/HoH"});
    confirmed(&broker, "hoh/settings/rules/set", &rules, NOW);
    for (n, command) in ["rm -rf ./generated", "cat ~/.ssh/id_rsa", "pwd"]
        .iter()
        .enumerate()
    {
        let action = json!({"method":"bash","params":{"command":command}});
        let request = json!({"sessionId":"window-main","toolName":"bash","approvalId":format!("m{n}"),
            "risk":"high","humanText":"Выполнить команду","expiresAt":NOW+300_000,
            "actionHash":hoh_protocol::approval::action_hash(&action),"action":action});
        close_window(&broker, begin(&broker, request, NOW + 50), NOW + 50);
    }
}

#[test]
fn the_grant_expires_is_not_given_for_a_rejection_and_dies_with_the_engine() {
    let broker = broker();
    let params = key_params("sk-late", "http://127.0.0.1:8080/v1");
    confirmed(&broker, KEY_SET, &params, NOW);
    let late = NOW + GRANT_TTL_MS + 1;
    close_window(
        &broker,
        begin(
            &broker,
            engine_request(KEY_SET, &params, "late", late),
            late,
        ),
        late,
    );

    human_decides(&broker, KEY_SET, &params, ApprovalOutcome::Rejected, NOW);
    close_window(
        &broker,
        begin(
            &broker,
            engine_request(KEY_SET, &params, "rej", NOW + 10),
            NOW + 10,
        ),
        NOW + 10,
    );

    confirmed(&broker, KEY_SET, &params, NOW);
    assert!(broker.cancel().is_none());
    close_window(
        &broker,
        begin(
            &broker,
            engine_request(KEY_SET, &params, "gone", NOW + 10),
            NOW + 10,
        ),
        NOW + 10,
    );
}

#[test]
fn any_privileged_action_of_the_window_needs_one_window_bound_to_its_parameters() {
    let broker = broker();
    let wd = json!({"sessionId":"window-main","path":"/home/u/other"});
    confirmed(&broker, "session/working-directory/set", &wd, NOW);
    // another path in the same gap is a different action: it gets its own window
    let other = json!({"sessionId":"window-main","path":"/home/u"});
    close_window(
        &broker,
        begin(
            &broker,
            engine_request("session/working-directory/set", &other, "wd-other", NOW),
            NOW,
        ),
        NOW,
    );
    assert!(is_signed(&begin(
        &broker,
        engine_request("session/working-directory/set", &wd, "wd", NOW),
        NOW
    )));
    // used once: the same request again needs a window
    close_window(
        &broker,
        begin(
            &broker,
            engine_request("session/working-directory/set", &wd, "wd2", NOW),
            NOW,
        ),
        NOW,
    );
    // a change that needs no confirmation is not a window action at all
    let view = json!({"view":{"theme":"dark","fontSize":16}});
    assert!(
        broker
            .begin_preflight("hoh/settings/set", &view, NOW)
            .is_err()
    );
}

#[test]
fn a_secret_inside_the_model_address_is_shown_and_journaled_closed() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("hoh-redact-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let broker = Broker::new(KEY).unwrap().with_audit_file(path.clone());
    let url = "https://user:hunter2@host.example/v1?api_key=SECRET2&n=1";
    let params = key_params("sk-k", url);

    // what the person reads in the settings window and what the page may ask for
    let (open, _receiver) = broker.begin_preflight(KEY_SET, &params, NOW).unwrap();
    let current = broker
        .current(
            &open.label,
            NOW,
            Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
        )
        .unwrap();
    for text in [&open.view.human_text, &current.human_text] {
        assert!(
            text.contains("user:•••@host.example/v1?api_key=•••&n=1"),
            "{text}"
        );
        assert!(
            !text.contains("hunter2") && !text.contains("SECRET2"),
            "{text}"
        );
    }
    broker
        .decide(
            &open.label,
            &open.view.approval_id,
            ApprovalOutcome::AllowedOnce,
            NOW,
            Instant::now(),
        )
        .unwrap();

    // the engine's own request carries the address as it was given: the window shows it as given
    arm(&broker, KEY_SET, &params, NOW);
    let mut request = engine_request(KEY_SET, &params, "eng-r", NOW + 1);
    request["humanText"] = json!(format!("Сохранить API-ключ для провайдера «{url}»?"));
    let Begun::Signed(_) = begin(&broker, request.clone(), NOW + 1) else {
        panic!("granted")
    };
    request["approvalId"] = json!("eng-r2");
    let Begun::Window(window) = begin(&broker, request, NOW + 2) else {
        panic!("second needs a window")
    };
    // the engine's text is shown as it is (a mask there could hide a nested command)
    assert!(window.view.human_text.contains(url));
    drop(broker.reject(&window.label, NOW + 3));

    let journal = std::fs::read_to_string(&path).unwrap();
    assert!(
        journal.contains("user:•••@host.example") && journal.contains("api_key=•••"),
        "{journal}"
    );
    for secret in ["hunter2", "SECRET2", "sk-k"] {
        assert!(
            !journal.contains(secret),
            "{secret} reached approvals.log: {journal}"
        );
    }
    assert!(
        !serde_json::to_string(&broker.audit_log())
            .unwrap()
            .contains("hunter2")
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn redaction_closes_only_the_secret_parts() {
    use hoh_desktop::approval::{redact_address, redact_text};
    assert_eq!(
        redact_address("https://api.example.com/v1"),
        "https://api.example.com/v1"
    );
    assert_eq!(
        redact_address("http://127.0.0.1:8080/v1?model=a"),
        "http://127.0.0.1:8080/v1?model=a"
    );
    assert_eq!(redact_address("https://u:p@h/v1"), "https://u:•••@h/v1");
    assert_eq!(redact_address("https://p@h/v1"), "https://•••@h/v1");
    assert_eq!(
        redact_address("https://h/v1?token=abc&b=2#sig=zz"),
        "https://h/v1?token=•••&b=2#•••"
    );
    assert_eq!(
        redact_address("https://h/v1?api%5Fkey=abc"),
        "https://h/v1?api%5Fkey=•••"
    );
    assert_eq!(
        redact_text("Проверить «https://u:p@h/v1?key=1» и http://x.y/z?ok=1 ?"),
        "Проверить «https://u:•••@h/v1?key=•••» и http://x.y/z?ok=1 ?"
    );
}

#[test]
fn an_unanswered_window_cancels_with_a_valid_signature_before_the_deadline() {
    let broker = broker();
    let wd = json!({"sessionId":"window-main","path":"/home/u/x"});
    // an engine request that nobody confirmed in the settings window
    let params = engine_request("session/working-directory/set", &wd, "late-1", NOW);
    let request: HostApprovalRequest = serde_json::from_value(params.clone()).unwrap();
    let Begun::Window(open) = begin(&broker, params, NOW) else {
        panic!("window")
    };
    let at = request.expires_at - hoh_desktop::approval::EXPIRY_MARGIN_MS;
    let Some(Resolution::Engine {
        decision: Ok(decision),
        ..
    }) = broker.expire(&open.label, at)
    else {
        panic!("the engine must get a signed decision");
    };
    assert_eq!(decision.outcome, ApprovalOutcome::Cancelled);
    hoh_protocol::approval::verify_decision(&KEY, &request, &decision, at).unwrap();
    let entry = broker.audit_log().pop().unwrap();
    assert_eq!((entry.outcome.as_str(), entry.at), ("cancelled", at));
    // the slot is free again
    assert!(matches!(
        begin(
            &broker,
            engine_request("session/working-directory/set", &wd, "late-2", at),
            at
        ),
        Begun::Window(_)
    ));
}

#[test]
fn the_journal_names_who_confirmed_what_and_when_without_the_key() {
    let broker = broker();
    let params = key_params("sk-secret-in-journal", "http://127.0.0.1:8080/v1");
    confirmed(&broker, KEY_SET, &params, NOW);
    assert!(is_signed(&begin(
        &broker,
        engine_request(KEY_SET, &params, "e1", NOW + 5),
        NOW + 5
    )));
    let log = broker.audit_log();
    assert_eq!(log.len(), 2);
    let human = &log[0];
    assert_eq!(
        (human.who, human.via, human.outcome.as_str()),
        ("human", "settings-window", "allowed-once")
    );
    assert_eq!(human.note, "подтверждено человеком в окне настройки");
    assert_eq!((human.at, human.method.as_str()), (NOW, KEY_SET));
    let core = &log[1];
    assert_eq!((core.who, core.at), ("core", NOW + 5));
    assert_eq!(core.action_hash.len(), 64);
    assert!(
        !serde_json::to_string(&log)
            .unwrap()
            .contains("sk-secret-in-journal")
    );
}

#[test]
fn the_journal_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("hoh-approvals-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let broker = Broker::new(KEY).unwrap().with_audit_file(path.clone());
    confirmed(
        &broker,
        KEY_SET,
        &key_params("sk-x", "http://127.0.0.1:8080/v1"),
        NOW,
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("подтверждено человеком в окне настройки") && !text.contains("sk-x"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_file(path).unwrap();
}

/// Shared with the browser test of the approval window (ui/tests/screens-a/run.mjs).
const SECRET_VIEW: &str = include_str!("../../../ui/tests/screens-a/secret-address-view.json");

#[test]
fn the_window_receives_the_engine_text_as_it_is_and_the_journal_closed() {
    let fixture: Value = serde_json::from_str(SECRET_VIEW).unwrap();
    let path = std::env::temp_dir().join(format!("hoh-view-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let broker = Broker::new(KEY).unwrap().with_audit_file(path.clone());
    let mut request = engine_request(
        "hoh/model/probe",
        &json!({"baseUrl":"http://127.0.0.1:8080/v1","model":"m"}),
        "eng-secret",
        NOW,
    );
    request["humanText"] = fixture["raw"]["humanText"].clone();
    request["reason"] = fixture["raw"]["reason"].clone();
    let Begun::Window(open) = begin(&broker, request, NOW) else {
        panic!("window")
    };
    let seen = broker
        .current(
            &open.label,
            NOW,
            Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
        )
        .unwrap();
    for view in [&open.view, &seen] {
        // the engine's own text is shown as it is; only the journal is closed
        assert_eq!(json!(view.human_text), fixture["raw"]["humanText"]);
        assert_eq!(json!(view.reason), fixture["raw"]["reason"]);
    }
    drop(broker.reject(&open.label, NOW + 1));
    let journal = std::fs::read_to_string(&path).unwrap();
    assert!(
        journal.contains("user:•••@host.example") && journal.contains("token=•••"),
        "{journal}"
    );
    for secret in fixture["secrets"].as_array().unwrap() {
        let secret = secret.as_str().unwrap();
        assert!(
            !journal.contains(secret),
            "{secret} reached approvals.log: {journal}"
        );
        assert!(
            !serde_json::to_string(&broker.audit_log())
                .unwrap()
                .contains(secret)
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn fragments_and_encoded_separators_are_closed() {
    use hoh_desktop::approval::redact_address;
    assert_eq!(
        redact_address("https://host/v1#hunter2"),
        "https://host/v1#•••"
    );
    assert_eq!(
        redact_address("https://host/v1?a=1#hunter2"),
        "https://host/v1?a=1#•••"
    );
    assert_eq!(
        redact_address("https://host/v1?a=1#"),
        "https://host/v1?a=1"
    );
    // a second parameter written with %26 / %3B / %3D inside the first
    assert_eq!(
        redact_address("https://host/v1?a=1%26token%3Dhunter2"),
        "https://host/v1?•••"
    );
    assert_eq!(
        redact_address("https://host/v1?x=1&a=1%3Bapi_key%3Dhunter2&y=2"),
        "https://host/v1?x=1&•••&y=2"
    );
    // a harmless encoded value stays readable
    assert_eq!(
        redact_address("https://host/v1?q=a%20b"),
        "https://host/v1?q=a%20b"
    );
}

#[test]
fn separators_encoded_twice_or_more_are_found_and_unsettled_text_is_closed_whole() {
    use hoh_desktop::approval::{redact_address, redact_text};
    // %2526 -> %26 -> &, %253D -> %3D -> =
    assert_eq!(
        redact_address("https://host/v1?a=1%2526ToKeN%253DSECRET"),
        "https://host/v1?•••"
    );
    // three layers
    assert_eq!(
        redact_address("https://host/v1?x=1&a=1%252526ToKeN%25253DSECRET"),
        "https://host/v1?x=1&•••"
    );
    // the name itself encoded twice
    assert_eq!(
        redact_address("https://host/v1?%2574oken=SECRET"),
        "https://host/v1?%2574oken=•••"
    );
    // a harmless value that is encoded twice stays readable
    assert_eq!(
        redact_address("https://host/v1?q=a%2520b"),
        "https://host/v1?q=a%2520b"
    );
    // still encoded after five layers: the whole query is closed, the other parameters too
    let mut hidden = "ToKeN=SECRET&b=2".to_owned();
    hidden = hidden.replace('&', "%26").replace('=', "%3D");
    for _ in 0..6 {
        hidden = hidden.replace('%', "%25");
    }
    let address = format!("https://host/v1?x=1&a={hidden}");
    let closed = redact_address(&address);
    assert_eq!(closed, "https://host/v1?•••");
    assert!(!closed.contains("SECRET") && !closed.contains("%25"));
    // the same inside a longer text
    let text = redact_text(&format!("Проверить «{address}» сейчас"));
    assert_eq!(text, "Проверить «https://host/v1?•••» сейчас");
}

#[test]
fn the_grant_does_not_live_longer_than_the_confirmation_it_came_from() {
    let broker = broker();
    let wd = json!({"sessionId":"window-main","path":"/home/u/slow"});
    let method = "session/working-directory/set";
    // confirmed at NOW; the outgoing request gets its id just before the confirmation runs out
    human_decides(&broker, method, &wd, ApprovalOutcome::AllowedOnce, NOW);
    let armed_at = NOW + GRANT_TTL_MS - 1_000;
    broker.arm_in_flight(method, &wd, 1, "native-1-1", armed_at);
    // inside the original term it still works ...
    let ok = begin(
        &broker,
        engine_request(method, &wd, "in-term", armed_at),
        armed_at,
    );
    assert!(is_signed(&ok));
    // ... but a renewed term (arm + 30 s) must not exist: after the original deadline there is a window
    human_decides(&broker, method, &wd, ApprovalOutcome::AllowedOnce, NOW);
    broker.arm_in_flight(method, &wd, 1, "native-1-2", armed_at);
    let late = NOW + GRANT_TTL_MS + 1;
    close_window(
        &broker,
        begin(
            &broker,
            engine_request(method, &wd, "after-term", late),
            late,
        ),
        late,
    );
}

#[test]
fn an_exact_clone_of_the_request_gets_a_window_unless_it_is_the_one_in_flight() {
    let broker = broker();
    let wd = json!({"sessionId":"window-main","path":"/home/u/other"});
    let method = "session/working-directory/set";
    let clone = |id: &str| engine_request(method, &wd, id, NOW);

    // confirmed, but the outgoing request has not left yet: a clone cannot use it
    human_decides(&broker, method, &wd, ApprovalOutcome::AllowedOnce, NOW);
    close_window(&broker, begin(&broker, clone("early"), NOW), NOW);

    // in flight: only the same engine run, the same session, no call id
    arm(&broker, method, &wd, NOW);
    let mut other_session = clone("session");
    other_session["sessionId"] = json!("another-session");
    close_window(&broker, begin(&broker, other_session, NOW), NOW);
    let mut with_call = clone("call");
    with_call["callId"] = json!("call-1");
    close_window(&broker, begin(&broker, with_call, NOW), NOW);
    let other_run = broker
        .begin_engine_or_grant(2, json!(7), "approval/request", clone("run"), NOW)
        .unwrap();
    close_window(&broker, other_run, NOW);
    // none of them spent it
    assert!(is_signed(&begin(&broker, clone("genuine"), NOW)));

    // one confirmation, one grant: a second outgoing request with the same action gets none
    confirmed(&broker, method, &wd, NOW);
    broker.arm_in_flight(method, &wd, 1, "native-1-2", NOW);
    assert!(is_signed(&begin(&broker, clone("first"), NOW)));
    close_window(&broker, begin(&broker, clone("second"), NOW), NOW);

    // answered, failed or timed out: the grant is gone with the request
    confirmed(&broker, method, &wd, NOW);
    broker.disarm(1, "native-1-1");
    close_window(&broker, begin(&broker, clone("after-answer"), NOW), NOW);
    confirmed(&broker, method, &wd, NOW);
    let late = NOW + GRANT_TTL_MS + 1;
    close_window(&broker, begin(&broker, clone("too-late"), late), late);
}

#[test]
fn a_late_timer_tells_the_truth_in_the_journal() {
    use hoh_desktop::approval::EXPIRY_MARGIN_MS;
    let broker = broker();
    let wd = json!({"sessionId":"window-main","path":"/home/u/x"});
    let params = engine_request("session/working-directory/set", &wd, "slow", NOW);
    let request: HostApprovalRequest = serde_json::from_value(params.clone()).unwrap();
    let Begun::Window(open) = begin(&broker, params, NOW) else {
        panic!("window")
    };
    let margin = std::hint::black_box(EXPIRY_MARGIN_MS);
    assert!(margin >= 5_000, "the timer must wake with room to spare");
    // the machine slept: the timer ran after the deadline, nothing valid can be signed any more
    let Some(Resolution::Engine { decision, .. }) =
        broker.expire(&open.label, request.expires_at + 10)
    else {
        panic!("engine resolution");
    };
    assert!(decision.is_err());
    let entry = broker.audit_log().pop().unwrap();
    assert_eq!(entry.outcome, "expired");
    assert_eq!(entry.note, "истёк без ответа, подпись невозможна");
}

#[test]
fn an_older_journal_file_becomes_private() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("hoh-oldlog-{}.log", std::process::id()));
    std::fs::write(&path, "старая запись\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let broker = Broker::new(KEY).unwrap().with_audit_file(path.clone());
    human_decides(
        &broker,
        KEY_SET,
        &key_params("sk-y", "http://127.0.0.1:8080/v1"),
        ApprovalOutcome::AllowedOnce,
        NOW,
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("старая запись")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn the_mask_marks_a_hidden_nested_command_and_the_journal_keeps_it_closed() {
    use hoh_desktop::approval::redact_text_checked;
    // a substitution inside the closed part of an address
    let text = "curl «https://h.example/p?token=$(touch /x)&n=1»";
    let (shown, nested) = redact_text_checked(text);
    assert!(
        nested && shown.contains("token=•••") && !shown.contains("$("),
        "{shown}"
    );
    let (shown, nested) = redact_text_checked("curl https://h.example/p#`id`");
    assert!(nested && shown.ends_with("#•••"), "{shown}");
    // an ordinary closed secret, and an address nothing changed in, are not marked
    assert_eq!(
        redact_text_checked("https://h/p?api_key=1&n=1"),
        ("https://h/p?api_key=•••&n=1".to_owned(), false)
    );
    assert_eq!(
        redact_text_checked("https://h/p?a=$(id)"),
        ("https://h/p?a=$(id)".to_owned(), false)
    );
}

#[test]
fn a_hidden_nested_command_in_the_settings_text_blocks_allow_once() {
    let path = std::env::temp_dir().join(format!("hoh-nested-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let broker = Broker::new(KEY).unwrap().with_audit_file(path.clone());
    let url = "https://host.example/v1?api_key=$(touch${IFS}/x)";
    let params = key_params("sk-k", url);
    let (open, _receiver) = broker.begin_preflight(KEY_SET, &params, NOW).unwrap();
    let text = &open.view.human_text;
    assert!(
        text.contains("Часть команды скрыта, в ней есть вложенная команда"),
        "{text}"
    );
    assert!(!text.contains("touch"), "{text}");
    let read = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    broker.current(&open.label, NOW, read).unwrap();
    let denied = broker.decide(
        &open.label,
        &open.view.approval_id,
        ApprovalOutcome::AllowedOnce,
        NOW,
        Instant::now(),
    );
    assert!(denied.is_err_and(|e| e.contains("разрешить один раз нельзя")));
    // rejecting is still possible
    broker
        .decide(
            &open.label,
            &open.view.approval_id,
            ApprovalOutcome::Rejected,
            NOW,
            Instant::now(),
        )
        .unwrap();
    let journal = std::fs::read_to_string(&path).unwrap();
    assert!(
        journal.contains("\"masked\":true") && !journal.contains("touch"),
        "{journal}"
    );
    let _ = std::fs::remove_file(&path);
}

const ACCESS_SET: &str = "hoh/executor/access/set";

/// Founder 22.11 19:16: the access mode of the executor changes from the main window itself, as in
/// T3. The person's action there is the confirmation: no window, one `human` line in the journal
/// (channel `main-window`), and the engine's own request for exactly that action is signed once.
#[test]
fn the_access_mode_from_the_main_window_needs_no_window_and_is_journaled_as_the_person() {
    let broker = broker();
    for (n, mode) in ["full", "confirm"].iter().enumerate() {
        let params = json!({"executor":"claude","mode":mode});
        broker
            .confirm_from_main_window(ACCESS_SET, &params, NOW)
            .unwrap();
        arm(&broker, ACCESS_SET, &params, NOW);
        let begun = begin(
            &broker,
            engine_request(ACCESS_SET, &params, &format!("acc-{n}"), NOW + 5),
            NOW + 5,
        );
        assert!(is_signed(&begun), "{mode}: signed without a window");
    }
    let log = broker.audit_log();
    assert_eq!(log.len(), 4);
    for (human, core) in [(&log[0], &log[1]), (&log[2], &log[3])] {
        assert_eq!(
            (human.who, human.via, human.outcome.as_str()),
            ("human", "main-window", "allowed-once")
        );
        assert_eq!(human.method, ACCESS_SET);
        assert_eq!(
            human.note,
            "подтверждено человеком действием в главном окне"
        );
        assert_eq!((core.who, core.outcome.as_str()), ("core", "allowed-once"));
    }
    assert!(
        log[0]
            .human_text
            .starts_with("Включить полный доступ для Claude?")
    );
    // The person reads it in the history as their own decision.
    let history = broker.history(10);
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|entry| entry["asker"] == "you"));
}

/// The grant is the main window's only: without that action (the executor, the model or the SDK
/// server asking the engine), or for another mode or executor, the engine's request gets a window.
#[test]
fn an_access_change_not_made_in_the_main_window_still_gets_a_window() {
    let broker = broker();
    let full = json!({"executor":"claude","mode":"full"});
    close_window(
        &broker,
        begin(&broker, engine_request(ACCESS_SET, &full, "x1", NOW), NOW),
        NOW,
    );
    let confirm = json!({"executor":"claude","mode":"confirm"});
    broker
        .confirm_from_main_window(ACCESS_SET, &confirm, NOW)
        .unwrap();
    arm(&broker, ACCESS_SET, &confirm, NOW);
    close_window(
        &broker,
        begin(
            &broker,
            engine_request(ACCESS_SET, &full, "x2", NOW + 1),
            NOW + 1,
        ),
        NOW + 1,
    );
    // Confirmed but never sent from the window: nothing is in flight, nothing is signed.
    let other = Broker::new(KEY).unwrap();
    other
        .confirm_from_main_window(ACCESS_SET, &full, NOW)
        .unwrap();
    close_window(
        &other,
        begin(
            &other,
            engine_request(ACCESS_SET, &full, "x3", NOW + 1),
            NOW + 1,
        ),
        NOW + 1,
    );
}

/// Every other privileged method keeps its confirmation window.
#[test]
fn only_the_access_mode_is_confirmed_by_the_main_window_itself() {
    let broker = broker();
    for (method, params) in [
        (KEY_SET, key_params("sk-x", "http://127.0.0.1:8080/v1")),
        (
            "hoh/executor/select",
            json!({"executor":"claude","model":"opus"}),
        ),
        (
            "hoh/settings/rules/set",
            json!({"internet":"ask","folder":"/home/u/Documents/HoH"}),
        ),
        (
            "hoh/settings/set",
            json!({"model":{"baseUrl":"http://127.0.0.1:8080/v1","model":"local"}}),
        ),
    ] {
        assert!(
            broker
                .confirm_from_main_window(method, &params, NOW)
                .is_err(),
            "{method}"
        );
    }
    assert!(
        broker
            .confirm_from_main_window(ACCESS_SET, &json!({"executor":"claude","mode":"root"}), NOW)
            .is_err()
    );
    assert!(broker.audit_log().is_empty());
}

/// A permission request of the engine as the T3 client shows it in its conversation.
fn tool_request(id: &str, now: u64) -> Value {
    let action = json!({"method":"bash","params":{"command":"rm -rf ./build"}});
    json!({"sessionId":"s-claude","toolName":"bash","approvalId":id,"risk":"high",
        "humanText":"Run rm -rf ./build?","expiresAt":now+300_000,
        "actionHash":hoh_protocol::approval::action_hash(&action),"action":action})
}

/// Founder 22.11 19:16: in the T3 client the permission request is answered in the conversation,
/// with no window of its own; the same checks and signature, journaled as the person's via `chat`.
#[test]
fn a_permission_request_shown_in_the_conversation_is_answered_from_the_main_window() {
    let broker = broker();
    let Begun::Window(open) = begin(&broker, tool_request("p1", NOW), NOW) else {
        panic!("an engine request needs the person")
    };
    let shown = Instant::now();
    assert!(broker.show_in_chat(&open.label, shown));
    // Not before it could be read.
    assert!(
        broker
            .decide_in_chat("p1", ApprovalOutcome::AllowedOnce, NOW, shown)
            .is_err()
    );
    // Only the request shown there, by its id.
    assert!(
        broker
            .decide_in_chat(
                "other",
                ApprovalOutcome::AllowedOnce,
                NOW,
                shown + Duration::from_secs(2)
            )
            .is_err()
    );
    let decided = broker
        .decide_in_chat(
            "p1",
            ApprovalOutcome::AllowedOnce,
            NOW + 2_000,
            shown + Duration::from_secs(2),
        )
        .unwrap();
    assert!(decided.command_result.is_ok());
    let Resolution::Engine {
        decision: Ok(decision),
        ..
    } = decided.resolution
    else {
        panic!("the engine gets the decision")
    };
    let request: HostApprovalRequest = serde_json::from_value(tool_request("p1", NOW)).unwrap();
    hoh_protocol::approval::verify_decision(&KEY, &request, &decision, NOW + 2_000).unwrap();
    let log = broker.audit_log();
    assert_eq!(
        (log[0].who, log[0].via, log[0].outcome.as_str()),
        ("human", "chat", "allowed-once")
    );
    assert_eq!(
        log[0].note,
        "подтверждено человеком в разговоре главного окна"
    );
    assert_eq!(broker.history(5)[0]["asker"], "assistant");
    // Answered once.
    assert!(
        broker
            .decide_in_chat(
                "p1",
                ApprovalOutcome::Rejected,
                NOW + 3_000,
                shown + Duration::from_secs(3)
            )
            .is_err()
    );
}

/// The previous shell keeps its approval window: a request not shown in the conversation, and the
/// shell's own settings question, cannot be answered from the main window.
#[test]
fn only_a_request_shown_in_the_conversation_is_answered_there() {
    let broker = broker();
    let Begun::Window(open) = begin(&broker, tool_request("p2", NOW), NOW) else {
        panic!("an engine request needs the person")
    };
    let later = Instant::now() + Duration::from_secs(5);
    assert!(
        broker
            .decide_in_chat("p2", ApprovalOutcome::AllowedOnce, NOW, later)
            .is_err()
    );
    drop(broker.reject(&open.label, NOW));
    let (settings, _receiver) = broker
        .begin_preflight(
            KEY_SET,
            &key_params("sk-z", "http://127.0.0.1:8080/v1"),
            NOW,
        )
        .unwrap();
    assert!(!broker.show_in_chat(&settings.label, Instant::now()));
    assert!(
        broker
            .decide_in_chat(
                &settings.view.approval_id,
                ApprovalOutcome::AllowedOnce,
                NOW,
                later
            )
            .is_err()
    );
    // A rejection in the conversation is journaled as the person's too.
    drop(broker.reject(&settings.label, NOW));
    let Begun::Window(open) = begin(&broker, tool_request("p3", NOW), NOW) else {
        panic!("an engine request needs the person")
    };
    assert!(broker.show_in_chat(&open.label, Instant::now()));
    broker
        .decide_in_chat("p3", ApprovalOutcome::Rejected, NOW, Instant::now())
        .unwrap();
    let last = broker.audit_log().pop().unwrap();
    assert_eq!(
        (last.who, last.via, last.outcome.as_str()),
        ("human", "chat", "rejected")
    );
}
