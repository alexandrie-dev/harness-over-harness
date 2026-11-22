//! Native-only approval state. JavaScript sees a redacted view and never the key or MAC.
use hoh_protocol::{ApprovalOutcome, HostApprovalDecision, HostApprovalRequest, Risk};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const ALLOW_DELAY: Duration = Duration::from_secs(1);
const MAX_LIFETIME_MS: u64 = 300_000;
/// A settings action the person confirmed lets the engine's own approval request for that very
/// action through, once, within this time counted from the confirmation (arming the outgoing
/// request does not extend it).
///
/// Known limitation (accepted): while the confirmed outgoing request is in flight, an engine
/// approval/request with the same generation, method, action and session and no call id cannot be
/// told apart from the genuine one without a protocol change; it means the same action the person
/// already confirmed, so it takes the grant.
pub const GRANT_TTL_MS: u64 = 30_000;
/// The expiry timer fires this long before `expiresAt`, so the signed `cancelled` is still valid.
/// Known limitation (accepted): if the timer wakes after `expiresAt` (the machine slept), the protocol
/// forbids signing; the engine cancels the request by its own deadline and the journal says `expired`.
pub const EXPIRY_MARGIN_MS: u64 = 5_000;
const MAX_AUDIT_ENTRIES: usize = 1_000;
/// The «Подтверждения» section reads at most this much of the end of the journal file.
pub const HISTORY_TAIL_BYTES: u64 = 512 * 1024;

/// The last journal lines, newest first, parsed one by one: a line that is not valid UTF-8 or JSON
/// is skipped (or read lossily), never the whole file. Reads at most `max_bytes` from the end and
/// looks at no more than `max_lines` lines.
pub fn read_journal_tail(path: &std::path::Path, max_bytes: u64, max_lines: usize) -> Vec<Value> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(length) = file.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let start = length.saturating_sub(max_bytes);
    // One byte more is read from before the window: if it is a newline the window begins exactly
    // at the start of a line and that line is whole; otherwise the first line is cut and dropped.
    let from = start.saturating_sub(1);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err()
        || file
            .take(max_bytes + (start - from))
            .read_to_end(&mut bytes)
            .is_err()
    {
        return Vec::new();
    }
    let (cut, body) = if start > 0 {
        (
            bytes.first() != Some(&b'\n'),
            bytes.get(1..).unwrap_or_default(),
        )
    } else {
        (false, &bytes[..])
    };
    let mut lines = body.split(|b| *b == b'\n');
    if cut {
        lines.next();
    }
    let lines: Vec<&[u8]> = lines.filter(|line| !line.is_empty()).collect();
    lines
        .iter()
        .rev()
        .take(max_lines)
        .filter_map(|line| serde_json::from_str(&String::from_utf8_lossy(line)).ok())
        .collect()
}

const MASK: &str = "•••";
const SECRET_WORDS: [&str; 8] = [
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "auth",
    "sig",
    "credential",
];

/// An address may carry a secret (`https://user:pass@host/v1`, `?api_key=...`). The engine refuses
/// such an address, but the person must never see or journal the secret part: it is shown closed.
pub fn redact_address(address: &str) -> String {
    let (scheme, rest) = match address.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), address),
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let authority = match authority.rsplit_once('@') {
        Some((userinfo, host)) => {
            let user = userinfo.split_once(':').map_or("", |(user, _)| user);
            format!(
                "{user}{}{MASK}@{host}",
                if user.is_empty() { "" } else { ":" }
            )
        }
        None => authority.to_owned(),
    };
    let (path, rest) = match tail.find(['?', '#']) {
        Some(at) => tail.split_at(at),
        None => (tail, ""),
    };
    let (query, fragment) = match rest.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (rest, None),
    };
    let mut out = format!("{scheme}{authority}{path}");
    if let Some(query) = query.strip_prefix('?') {
        out.push('?');
        out.push_str(&redact_query(query));
    }
    // A fragment is never needed to reach a model and can hold anything: it is closed whole.
    if fragment.is_some_and(|fragment| !fragment.is_empty()) {
        out.push('#');
        out.push_str(MASK);
    }
    out
}

fn redact_query(query: &str) -> String {
    // Encoded again and again: what the receiver finally decodes cannot be judged here.
    if percent_decode_stable(query).is_none() {
        return MASK.to_owned();
    }
    let mut out = String::with_capacity(query.len());
    let mut pair = String::new();
    for ch in query.chars() {
        if matches!(ch, '&' | ';') {
            out.push_str(&redact_pair(&pair));
            out.push(ch);
            pair.clear();
        } else {
            pair.push(ch);
        }
    }
    out.push_str(&redact_pair(&pair));
    out
}

fn redact_pair(pair: &str) -> String {
    if let Some((name, _)) = pair.split_once('=')
        && is_secret_name(name)
    {
        return format!("{name}={MASK}");
    }
    // A separator written as %26 / %3B / %3D (also encoded twice, %2526) hides a second parameter.
    let Some(decoded) = percent_decode_stable(pair) else {
        return MASK.to_owned();
    };
    if decoded != pair
        && decoded
            .split(['&', ';'])
            .any(|inner| inner.split('=').next().is_some_and(is_secret_name))
    {
        return MASK.to_owned();
    }
    pair.to_owned()
}

fn is_secret_name(name: &str) -> bool {
    let name = percent_decode_stable(name)
        .unwrap_or_else(|| percent_decode(name))
        .to_ascii_lowercase();
    SECRET_WORDS.iter().any(|word| name.contains(word))
}

/// Every address inside a text (`scheme://...` up to a space or quote) with its secret part closed.
pub fn redact_text(text: &str) -> String {
    redact_text_checked(text).0
}

/// The note the window shows when the mask hid a part that holds a nested command.
const HIDDEN_NESTED_NOTE: &str = "⚠ Часть команды скрыта, в ней есть вложенная команда. Это опасно: «Разрешить один раз» недоступно.";

/// A shell would run something inside this word: a substitution `$(…)`, backticks, a pipe, or a
/// `;` / `&` outside the query (inside the query they only separate its pairs).
fn has_nested_command(word: &str) -> bool {
    if word.contains("$(") || word.contains('`') || word.contains('|') {
        return true;
    }
    let (head, fragment) = word.split_once('#').unwrap_or((word, ""));
    let head = head.split_once('?').map_or(head, |(head, _)| head);
    head.contains([';', '&']) || fragment.contains([';', '&'])
}

/// [`redact_text`], and whether the mask changed a word that holds a nested command.
pub fn redact_text_checked(text: &str) -> (String, bool) {
    let mut nested = false;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let start = rest[..at]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
            .map_or(0, |i| {
                i + rest[i..].chars().next().map_or(1, char::len_utf8)
            });
        let after = at + 3;
        let end = rest[after..]
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '«' | '»' | '"' | '\'' | '<' | '>' | '“' | '”')
            })
            .map_or(rest.len(), |i| after + i);
        out.push_str(&rest[..start]);
        let word = &rest[start..end];
        let masked = redact_address(word);
        nested |= masked != word && has_nested_command(word);
        out.push_str(&masked);
        rest = &rest[end..];
    }
    out.push_str(rest);
    (out, nested)
}

const MAX_DECODE_LAYERS: usize = 5;

/// Decodes until nothing changes. `None` when `%XX` sequences remain after `MAX_DECODE_LAYERS`
/// layers: such text is deliberately obscure and the caller closes it whole.
fn percent_decode_stable(text: &str) -> Option<String> {
    let mut current = text.to_owned();
    for _ in 0..MAX_DECODE_LAYERS {
        let next = percent_decode(&current);
        if next == current {
            return Some(current);
        }
        current = next;
    }
    (percent_decode(&current) == current).then_some(current)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((high * 16 + low) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A window action the person confirmed; it waits for its outgoing request to be sent.
struct Confirmed {
    method: String,
    action: Value,
    session: String,
    expires_at: u64,
}

/// The confirmation, bound to the one outgoing JSON-RPC request the window action became. It
/// exists only while that request is in flight.
struct InFlight {
    generation: u64,
    id: String,
    method: String,
    action: Value,
    session: String,
    expires_at: u64,
}

/// The session the engine puts into the approval request of a method call.
fn session_of(action: &Value) -> String {
    action
        .pointer("/params/sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or("host")
        .to_owned()
}

/// The executor whose choices the person makes in the main window itself (the T3 client: its model
/// picker, its mode chip, its project folder).
pub const MAIN_WINDOW_EXECUTOR: &str = "claude";

/// Privileged window requests whose confirmation is the person's action in the main window itself,
/// without a separate confirmation window (founder 22.11 19:16, lead 22.11 20:43; as in T3): the
/// executor's access mode, Claude chosen to answer, and the folder of a conversation Claude runs
/// (`session_executor` — what the engine reports for that conversation). The built-in agent loop's
/// model and folder keep their window. Only the main window may send these (`bridge::check_main`);
/// the executor, the model and the SDK server never reach this path, and the engine's own request
/// still needs the core's signature.
pub fn main_window_confirms(method: &str, params: &Value, session_executor: Option<&str>) -> bool {
    match method {
        "hoh/executor/access/set" => true,
        "hoh/executor/select" => params["executor"].as_str() == Some(MAIN_WINDOW_EXECUTOR),
        "session/working-directory/set" => session_executor == Some(MAIN_WINDOW_EXECUTOR),
        _ => false,
    }
}

/// One line of the approval journal: who confirmed what, where and when. No secrets: the model key
/// appears only as its hash inside `actionHash`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub at: u64,
    pub who: &'static str,
    pub via: &'static str,
    pub method: String,
    pub action_hash: String,
    pub human_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub outcome: String,
    pub note: &'static str,
    /// The text above is closed by the mask (an address in it had a secret part).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub masked: bool,
}

/// What the core does with an approval request of the engine.
pub enum Begun {
    /// Ask the person in a window.
    Window(OpenApproval),
    /// The person already confirmed exactly this action in the settings window: signed, no window.
    Signed(Resolution),
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    pub approval_id: String,
    pub risk: Risk,
    pub human_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Shown folded away (the raw command); closed like `reason`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub expires_at: u64,
}

pub struct OpenApproval {
    pub label: String,
    pub view: ApprovalView,
}

pub enum Resolution {
    Preflight {
        reply: mpsc::SyncSender<bool>,
        allowed: bool,
    },
    Engine {
        generation: u64,
        id: Value,
        decision: Result<HostApprovalDecision, String>,
    },
}

pub struct DecisionOutcome {
    pub resolution: Resolution,
    pub command_result: Result<(), String>,
}

enum Destination {
    Preflight(mpsc::SyncSender<bool>),
    Engine { generation: u64, id: Value },
}

struct Pending {
    label: String,
    request: HostApprovalRequest,
    shown_at: Option<Instant>,
    destination: Destination,
    /// The mask hid a nested command in the text the shell composed: no "allow once".
    blocks_allow: bool,
    /// Shown in the conversation of the main window (the T3 client), not in a window of its own.
    in_chat: bool,
}

impl Pending {
    /// The channel the person answers through, as the journal names it.
    fn via(&self) -> &'static str {
        match (&self.destination, self.in_chat) {
            (Destination::Preflight(_), _) => "settings-window",
            (Destination::Engine { .. }, true) => "chat",
            (Destination::Engine { .. }, false) => "approval-window",
        }
    }
}

pub struct Broker {
    key: [u8; 32],
    next: AtomicU64,
    pending: Mutex<Option<Pending>>,
    confirmed: Mutex<Vec<Confirmed>>,
    in_flight: Mutex<Vec<InFlight>>,
    audit: Mutex<Vec<AuditEntry>>,
    audit_file: Option<std::path::PathBuf>,
}

impl Broker {
    pub fn new(key: [u8; 32]) -> Result<Self, String> {
        hoh_protocol::approval::key_from_hex(&hoh_protocol::approval::hex(&key))?;
        Ok(Self {
            key,
            next: AtomicU64::new(0),
            pending: Mutex::new(None),
            confirmed: Mutex::new(Vec::new()),
            in_flight: Mutex::new(Vec::new()),
            audit: Mutex::new(Vec::new()),
            audit_file: None,
        })
    }

    /// Also append every journal entry to this file (owner-only).
    pub fn with_audit_file(mut self, path: std::path::PathBuf) -> Self {
        self.audit_file = Some(path);
        self
    }

    /// The decisions the person can read in the main window, newest first: who asked, what, when
    /// and the answer. Read from the journal file, so it survives a restart; the entries the core
    /// signed on the person's earlier confirmation are left out (they repeat the person's decision).
    /// Addresses stay closed, as in the journal itself.
    pub fn history(&self, limit: usize) -> Vec<Value> {
        // Newest first. The file is read from its end, boundedly, before anything is parsed.
        let newest_first: Vec<Value> = match &self.audit_file {
            Some(path) => read_journal_tail(path, HISTORY_TAIL_BYTES, limit.saturating_mul(4)),
            None => self
                .audit_log()
                .iter()
                .rev()
                .filter_map(|entry| serde_json::to_value(entry).ok())
                .collect(),
        };
        newest_first
            .iter()
            .filter(|entry| {
                entry["who"] == "human"
                    || !entry["note"]
                        .as_str()
                        .unwrap_or_default()
                        .starts_with("подписано ядром без второго окна")
            })
            .filter_map(|entry| {
                let closed = |key: &str| entry[key].as_str().map(redact_text);
                let mut record = json!({
                    "at": entry["at"].as_u64()?,
                    "asker": if entry["via"] == "approval-window" || entry["via"] == "chat" { "assistant" } else { "you" },
                    "method": entry["method"].as_str()?,
                    "humanText": closed("humanText")?,
                    "outcome": entry["outcome"].as_str()?,
                });
                if let Some(reason) = closed("reason") {
                    record["reason"] = json!(reason);
                }
                if let Some(details) = closed("details") {
                    record["details"] = json!(details);
                }
                Some(record)
            })
            .take(limit)
            .collect()
    }

    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit.lock().map(|log| log.clone()).unwrap_or_default()
    }

    fn log(
        &self,
        now: u64,
        who: &'static str,
        via: &'static str,
        request: &HostApprovalRequest,
        outcome: &str,
        note: &'static str,
    ) {
        let (human_text, _) = redact_text_checked(&request.human_text);
        let reason = request.reason.as_deref().map(redact_text_checked);
        let details = request.details.as_deref().map(redact_text_checked);
        let masked = human_text != request.human_text
            || human_text.contains(MASK)
            || [&reason, &details]
                .into_iter()
                .flatten()
                .any(|(text, _)| text.contains(MASK));
        let entry = AuditEntry {
            at: now,
            who,
            via,
            method: request.tool_name.clone(),
            action_hash: request.action_hash.clone(),
            human_text,
            reason: reason.map(|(text, _)| text),
            details: details.map(|(text, _)| text),
            outcome: outcome.to_owned(),
            note,
            masked,
        };
        if let Some(path) = &self.audit_file {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path)
            {
                // `mode` applies only when the file is created: make an older file private too.
                let _ = file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600));
                let _ = writeln!(
                    file,
                    "{}",
                    serde_json::to_string(&entry).unwrap_or_default()
                );
            }
        }
        if let Ok(mut log) = self.audit.lock() {
            if log.len() >= MAX_AUDIT_ENTRIES {
                log.remove(0);
            }
            log.push(entry);
        }
    }

    /// Engine restart or a lost connection: nothing the person confirmed earlier is valid any more.
    pub fn clear_grants(&self) {
        if let Ok(mut confirmed) = self.confirmed.lock() {
            confirmed.clear();
        }
        if let Ok(mut in_flight) = self.in_flight.lock() {
            in_flight.clear();
        }
    }

    /// The person allowed a window action. The confirmation waits for the outgoing request that
    /// carries exactly this action; it is not usable by anything yet.
    fn confirm(&self, request: &HostApprovalRequest, now: u64) {
        if let Ok(mut confirmed) = self.confirmed.lock() {
            confirmed.retain(|c| c.expires_at > now);
            confirmed.push(Confirmed {
                method: request.tool_name.clone(),
                session: session_of(&request.action),
                action: request.action.clone(),
                expires_at: now.saturating_add(GRANT_TTL_MS),
            });
        }
    }

    /// Called as the outgoing request leaves for the engine, before its frame is written:
    /// binds the matching confirmation to this request's generation and JSON-RPC id. At most one
    /// grant exists per outgoing request.
    pub fn arm_in_flight(&self, method: &str, params: &Value, generation: u64, id: &str, now: u64) {
        let Ok(action) = redacted_action(method, params) else {
            return;
        };
        let Ok(mut confirmed) = self.confirmed.lock() else {
            return;
        };
        confirmed.retain(|c| c.expires_at > now);
        let Some(index) = confirmed
            .iter()
            .position(|c| c.method == method && c.action == action)
        else {
            return;
        };
        let c = confirmed.remove(index);
        drop(confirmed);
        // the remaining time of the original confirmation, not a fresh one
        if let Ok(mut in_flight) = self.in_flight.lock() {
            in_flight.retain(|g| g.expires_at > now && !(g.generation == generation && g.id == id));
            in_flight.push(InFlight {
                generation,
                id: id.to_owned(),
                method: c.method,
                action: c.action,
                session: c.session,
                expires_at: c.expires_at,
            });
        }
    }

    /// The outgoing request was answered, failed or timed out: its confirmation dies with it.
    pub fn disarm(&self, generation: u64, id: &str) {
        if let Ok(mut in_flight) = self.in_flight.lock() {
            in_flight.retain(|g| !(g.generation == generation && g.id == id));
        }
    }

    /// Consumes the confirmation of the outgoing request this approval request belongs to: the same
    /// engine run, the same method, the same action and the same session, while that request is in flight.
    fn take_grant(&self, generation: u64, request: &HostApprovalRequest, now: u64) -> bool {
        let Ok(mut in_flight) = self.in_flight.lock() else {
            return false;
        };
        in_flight.retain(|g| g.expires_at > now);
        let found = in_flight.iter().position(|g| {
            g.generation == generation
                && g.method == request.tool_name
                && g.action == request.action
                && g.session == request.session_id
                && request.call_id.is_none()
        });
        found.map(|index| in_flight.remove(index)).is_some()
    }

    pub fn begin_preflight(
        &self,
        method: &str,
        params: &Value,
        now: u64,
    ) -> Result<(OpenApproval, mpsc::Receiver<bool>), String> {
        if !hoh_protocol::requires_host_approval(method, params) {
            return Err("Действие не требует подтверждения".into());
        }
        let sequence = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let action = redacted_action(method, params)?;
        // Only here, in the text the shell composes itself from the settings, the mask applies.
        let (mut human_text, nested) = redact_text_checked(&preflight_text(method, params)?);
        if nested {
            human_text.push(' ');
            human_text.push_str(HIDDEN_NESTED_NOTE);
        }
        let request = HostApprovalRequest {
            session_id: "native".into(),
            tool_name: method.into(),
            call_id: None,
            reason: None,
            details: None,
            approval_id: format!("native-preflight-{sequence}"),
            risk: Risk::High,
            human_text,
            action_hash: hoh_protocol::approval::action_hash(&action),
            expires_at: now.saturating_add(MAX_LIFETIME_MS),
            action,
        };
        let (reply, receiver) = mpsc::sync_channel(1);
        let open = self.insert(request, Destination::Preflight(reply), nested)?;
        Ok((open, receiver))
    }

    /// The person's own action in the main window is the confirmation (founder 22.11 19:16): for the
    /// requests `main_window_confirms` names no confirmation window opens. The core journals it as the
    /// person's (`who: human`, `via: main-window`) and keeps the grant for exactly this action, so the
    /// engine's own approval request for it is signed once, as after the settings window. The caller
    /// must have checked that the request came from the main window (`bridge::check_main`).
    pub fn confirm_from_main_window(
        &self,
        method: &str,
        params: &Value,
        session_executor: Option<&str>,
        now: u64,
    ) -> Result<(), String> {
        if !main_window_confirms(method, params, session_executor) {
            return Err("Действие подтверждается в отдельном окне".into());
        }
        if !hoh_protocol::requires_host_approval(method, params) {
            return Err("Действие не требует подтверждения".into());
        }
        let sequence = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let action = redacted_action(method, params)?;
        let (human_text, _) = redact_text_checked(&preflight_text(method, params)?);
        let request = HostApprovalRequest {
            session_id: "native".into(),
            tool_name: method.into(),
            call_id: None,
            reason: None,
            details: None,
            approval_id: format!("main-window-{sequence}"),
            risk: Risk::High,
            human_text,
            action_hash: hoh_protocol::approval::action_hash(&action),
            expires_at: now.saturating_add(MAX_LIFETIME_MS),
            action,
        };
        hoh_protocol::approval::validate_host_request(&request)
            .map_err(|_| "Некорректное подтверждение".to_owned())?;
        self.confirm(&request, now);
        self.log(
            now,
            "human",
            "main-window",
            &request,
            "allowed-once",
            "подтверждено человеком действием в главном окне",
        );
        Ok(())
    }

    fn parse_engine_request(
        method: &str,
        params: Value,
        now: u64,
    ) -> Result<HostApprovalRequest, String> {
        if method != "approval/request" {
            return Err("Обратный метод не поддерживается".into());
        }
        hoh_protocol::validate_params_host(method, &params)
            .map_err(|_| "Получено некорректное подтверждение".to_owned())?;
        let request: HostApprovalRequest = serde_json::from_value(params)
            .map_err(|_| "Получено некорректное подтверждение".to_owned())?;
        validate_time(&request, now)?;
        Ok(request)
    }

    /// The engine's request always gets a window.
    pub fn begin_engine(
        &self,
        generation: u64,
        id: Value,
        method: &str,
        params: Value,
        now: u64,
    ) -> Result<OpenApproval, String> {
        let request = Self::parse_engine_request(method, params, now)?;
        self.insert(request, Destination::Engine { generation, id }, false)
    }

    /// The engine's request gets a window, except the one the person has just confirmed in the
    /// settings window (same method and same canonical action, once, within `GRANT_TTL_MS`):
    /// that one is signed here. Requests of the model's tools never match a grant.
    pub fn begin_engine_or_grant(
        &self,
        generation: u64,
        id: Value,
        method: &str,
        params: Value,
        now: u64,
    ) -> Result<Begun, String> {
        let request = Self::parse_engine_request(method, params, now)?;
        hoh_protocol::approval::validate_host_request(&request)
            .map_err(|_| "Некорректное подтверждение".to_owned())?;
        if !self.take_grant(generation, &request, now) {
            return self
                .insert(request, Destination::Engine { generation, id }, false)
                .map(Begun::Window);
        }
        let decision = HostApprovalDecision {
            approval_id: request.approval_id.clone(),
            outcome: ApprovalOutcome::AllowedOnce,
            mac: hoh_protocol::approval::mac(&self.key, &request, "allowed-once"),
        };
        hoh_protocol::approval::verify_decision(&self.key, &request, &decision, now)
            .map_err(|_| "Подтверждение недействительно".to_owned())?;
        self.log(
            now,
            "core",
            "settings-window",
            &request,
            "allowed-once",
            "подписано ядром без второго окна: то же действие подтверждено человеком в окне настройки",
        );
        Ok(Begun::Signed(Resolution::Engine {
            generation,
            id,
            decision: Ok(decision),
        }))
    }

    fn insert(
        &self,
        request: HostApprovalRequest,
        destination: Destination,
        blocks_allow: bool,
    ) -> Result<OpenApproval, String> {
        hoh_protocol::approval::validate_host_request(&request)
            .map_err(|_| "Некорректное подтверждение".to_owned())?;
        let sequence = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let label = format!("approval-{sequence}");
        let view = ApprovalView {
            approval_id: request.approval_id.clone(),
            risk: request.risk.clone(),
            // The engine's text is shown as it is; the shell's own text was masked when composed.
            human_text: request.human_text.clone(),
            reason: request.reason.clone(),
            details: request.details.clone(),
            expires_at: request.expires_at,
        };
        let mut slot = self
            .pending
            .lock()
            .map_err(|_| "Подтверждение недоступно")?;
        if slot.is_some() {
            return Err("Другое подтверждение уже открыто".into());
        }
        *slot = Some(Pending {
            label: label.clone(),
            request,
            shown_at: None,
            destination,
            blocks_allow,
            in_chat: false,
        });
        Ok(OpenApproval { label, view })
    }

    /// The T3 client shows the engine's permission request in its own conversation (founder 22.11 19:16):
    /// no window of its own; the person reads it from now on and answers from the main window
    /// (`decide_in_chat`). Only an engine request: the shell's own settings questions keep their window.
    pub fn show_in_chat(&self, label: &str, instant: Instant) -> bool {
        let Ok(mut slot) = self.pending.lock() else {
            return false;
        };
        match slot.as_mut() {
            Some(pending)
                if pending.label == label
                    && matches!(pending.destination, Destination::Engine { .. }) =>
            {
                pending.in_chat = true;
                pending.shown_at = Some(instant);
                true
            }
            _ => false,
        }
    }

    /// The person's answer in the conversation of the main window to the request shown there: the
    /// same checks and signature as in the approval window (allow only after it could be read, not
    /// with a hidden nested command), journaled as the person's through `chat`.
    pub fn decide_in_chat(
        &self,
        approval_id: &str,
        outcome: ApprovalOutcome,
        now: u64,
        instant: Instant,
    ) -> Result<DecisionOutcome, String> {
        let label = {
            let slot = self
                .pending
                .lock()
                .map_err(|_| "Подтверждение недоступно")?;
            match slot.as_ref() {
                Some(pending) if pending.in_chat && pending.request.approval_id == approval_id => {
                    pending.label.clone()
                }
                _ => return Err("Нет такого запроса разрешения в разговоре".into()),
            }
        };
        self.decide(&label, approval_id, outcome, now, instant)
    }

    pub fn current(&self, label: &str, now: u64, instant: Instant) -> Result<ApprovalView, String> {
        let mut slot = self
            .pending
            .lock()
            .map_err(|_| "Подтверждение недоступно")?;
        let pending = slot.as_mut().ok_or("Нет ожидающего подтверждения")?;
        if pending.label != label {
            return Err("Это окно не может открыть подтверждение".into());
        }
        validate_time(&pending.request, now)?;
        pending.shown_at.get_or_insert(instant);
        Ok(ApprovalView {
            approval_id: pending.request.approval_id.clone(),
            risk: pending.request.risk.clone(),
            human_text: pending.request.human_text.clone(),
            reason: pending.request.reason.clone(),
            details: pending.request.details.clone(),
            expires_at: pending.request.expires_at,
        })
    }

    pub fn decide(
        &self,
        label: &str,
        approval_id: &str,
        outcome: ApprovalOutcome,
        now: u64,
        instant: Instant,
    ) -> Result<DecisionOutcome, String> {
        if !matches!(
            outcome,
            ApprovalOutcome::AllowedOnce | ApprovalOutcome::Rejected
        ) {
            return Err("Недопустимый ответ окна".into());
        }
        {
            let slot = self
                .pending
                .lock()
                .map_err(|_| "Подтверждение недоступно")?;
            let pending = slot.as_ref().ok_or("Нет ожидающего подтверждения")?;
            if pending.label != label {
                return Err("Это окно не может ответить на подтверждение".into());
            }
            if pending.request.approval_id != approval_id {
                return Err("Подтверждение уже сменилось".into());
            }
            if outcome == ApprovalOutcome::AllowedOnce && pending.blocks_allow {
                return Err(
                    "Часть команды скрыта, в ней есть вложенная команда: разрешить один раз нельзя"
                        .into(),
                );
            }
            if outcome == ApprovalOutcome::AllowedOnce
                && pending.shown_at.is_none_or(|shown_at| {
                    instant
                        .checked_duration_since(shown_at)
                        .is_none_or(|elapsed| elapsed < ALLOW_DELAY)
                })
            {
                return Err("Сначала прочитайте подтверждение".into());
            }
        }
        let request = {
            let slot = self
                .pending
                .lock()
                .map_err(|_| "Подтверждение недоступно")?;
            slot.as_ref()
                .map(|pending| pending.request.clone())
                .ok_or("Нет ожидающего подтверждения")?
        };
        let outcome_text = serde_json::to_value(&outcome)
            .expect("approval outcome serializes")
            .as_str()
            .expect("approval outcome is a string")
            .to_owned();
        let decision = HostApprovalDecision {
            approval_id: request.approval_id.clone(),
            outcome,
            mac: hoh_protocol::approval::mac(&self.key, &request, &outcome_text),
        };
        self.submit_signed(label, approval_id, decision, now)
    }

    pub fn submit_signed(
        &self,
        label: &str,
        approval_id: &str,
        decision: HostApprovalDecision,
        now: u64,
    ) -> Result<DecisionOutcome, String> {
        if decision.outcome == ApprovalOutcome::AllowedOnce
            && self
                .pending
                .lock()
                .map_err(|_| "Подтверждение недоступно")?
                .as_ref()
                .is_some_and(|pending| pending.label == label && pending.blocks_allow)
        {
            return Err(
                "Часть команды скрыта, в ней есть вложенная команда: разрешить один раз нельзя"
                    .into(),
            );
        }
        let pending = self.take_exact(label, approval_id)?;
        let settings_window = matches!(pending.destination, Destination::Preflight(_));
        let via = pending.via();
        let verified =
            hoh_protocol::approval::verify_decision(&self.key, &pending.request, &decision, now);
        match (&verified, &decision.outcome) {
            (Ok(()), ApprovalOutcome::AllowedOnce) => {
                if settings_window {
                    self.confirm(&pending.request, now);
                }
                self.log(
                    now,
                    "human",
                    via,
                    &pending.request,
                    "allowed-once",
                    match via {
                        "settings-window" => "подтверждено человеком в окне настройки",
                        "chat" => "подтверждено человеком в разговоре главного окна",
                        _ => "подтверждено человеком в окне подтверждения",
                    },
                );
            }
            (Ok(()), _) => self.log(
                now,
                "human",
                via,
                &pending.request,
                "rejected",
                "отклонено человеком",
            ),
            (Err(_), _) => self.log(
                now,
                "core",
                via,
                &pending.request,
                "invalid",
                "подтверждение недействительно",
            ),
        }
        match verified {
            Ok(()) => Ok(DecisionOutcome {
                resolution: resolve(
                    pending.destination,
                    Ok(decision.clone()),
                    decision.outcome == ApprovalOutcome::AllowedOnce,
                ),
                command_result: Ok(()),
            }),
            Err(_) => Ok(DecisionOutcome {
                resolution: resolve(
                    pending.destination,
                    Err("Подтверждение отклонено".into()),
                    false,
                ),
                command_result: Err("Подтверждение недействительно".into()),
            }),
        }
    }

    /// The window was not answered in time: the engine gets `cancelled`, signed while the request
    /// is still valid (so it must run before `expiresAt`, see `EXPIRY_MARGIN_MS`). A settings-window
    /// question simply fails.
    pub fn expire(&self, label: &str, now: u64) -> Option<Resolution> {
        let pending = self.take_label(label)?;
        let via = pending.via();
        let decision = HostApprovalDecision {
            approval_id: pending.request.approval_id.clone(),
            outcome: ApprovalOutcome::Cancelled,
            mac: hoh_protocol::approval::mac(&self.key, &pending.request, "cancelled"),
        };
        let verified =
            hoh_protocol::approval::verify_decision(&self.key, &pending.request, &decision, now)
                .map(|()| decision)
                .map_err(|_| "Подтверждение истекло".to_owned());
        // The journal tells what really happened: after the deadline nothing valid can be signed.
        if verified.is_ok() {
            self.log(
                now,
                "core",
                via,
                &pending.request,
                "cancelled",
                "срок подтверждения вышел: отменено",
            );
        } else {
            self.log(
                now,
                "core",
                via,
                &pending.request,
                "expired",
                "истёк без ответа, подпись невозможна",
            );
        }
        Some(resolve(pending.destination, verified, false))
    }

    pub fn reject(&self, label: &str, now: u64) -> Option<Resolution> {
        let pending = self.take_label(label)?;
        self.log(
            now,
            "human",
            pending.via(),
            &pending.request,
            "rejected",
            "окно закрыто или срок вышел: отклонено",
        );
        let decision = HostApprovalDecision {
            approval_id: pending.request.approval_id.clone(),
            outcome: ApprovalOutcome::Rejected,
            mac: hoh_protocol::approval::mac(&self.key, &pending.request, "rejected"),
        };
        let verified =
            hoh_protocol::approval::verify_decision(&self.key, &pending.request, &decision, now)
                .map(|()| decision)
                .map_err(|_| "Подтверждение истекло".into());
        Some(resolve(pending.destination, verified, false))
    }

    pub fn cancel(&self) -> Option<(String, Resolution)> {
        self.clear_grants();
        let pending = self.pending.lock().ok()?.take()?;
        let label = pending.label;
        Some((
            label,
            resolve(
                pending.destination,
                Err("Подтверждение отменено".into()),
                false,
            ),
        ))
    }

    fn take_exact(&self, label: &str, approval_id: &str) -> Result<Pending, String> {
        let mut slot = self
            .pending
            .lock()
            .map_err(|_| "Подтверждение недоступно")?;
        let pending = slot.as_ref().ok_or("Нет ожидающего подтверждения")?;
        if pending.label != label {
            return Err("Это окно не может ответить на подтверждение".into());
        }
        if pending.request.approval_id != approval_id {
            return Err("Подтверждение уже сменилось".into());
        }
        slot.take()
            .ok_or_else(|| "Нет ожидающего подтверждения".into())
    }

    fn take_label(&self, label: &str) -> Option<Pending> {
        let mut slot = self.pending.lock().ok()?;
        if slot.as_ref()?.label != label {
            return None;
        }
        slot.take()
    }
}

fn resolve(
    destination: Destination,
    decision: Result<HostApprovalDecision, String>,
    allowed: bool,
) -> Resolution {
    match destination {
        Destination::Preflight(reply) => Resolution::Preflight { reply, allowed },
        Destination::Engine { generation, id } => Resolution::Engine {
            generation,
            id,
            decision,
        },
    }
}

fn validate_time(request: &HostApprovalRequest, now: u64) -> Result<(), String> {
    if now >= request.expires_at {
        return Err("Подтверждение истекло".into());
    }
    if request.expires_at - now > MAX_LIFETIME_MS {
        return Err("Срок подтверждения слишком велик".into());
    }
    Ok(())
}

fn redacted_action(method: &str, params: &Value) -> Result<Value, String> {
    let params = if method == "hoh/model/key/set" {
        let api_key = params.get("apiKey").ok_or("Нет ключа модели")?;
        let base_url = params
            .get("baseUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("Нет адреса модели")?;
        json!({
            "keyHash": hoh_protocol::approval::action_hash(&json!({"apiKey": api_key})),
            "baseUrl": base_url,
        })
    } else {
        params.clone()
    };
    Ok(json!({"method": method, "params": params}))
}

fn preflight_text(method: &str, params: &Value) -> Result<String, String> {
    match method {
        "session/working-directory/set" => {
            let request: hoh_protocol::SessionWorkingDirectorySetParams =
                serde_json::from_value(params.clone()).map_err(|_| "Некорректная рабочая папка")?;
            Ok(format!(
                "Разрешить работу в папке {}?",
                quoted(&request.path)
            ))
        }
        "hoh/settings/set" => {
            let request: hoh_protocol::SettingsSetParams =
                serde_json::from_value(params.clone()).map_err(|_| "Некорректные настройки")?;
            let mut changes = Vec::new();
            if let Some(model) = request.model {
                changes.push(format!(
                    "модель — {}, сервис — {}",
                    quoted(&model.model),
                    quoted(&model.base_url)
                ));
            }
            if let Some(security) = request.security {
                changes.push(format!("права: {}", permissions_text(&security)));
            }
            if let Some(view) = request.view {
                changes.push(format!(
                    "вид — {}, размер текста — {}",
                    theme_text(&view.theme),
                    view.font_size
                ));
            }
            if changes.is_empty() {
                return Err("Настройки не содержат изменений".into());
            }
            Ok(format!("Изменить настройки: {}?", changes.join("; ")))
        }
        "hoh/employee/create" => {
            let request: hoh_protocol::EmployeeCreateParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректный новый сотрудник")?;
            Ok(format!(
                "Создать сотрудника {} с ролью {}; правила — {}; назначенные задачи — {}; права: {}?",
                quoted(&request.draft.name),
                quoted(&request.draft.role),
                quoted_list(&request.draft.rules),
                quoted_list(&request.draft.task_ids),
                permissions_text(&request.draft.permissions)
            ))
        }
        "hoh/employee/update" => {
            let request: hoh_protocol::EmployeeUpdateParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректное изменение сотрудника")?;
            let mut changes = Vec::new();
            if let Some(name) = request.patch.name {
                changes.push(format!("имя — {}", quoted(&name)));
            }
            if let Some(role) = request.patch.role {
                changes.push(format!("роль — {}", quoted(&role)));
            }
            if let Some(rules) = request.patch.rules {
                changes.push(format!("правила — {}", quoted_list(&rules)));
            }
            if let Some(task_ids) = request.patch.task_ids {
                changes.push(format!("назначенные задачи — {}", quoted_list(&task_ids)));
            }
            if let Some(permissions) = request.patch.permissions {
                changes.push(format!("права: {}", permissions_text(&permissions)));
            }
            if changes.is_empty() {
                return Err("Изменение сотрудника не содержит новых прав или задач".into());
            }
            Ok(format!(
                "Изменить сотрудника {}: {}?",
                quoted(&request.id),
                changes.join("; ")
            ))
        }
        "hoh/task/draft/decide" => {
            let request: hoh_protocol::TaskDraftDecideParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректное решение по черновику")?;
            Ok(format!(
                "Создать задачу из черновика {}?",
                quoted(&request.draft_id)
            ))
        }
        "hoh/task/rollback" => {
            let request: hoh_protocol::TaskRollbackParams =
                serde_json::from_value(params.clone()).map_err(|_| "Некорректный откат задачи")?;
            Ok(format!(
                "Откатить задачу {} к версии {}?",
                quoted(&request.id),
                request.version
            ))
        }
        "hoh/settings/rules/set" => {
            let rules: hoh_protocol::SettingsRulesSetParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректные правила доступа")?;
            let internet = match rules.internet {
                hoh_protocol::SetupInternetMode::Ask => "с запросом разрешения",
                hoh_protocol::SetupInternetMode::Public => "к общедоступным сайтам",
            };
            Ok(format!(
                "Разрешить работу в папке {} и доступ в интернет {internet}?",
                quoted(&rules.folder)
            ))
        }
        // The user's own choice of who answers and how freely: the text names the executor (review 232500, item 1;
        // the same words as hoh-user-approval/src/method.rs).
        "hoh/executor/access/set" => {
            let name = executor_name(
                params["executor"]
                    .as_str()
                    .ok_or("Некорректный исполнитель")?,
            );
            match params["mode"].as_str() {
                Some("full") => Ok(format!(
                    "Включить полный доступ для {name}? Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить."
                )),
                Some("confirm") => Ok(format!("Вернуть режим «Спрашивать разрешение» для {name}?")),
                _ => Err("Некорректный режим доступа".into()),
            }
        }
        "hoh/executor/select" => {
            let model = params["model"].as_str().ok_or("Некорректная модель")?;
            match params["executor"].as_str() {
                Some(executor) => Ok(format!(
                    "Отвечать будет {} (модель {model})?",
                    executor_name(executor)
                )),
                None => Ok("Отвечать будет модель на этом компьютере?".to_owned()),
            }
        }
        "hoh/model/key/set" => {
            let address = params["baseUrl"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("Нет адреса сервиса модели")?;
            Ok(format!(
                "Сохранить ключ доступа к сервису {}?",
                quoted(address)
            ))
        }
        "hoh/model/probe" => {
            let probe: hoh_protocol::ModelProbeParams = serde_json::from_value(params.clone())
                .map_err(|_| "Некорректная проверка модели")?;
            Ok(format!(
                "Разрешить проверку модели {} у сервиса {}?",
                quoted(&probe.model),
                quoted(&probe.base_url)
            ))
        }
        _ => Err("Нет безопасного описания привилегированного действия".into()),
    }
}

/// An executor's name as a word in a sentence: `claude` → `Claude`.
fn executor_name(executor: &str) -> String {
    let mut chars = executor.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn quoted(value: &str) -> String {
    format!("«{value}»")
}

fn quoted_list(values: &[String]) -> String {
    if values.is_empty() {
        "нет".into()
    } else {
        values
            .iter()
            .map(|value| quoted(value))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn permissions_text(permissions: &hoh_protocol::Permissions) -> String {
    let internet = match &permissions.internet {
        hoh_protocol::InternetMode::Ask => "с запросом разрешения",
        hoh_protocol::InternetMode::Public => "общедоступные сайты",
        hoh_protocol::InternetMode::Deny => "запрещён",
    };
    format!(
        "интернет — {internet}; сайты — {}; папки — {}; изменение файлов — {}; запуск программ — {}; сеть для программ — {}; действия от вашего имени — {}",
        quoted_list(&permissions.domains),
        quoted_list(&permissions.folders),
        allowed(permissions.write_files),
        allowed(permissions.programs),
        quoted_list(&permissions.program_network),
        allowed(permissions.act_as_user)
    )
}

fn allowed(value: bool) -> &'static str {
    if value {
        "разрешено"
    } else {
        "запрещено"
    }
}

fn theme_text(theme: &hoh_protocol::Theme) -> &'static str {
    match theme {
        hoh_protocol::Theme::Light => "светлая тема",
        hoh_protocol::Theme::Dark => "тёмная тема",
        hoh_protocol::Theme::System => "как в системе",
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, now: u64) -> Value {
        let action = json!({"method":"hoh/model/probe","params":{"baseUrl":"https://example.test","model":"m"}});
        json!({
            "sessionId":"s", "toolName":"hoh/model/probe", "approvalId":id,
            "risk":"high", "humanText":"Разрешить проверку модели?",
            "actionHash":hoh_protocol::approval::action_hash(&action),
            "expiresAt":now+5_000, "action":action
        })
    }

    #[test]
    fn bad_mac_is_rejected_and_consumes_the_request() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        let decision = HostApprovalDecision {
            approval_id: "a".into(),
            outcome: ApprovalOutcome::AllowedOnce,
            mac: "00".repeat(32),
        };
        let result = broker
            .submit_signed(&open.label, "a", decision, now)
            .unwrap();
        assert!(result.command_result.is_err());
        assert!(broker.current(&open.label, now, Instant::now()).is_err());
    }

    #[test]
    fn main_window_cannot_read_or_decide() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(broker.current("main", now, Instant::now()).is_err());
        assert!(
            broker
                .decide("main", "a", ApprovalOutcome::Rejected, now, Instant::now(),)
                .is_err()
        );
        assert!(broker.current(&open.label, now, Instant::now()).is_ok());
    }

    #[test]
    fn stale_window_cannot_decide_reused_id_after_restart() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let old = broker
            .begin_engine(1, json!(1), "approval/request", request("same", now), now)
            .unwrap();
        assert!(broker.reject(&old.label, now).is_some());
        let new = broker
            .begin_engine(2, json!(2), "approval/request", request("same", now), now)
            .unwrap();
        assert_ne!(old.label, new.label);
        assert!(
            broker
                .decide(
                    &old.label,
                    "same",
                    ApprovalOutcome::Rejected,
                    now,
                    Instant::now(),
                )
                .is_err()
        );
        assert!(broker.current(&new.label, now, Instant::now()).is_ok());
    }

    #[test]
    fn allow_delay_starts_at_first_current_and_is_not_reset() {
        let now = 10_000;
        let shown = Instant::now();
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::AllowedOnce,
                    now,
                    shown + Duration::from_secs(10),
                )
                .is_err()
        );
        broker.current(&open.label, now, shown).unwrap();
        broker
            .current(&open.label, now, shown + Duration::from_millis(500))
            .unwrap();
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::AllowedOnce,
                    now,
                    shown + ALLOW_DELAY - Duration::from_millis(1),
                )
                .is_err()
        );
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::AllowedOnce,
                    now,
                    shown + ALLOW_DELAY,
                )
                .unwrap()
                .command_result
                .is_ok()
        );
    }

    #[test]
    fn rejection_is_immediate_before_current() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::Rejected,
                    now,
                    Instant::now(),
                )
                .unwrap()
                .command_result
                .is_ok()
        );
    }

    #[test]
    fn second_engine_request_is_rejected_without_replacing_the_first() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let first = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(
            broker
                .begin_engine(1, json!(2), "approval/request", request("b", now), now)
                .is_err()
        );
        assert!(broker.current(&first.label, now, Instant::now()).is_ok());
    }

    #[test]
    fn model_key_preflight_view_contains_no_secret_or_signing_fields() {
        let broker = Broker::new([0x42; 32]).unwrap();
        let (open, _receiver) = broker
            .begin_preflight(
                "hoh/model/key/set",
                &json!({
                    "apiKey":"private-test-key",
                    "baseUrl":"https://api.example.test/v1"
                }),
                10_000,
            )
            .unwrap();
        let encoded = serde_json::to_string(&open.view).unwrap();
        assert!(!encoded.contains("private-test-key"));
        for forbidden in ["apiKey", "keyHash", "action", "mac"] {
            assert!(!encoded.contains(forbidden), "view leaked {forbidden}");
        }
        let stored = broker.pending.lock().unwrap();
        let action = serde_json::to_string(&stored.as_ref().unwrap().request.action).unwrap();
        assert!(!action.contains("private-test-key"));
        assert!(!action.contains("apiKey"));
        assert!(action.contains("keyHash"));
        assert!(action.contains("https://api.example.test/v1"));
    }

    fn permissions() -> Value {
        json!({
            "internet":"ask", "domains":["example.test"], "folders":["/work"],
            "writeFiles":true, "programs":true, "programNetwork":["example.test"],
            "actAsUser":false
        })
    }

    #[test]
    fn every_privileged_method_has_a_concrete_parameterized_text() {
        let samples = vec![
            (
                "hoh/employee/create",
                json!({"draft":{"name":"Алиса","role":"аналитик","rules":["rule-a"],"taskIds":["task-7"],"permissions":permissions()}}),
                "Алиса",
            ),
            (
                "hoh/employee/update",
                json!({"id":"employee-7","patch":{"taskIds":["task-9"],"permissions":permissions()},"rev":1}),
                "employee-7",
            ),
            (
                "hoh/task/draft/decide",
                json!({"draftId":"draft-3","decision":"yes"}),
                "draft-3",
            ),
            (
                "hoh/task/rollback",
                json!({"id":"task-4","version":7}),
                "task-4",
            ),
            (
                "hoh/settings/set",
                json!({"model":{"baseUrl":"https://model.example.test","model":"model-x"}}),
                "model-x",
            ),
            (
                "session/working-directory/set",
                json!({"sessionId":"session-1","path":"/work/project"}),
                "/work/project",
            ),
            (
                "hoh/model/probe",
                json!({"baseUrl":"https://probe.example.test","model":"probe-model"}),
                "probe-model",
            ),
            (
                "hoh/model/key/set",
                json!({"apiKey":"never-show-this","baseUrl":"https://key.example.test"}),
                "https://key.example.test",
            ),
            (
                "hoh/settings/rules/set",
                json!({"internet":"public","folder":"/rules/work"}),
                "/rules/work",
            ),
            (
                "hoh/executor/access/set",
                json!({"executor":"claude","mode":"full"}),
                "Включить полный доступ для Claude?",
            ),
            (
                "hoh/executor/select",
                json!({"executor":"claude","model":"opus-sample"}),
                "Claude (модель opus-sample)",
            ),
        ];
        assert_eq!(samples.len(), hoh_protocol::PRIVILEGED_METHODS.len());
        let broker = Broker::new([0x42; 32]).unwrap();
        for (method, params, key) in samples {
            assert!(hoh_protocol::PRIVILEGED_METHODS.contains(&method));
            let (open, _receiver) = broker.begin_preflight(method, &params, 10_000).unwrap();
            assert!(
                open.view.human_text.contains(key),
                "{method}: {}",
                open.view.human_text
            );
            assert_ne!(
                open.view.human_text,
                format!("Разрешить действие {method}?")
            );
            assert!(!open.view.human_text.contains("never-show-this"));
            assert!(broker.reject(&open.label, 10_000).is_some());
        }
    }

    #[test]
    fn executor_choices_are_asked_in_words_that_name_the_executor_and_the_right() {
        let text = |method, params: Value| preflight_text(method, &params).unwrap();
        let full = text(
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"full"}),
        );
        let confirm = text(
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"confirm"}),
        );
        assert_ne!(full, confirm, "full and confirm are asked differently");
        assert!(
            full.starts_with("Включить полный доступ для Claude?"),
            "{full}"
        );
        assert!(
            full.contains("Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить."),
            "{full}"
        );
        assert_eq!(confirm, "Вернуть режим «Спрашивать разрешение» для Claude?");
        assert_eq!(
            text(
                "hoh/executor/select",
                json!({"executor":"claude","model":"opus"})
            ),
            "Отвечать будет Claude (модель opus)?"
        );
        assert_eq!(
            text(
                "hoh/executor/select",
                json!({"executor":null,"model":"default"})
            ),
            "Отвечать будет модель на этом компьютере?"
        );
        for method in ["hoh/executor/access/set", "hoh/executor/select"] {
            let generic = format!("Разрешить действие {method}?");
            assert_ne!(
                text(
                    method,
                    json!({"executor":"claude","mode":"full","model":"m"})
                ),
                generic
            );
        }
        assert!(
            preflight_text(
                "hoh/executor/access/set",
                &json!({"executor":"claude","mode":"root"})
            )
            .is_err()
        );
    }
}
