//! Tauri command adapters. Security decisions live in Rust, never in the page.
use crate::{
    approval::{ApprovalView, Begun, Broker, OpenApproval, Resolution, now_ms},
    bridge,
    engine::{Engine, HostEvent, ProcessConfig},
    questions::QuestionBroker,
    workspace,
};
use hoh_protocol::ApprovalOutcome;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
#[cfg(target_os = "linux")]
use webkit2gtk::WebViewExt;

/// Where the session's folder comes from and is remembered.
#[derive(Clone)]
struct Workspace {
    default: PathBuf,
    config_dir: PathBuf,
}

pub struct Host {
    pub engine: Arc<Engine>,
    pub approvals: Arc<Broker>,
    pub questions: Arc<QuestionBroker>,
    workspace: Workspace,
    ready: Arc<AtomicBool>,
}

/// How the native host starts the engine. The real `hoh` needs the window profile and its own
/// data folder; the replay stub (tests only) takes no arguments.
#[derive(Debug, PartialEq, Eq)]
struct EngineLaunch {
    path: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    /// Provider and model announced in `initialize`; the real engine registers its OpenAI-compatible
    /// provider under this id and takes the model name from its settings afterwards.
    provider: &'static str,
    model: &'static str,
}

fn select_engine_launch(
    test_engine: Option<OsString>,
    configured_engine: Option<OsString>,
    engine_home: &std::path::Path,
) -> Result<EngineLaunch, String> {
    let is_test = test_engine.is_some();
    let value = test_engine
        .or(configured_engine)
        .ok_or_else(|| "Путь к движку не задан при сборке. Переустановите приложение".to_owned())?;
    if value.is_empty() {
        return Err("Путь к движку пуст. Переустановите приложение".into());
    }
    if is_test {
        return Ok(EngineLaunch {
            path: PathBuf::from(value),
            args: Vec::new(),
            env: Vec::new(),
            provider: "mock",
            model: "mock",
        });
    }
    Ok(EngineLaunch {
        path: PathBuf::from(value),
        args: vec!["--profile".into(), "window".into()],
        env: vec![(
            "DSH_HOME".into(),
            engine_home
                .to_str()
                .ok_or("Папка данных должна быть в UTF-8")?
                .to_owned(),
        )],
        provider: "openai-compatible",
        model: "local",
    })
}

/// The engine's data folder (DSH_HOME) must be owner-only: the engine refuses to store the model
/// key in a folder others can read, so the launcher creates it.
fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

fn engine_launch(engine_home: &std::path::Path) -> Result<EngineLaunch, String> {
    let configured = std::env::var_os("HOH_ENGINE_PATH")
        .or_else(|| option_env!("HOH_ENGINE_PATH").map(OsString::from));
    select_engine_launch(std::env::var_os("HOH_TEST_ENGINE"), configured, engine_home)
}

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "probe")]
    crate::probe::trace("native:setup:start");
    let mut key = [0_u8; 32];
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(&mut key)?;
    let approvals = Arc::new(
        Broker::new(key)?.with_audit_file(app.path().app_data_dir()?.join("approvals.log")),
    );
    let questions = Arc::new(QuestionBroker::default());
    let ready = Arc::new(AtomicBool::new(false));
    let engine_home = app.path().app_data_dir()?.join("engine");
    create_private_dir(&engine_home)?;
    let launch = engine_launch(&engine_home)?;
    let config_dir = app.path().app_config_dir()?;
    // Without XDG user dirs (a fresh account, a container) there is no Documents entry: use ~/Documents.
    let documents = match app.path().document_dir() {
        Ok(path) => path,
        Err(_) => app.path().home_dir()?.join("Documents"),
    };
    let default_folder = workspace::default_folder(&documents);
    // The sandbox of the session is rooted in this folder: the one chosen on the first run, else
    // the app's own folder in Documents. Never the home folder.
    let session_folder = workspace::startup_folder(&config_dir, &default_folder)?;
    let path = config_dir.join("engine.json");
    let mut config = if path.exists() {
        serde_json::from_slice::<ProcessConfig>(&std::fs::read(path)?)?
    } else {
        ProcessConfig {
            path: launch.path.clone(),
            args: Vec::new(),
            env: Vec::new(),
            stderr_file: None,
            cwd: engine_home.clone(),
            provider: launch.provider.into(),
            model: launch.model.into(),
        }
    };
    // A writable settings file must never redirect the native host to another
    // executable or change how it is started. Production receives the path from
    // its build/runtime variable; tests can opt into the replay stub only through
    // HOH_TEST_ENGINE.
    config.path = launch.path;
    config.args = launch.args;
    config.env = launch.env;
    config.stderr_file = None;
    config.provider = launch.provider.into();
    config.model = launch.model.into();
    // The engine process lives in its private folder; only the session is rooted in the work folder.
    config.cwd = engine_home.clone();
    let handle = app.handle().clone();
    let event_approvals = approvals.clone();
    let event_questions = questions.clone();
    let event_ready = ready.clone();
    let init = json!({"cwd":session_folder,"provider":config.provider,"model":config.model,"capabilities":{"approvals":"host","userQuestions":true},"hoh":{"protocolVersion":1,"approvalKey":hoh_protocol::approval::hex(&key)}});
    let engine = Engine::start(
        Arc::new(config),
        init,
        Arc::new(move |event| {
            handle_event(
                &handle,
                &event_approvals,
                &event_questions,
                &event_ready,
                event,
            )
        }),
    );
    app.manage(Host {
        engine,
        approvals,
        questions,
        workspace: Workspace {
            default: default_folder,
            config_dir,
        },
        ready,
    });
    #[cfg(feature = "probe")]
    crate::probe::trace("native:setup:complete");
    Ok(())
}

fn handle_event(
    handle: &tauri::AppHandle,
    approvals: &Arc<Broker>,
    questions: &Arc<QuestionBroker>,
    ready: &Arc<AtomicBool>,
    event: HostEvent,
) {
    match event {
        HostEvent::Notification(frame) => {
            if frame["method"] == "user-question.cancelled"
                && let Some(id) = frame["params"]["requestId"].as_str()
            {
                questions.cancel(id);
            }
            let mut frame = frame;
            if frame["method"] == "approval.pending"
                && let Some(text) = frame["params"]["humanText"].as_str()
            {
                // The window is told only that a decision waits; an address in it stays closed.
                frame["params"]["humanText"] = Value::String(crate::approval::redact_text(text));
                for key in ["reason", "details"] {
                    if let Some(text) = frame["params"][key].as_str() {
                        frame["params"][key] = Value::String(crate::approval::redact_text(text));
                    }
                }
            }
            let _ = handle.emit_to("main", "engine-notification", frame);
        }
        HostEvent::Status(status) => {
            let is_ready = status["state"] == "ready";
            #[cfg(feature = "probe")]
            crate::probe::trace(if is_ready {
                "engine:status:ready"
            } else {
                "engine:status:not-ready"
            });
            ready.store(is_ready, Ordering::Release);
            if !is_ready {
                questions.clear();
            }
            if let Some((label, resolution)) = cancel_for_status(approvals, is_ready) {
                dispatch_resolution(handle, resolution);
                destroy_window(handle, &label);
            }
            if let Some(window) = handle.get_webview_window("main") {
                let title = if is_ready {
                    "HoH — Помощник".to_owned()
                } else {
                    format!(
                        "HoH — {}",
                        status["message"].as_str().unwrap_or("Подключаю Помощника…")
                    )
                };
                let _ = window.set_title(&title);
            }
            let _ = handle.emit_to("main", "engine-status", status);
        }
        HostEvent::Request {
            generation,
            id,
            method,
            params,
        } if method == "user-question/ask" => {
            match questions.begin(generation, id.clone(), params) {
                Ok(question) => {
                    let _ = handle.emit_to("main", "engine-question", question);
                }
                Err(_) => respond_error(handle, generation, id, "Вопрос недоступен"),
            }
        }
        HostEvent::Request {
            generation,
            id,
            method,
            params,
        } => {
            match approvals.begin_engine_or_grant(generation, id.clone(), &method, params, now_ms())
            {
                Ok(Begun::Window(open)) => schedule_window(handle, approvals, open),
                // Confirmed by the person a moment ago in the settings window: no second window.
                Ok(Begun::Signed(resolution)) => dispatch_resolution(handle, resolution),
                Err(_) => respond_error(handle, generation, id, "Подтверждение недоступно"),
            }
        }
    }
}

fn schedule_window(handle: &tauri::AppHandle, approvals: &Arc<Broker>, open: OpenApproval) {
    #[cfg(feature = "probe")]
    crate::probe::trace(format!("approval:{}:schedule", open.label));
    let app = handle.clone();
    let failure_app = handle.clone();
    let broker = approvals.clone();
    let failure_broker = approvals.clone();
    let failure_label = open.label.clone();
    let run_label = failure_label.clone();
    if handle
        .run_on_main_thread(move || {
            #[cfg(feature = "probe")]
            crate::probe::trace(format!("approval:{run_label}:main-thread:start"));
            if let Err(_error) = open_window(&app, &broker, open) {
                #[cfg(feature = "probe")]
                crate::probe::trace(format!("approval:{run_label}:create:error:{_error}"));
                if let Some(resolution) = broker.reject(&run_label, now_ms()) {
                    dispatch_resolution(&app, resolution);
                }
            }
        })
        .is_err()
        && let Some(resolution) = failure_broker.reject(&failure_label, now_ms())
    {
        #[cfg(feature = "probe")]
        crate::probe::trace(format!("approval:{failure_label}:schedule:error"));
        dispatch_resolution(&failure_app, resolution);
    }
}

fn open_window(
    handle: &tauri::AppHandle,
    approvals: &Arc<Broker>,
    open: OpenApproval,
) -> tauri::Result<()> {
    let parent = handle
        .get_webview_window("main")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let label = open.label.clone();
    let builder = tauri::WebviewWindowBuilder::new(
        handle,
        &label,
        tauri::WebviewUrl::App("approval.html".into()),
    )
    .title("HoH — Подтверждение")
    .inner_size(640.0, 620.0)
    .min_inner_size(520.0, 520.0)
    .resizable(true)
    .focused(true)
    .parent(&parent)?
    .on_navigation(|url| {
        let allowed = approval_navigation_allowed(url);
        #[cfg(feature = "probe")]
        trace_navigation("approval", url, allowed);
        allowed
    })
    .on_new_window(|_, _| deny_new_window());
    #[cfg(feature = "probe")]
    let builder = builder
        .on_page_load(move |window, payload| {
            crate::probe::trace(format!(
                "approval:{}:page-load:{:?}",
                window.label(),
                payload.event()
            ));
            if is_approval_probe()
                && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
            {
                let _ = window.eval(
                    r#"(() => {
                        const ready = () => {
                            const page = document.querySelector('[data-screen="approval"][data-phase="pending"]');
                            const reject = document.querySelector('button.ap-safe');
                            if (page && reject) {
                                setTimeout(() => { document.title = 'HOH_APPROVAL_READY'; }, 100);
                            } else {
                                setTimeout(ready, 25);
                            }
                        };
                        ready();
                    })();"#,
                );
            }
        })
        .on_document_title_changed(|window, title| {
            crate::probe::trace(format!("approval:{}:title:changed", window.label()));
            if is_approval_probe() && title == "HOH_APPROVAL_READY" {
                crate::probe::trace(format!("approval:{}:auto-close", window.label()));
                crate::probe::mark_approval_window_closed();
                let _ = window.close();
            }
        });
    let window = builder.build()?;
    #[cfg(feature = "probe")]
    crate::probe::trace(format!("approval:{label}:created"));

    let close_app = handle.clone();
    let close_broker = approvals.clone();
    let close_label = label.clone();
    window.on_window_event(move |event| {
        #[cfg(feature = "probe")]
        if matches!(
            event,
            tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
        ) {
            crate::probe::trace(format!(
                "approval:{close_label}:window-event:{}",
                if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                    "CloseRequested"
                } else {
                    "Destroyed"
                }
            ));
        }
        if let Some(resolution) =
            approval_window_resolution(&close_broker, &close_label, event, now_ms())
        {
            dispatch_resolution(&close_app, resolution);
        }
    });

    let expiry_app = handle.clone();
    let expiry_broker = approvals.clone();
    let expires_at = open.view.expires_at;
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(
            expires_at
                .saturating_sub(now_ms())
                .saturating_sub(crate::approval::EXPIRY_MARGIN_MS)
                .min(300_000),
        ));
        if let Some(resolution) = expire_approval(&expiry_broker, &label, now_ms()) {
            dispatch_resolution(&expiry_app, resolution);
            destroy_window(&expiry_app, &label);
        }
    });
    Ok(())
}

fn trusted_app_origin(url: &tauri::Url) -> bool {
    let trusted = url.scheme() == "tauri" && url.host_str() == Some("localhost");
    #[cfg(any(windows, target_os = "android"))]
    let trusted = trusted || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"));
    url.port().is_none() && trusted
}

pub fn approval_navigation_allowed(url: &tauri::Url) -> bool {
    trusted_app_origin(url) && url.path() == "/approval.html"
}

pub fn main_navigation_allowed(url: &tauri::Url) -> bool {
    if !trusted_app_origin(url) {
        return false;
    }
    match url.path() {
        // Tauri canonicalizes `WebviewUrl::App("index.html")` to its bare
        // application origin before invoking navigation hooks.
        "" | "/" | "/index.html" => true,
        #[cfg(feature = "probe")]
        "/probe.html" => true,
        _ => false,
    }
}

pub fn main_security_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let builder = tauri::plugin::Builder::new("main-security").on_navigation(|webview, url| {
        let allowed = webview.label() != "main" || main_navigation_allowed(url);
        #[cfg(feature = "probe")]
        if webview.label() == "main" {
            trace_navigation("main", url, allowed);
        }
        allowed
    });
    #[cfg(feature = "probe")]
    let builder = builder
        .on_page_load(|webview, payload| {
            if webview.label() == "main" {
                crate::probe::trace(format!(
                    "main:page-load:{:?}:{}",
                    payload.event(),
                    payload.url().path()
                ));
            }
        })
        .on_window_ready(|window| {
            if window.label() == "main" {
                crate::probe::trace("main:window-ready");
            }
        });
    #[cfg(target_os = "linux")]
    let builder = builder.on_webview_ready(|webview| {
        if webview.label() == "main" {
            let _ = webview.with_webview(|platform| {
                platform.inner().connect_create(|_, _| None);
            });
        }
    });
    builder.build()
}

#[cfg(feature = "probe")]
fn trace_navigation(kind: &str, url: &tauri::Url, allowed: bool) {
    crate::probe::trace(format!(
        "{kind}:navigation:{}:{}://{}{}",
        if allowed { "allowed" } else { "denied" },
        url.scheme(),
        url.host_str().unwrap_or(""),
        url.path()
    ));
}

pub fn deny_new_window<R: tauri::Runtime>() -> tauri::webview::NewWindowResponse<R> {
    tauri::webview::NewWindowResponse::Deny
}

fn approval_window_resolution(
    approvals: &Broker,
    label: &str,
    event: &tauri::WindowEvent,
    now: u64,
) -> Option<Resolution> {
    if matches!(
        event,
        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
    ) {
        approvals.reject(label, now)
    } else {
        None
    }
}

fn cancel_for_status(approvals: &Broker, is_ready: bool) -> Option<(String, Resolution)> {
    if is_ready { None } else { approvals.cancel() }
}

fn expire_approval(approvals: &Broker, label: &str, now: u64) -> Option<Resolution> {
    approvals.expire(label, now)
}

#[cfg(feature = "probe")]
fn is_approval_probe() -> bool {
    crate::probe::approval_requested()
}

fn dispatch_resolution(handle: &tauri::AppHandle, resolution: Resolution) {
    match resolution {
        Resolution::Preflight { reply, allowed } => {
            let _ = reply.send(allowed);
        }
        Resolution::Engine {
            generation,
            id,
            decision,
        } => {
            if let Some(state) = handle.try_state::<Host>() {
                let frame = match decision {
                    Ok(decision) => hoh_protocol::framing::success(
                        id,
                        serde_json::to_value(decision).expect("approval decision serializes"),
                    ),
                    Err(_) => {
                        hoh_protocol::framing::error(id, -32000, "Native approval unavailable")
                    }
                };
                let _ = state.engine.respond(generation, frame);
            }
        }
    }
}

fn respond_error(handle: &tauri::AppHandle, generation: u64, id: Value, message: &str) {
    if let Some(state) = handle.try_state::<Host>() {
        let _ = state.engine.respond(
            generation,
            hoh_protocol::framing::error(id, -32000, message),
        );
    }
}

fn destroy_window(handle: &tauri::AppHandle, label: &str) {
    if let Some(window) = handle.get_webview_window(label) {
        let _ = window.destroy();
    }
}

#[tauri::command]
pub async fn engine_request(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    #[cfg(feature = "probe")]
    crate::probe::trace(format!("invoke:engine_request:received:{method}"));
    let label = window.label().to_owned();
    let engine = state.engine.clone();
    let approvals = state.approvals.clone();
    let ready = state.ready.clone();
    let workspace = state.workspace.clone();
    let handle = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut params = params;
        let chosen_folder = if method == "hoh/settings/rules/set" && label == "main" {
            Some(normalize_rules_folder(&mut params, &workspace)?)
        } else {
            None
        };
        // The confirmation is bound to this outgoing request: armed as it leaves, dropped as soon as
        // it is answered, fails or times out.
        let sent = Arc::new(std::sync::Mutex::new(None::<(u64, String)>));
        let arm = {
            let (approvals, sent, method, params) = (
                approvals.clone(),
                sent.clone(),
                method.clone(),
                params.clone(),
            );
            Box::new(move |generation: u64, id: &str| {
                approvals.arm_in_flight(&method, &params, generation, id, now_ms());
                *sent.lock().unwrap_or_else(|p| p.into_inner()) = Some((generation, id.to_owned()));
            })
        };
        let answer = bridge::request_with_approval_armed(
            &engine,
            &label,
            &method,
            params,
            |params| {
                #[cfg(feature = "probe")]
                crate::probe::trace(format!("preflight:{method}:start"));
                if !ready.load(Ordering::Acquire) {
                    #[cfg(feature = "probe")]
                    crate::probe::trace(format!("preflight:{method}:engine-not-ready"));
                    return Err("Помощник ещё подключается".into());
                }
                let (open, receiver) = approvals.begin_preflight(&method, params, now_ms())?;
                let approval_label = open.label.clone();
                #[cfg(feature = "probe")]
                crate::probe::trace(format!("preflight:{method}:begun:{approval_label}"));
                schedule_window(&handle, &approvals, open);
                match receiver.recv_timeout(Duration::from_secs(305)) {
                    Ok(true) => {
                        #[cfg(feature = "probe")]
                        crate::probe::trace(format!("preflight:{method}:complete:allowed"));
                        Ok(())
                    }
                    Ok(false) => {
                        #[cfg(feature = "probe")]
                        crate::probe::trace(format!("preflight:{method}:complete:rejected"));
                        Err("Действие отклонено".into())
                    }
                    Err(_) => {
                        #[cfg(feature = "probe")]
                        crate::probe::trace(format!("preflight:{method}:complete:timeout"));
                        if let Some(resolution) = approvals.reject(&approval_label, now_ms()) {
                            dispatch_resolution(&handle, resolution);
                            destroy_window(&handle, &approval_label);
                        }
                        Err("Подтверждение не получено".into())
                    }
                }
            },
            Some(arm),
        );
        if let Some((generation, id)) = sent.lock().unwrap_or_else(|p| p.into_inner()).take() {
            approvals.disarm(generation, &id);
        }
        let answer = answer?;
        if let Some(folder) = chosen_folder {
            // The rules were allowed: remember the folder and reopen the session in it, so the
            // sandbox is rooted there and only leaving it needs a question.
            workspace::save(&workspace.config_dir, &folder)
                .map_err(|_| "Не удалось запомнить папку".to_owned())?;
            engine.restart_in(&folder)?;
        }
        Ok(answer)
    })
    .await
    .map_err(|_| "Запрос прерван".to_owned())?
}

/// The window sends a display name or a picked path; the engine and the person's confirmation
/// must see the real, existing folder.
fn normalize_rules_folder(params: &mut Value, workspace: &Workspace) -> Result<PathBuf, String> {
    let choice = params["folder"].as_str().unwrap_or_default();
    let folder = workspace::resolve_choice(choice, &workspace.default)?;
    if folder == workspace.default {
        std::fs::create_dir_all(&folder).map_err(|_| "Не удалось создать папку HoH".to_owned())?;
    } else if !folder.is_dir() {
        return Err("Папка не найдена".into());
    }
    params["folder"] = json!(folder);
    Ok(folder)
}

/// The person answered (or dismissed) a question of the assistant. Only the main window may.
#[tauri::command]
pub fn question_answer(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    request_id: String,
    answer: Value,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Ответ доступен только главному окну".into());
    }
    let (generation, frame) = state.questions.answer(&request_id, &answer)?;
    state
        .engine
        .respond(generation, frame)
        .map_err(|_| "Помощник недоступен".to_owned())
}

/// The decisions of the person, for the «Подтверждения» section. Only the main window may read them.
#[tauri::command]
pub fn approval_history(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
) -> Result<Vec<Value>, String> {
    if window.label() != "main" {
        return Err("История доступна только главному окну".into());
    }
    Ok(state.approvals.history(200))
}

#[tauri::command]
pub fn approval_current(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
) -> Result<ApprovalView, String> {
    #[cfg(feature = "probe")]
    crate::probe::trace(format!("approval:{}:current", window.label()));
    state
        .approvals
        .current(window.label(), now_ms(), Instant::now())
}

#[tauri::command]
pub fn approval_decide(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    approval_id: String,
    outcome: ApprovalOutcome,
) -> Result<(), String> {
    let label = window.label().to_owned();
    let decision =
        state
            .approvals
            .decide(&label, &approval_id, outcome, now_ms(), Instant::now())?;
    dispatch_resolution(window.app_handle(), decision.resolution);
    destroy_window(window.app_handle(), &label);
    decision.command_result
}

pub fn shutdown(handle: &tauri::AppHandle, host: &Host) {
    if let Some((label, resolution)) = host.approvals.cancel() {
        dispatch_resolution(handle, resolution);
        destroy_window(handle, &label);
    }
    host.engine.stop();
}

#[tauri::command]
pub async fn pick_folder(window: tauri::WebviewWindow) -> Result<Option<String>, String> {
    if window.label() != "main" {
        return Err("Выбор папки доступен главному окну".into());
    }
    let app = window.app_handle().clone();
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        #[cfg(target_os = "linux")]
        {
            use gtk::prelude::*;
            let result = window
                .gtk_window()
                .map_err(|_| "Нет родительского окна".to_owned())
                .and_then(|parent| {
                    let dialog = gtk::FileChooserNative::new(
                        Some("Папка для работы"),
                        Some(&parent),
                        gtk::FileChooserAction::SelectFolder,
                        Some("Выбрать"),
                        Some("Отмена"),
                    );
                    dialog.set_local_only(true);
                    let result = if dialog.run() == gtk::ResponseType::Accept {
                        dialog
                            .filename()
                            .map(|p| {
                                p.to_str()
                                    .map(str::to_owned)
                                    .ok_or_else(|| "Имя папки должно быть в UTF-8".to_owned())
                            })
                            .transpose()
                    } else {
                        Ok(None)
                    };
                    dialog.destroy();
                    result
                });
            let _ = tx.send(result);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = tx.send(Err(
                "Системный выбор папки ещё не подключён на этой платформе".to_owned(),
            ));
        }
    })
    .map_err(|_| "Не удалось открыть выбор папки".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv().map_err(|_| "Выбор папки прерван".to_owned())?
    })
    .await
    .map_err(|_| "Выбор папки прерван".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, sync::mpsc};

    fn begin(broker: &Broker, now: u64) -> (OpenApproval, mpsc::Receiver<bool>) {
        broker
            .begin_preflight(
                "hoh/model/probe",
                &json!({"baseUrl":"https://example.test","model":"m"}),
                now,
            )
            .unwrap()
    }

    fn deliver_preflight(resolution: Resolution) {
        match resolution {
            Resolution::Preflight { reply, allowed } => {
                assert!(!allowed);
                reply.send(allowed).unwrap();
            }
            Resolution::Engine { .. } => panic!("expected preflight resolution"),
        }
    }

    #[test]
    fn destroyed_window_rejects_waiter_and_frees_slot() {
        let broker = Broker::new([0x42; 32]).unwrap();
        let (open, receiver) = begin(&broker, 10_000);
        deliver_preflight(
            approval_window_resolution(
                &broker,
                &open.label,
                &tauri::WindowEvent::Destroyed,
                10_000,
            )
            .unwrap(),
        );
        assert!(!receiver.recv_timeout(Duration::from_millis(50)).unwrap());
        let _next = begin(&broker, 10_000);
    }

    #[test]
    fn non_ready_status_cancels_waiter_and_frees_slot() {
        let broker = Broker::new([0x42; 32]).unwrap();
        let (_open, receiver) = begin(&broker, 10_000);
        let (_label, resolution) = cancel_for_status(&broker, false).unwrap();
        deliver_preflight(resolution);
        assert!(!receiver.recv_timeout(Duration::from_millis(50)).unwrap());
        let _next = begin(&broker, 10_000);
    }

    #[test]
    fn expiry_rejects_waiter_and_frees_slot() {
        let broker = Broker::new([0x42; 32]).unwrap();
        let (open, receiver) = begin(&broker, 10_000);
        deliver_preflight(expire_approval(&broker, &open.label, 310_000).unwrap());
        assert!(!receiver.recv_timeout(Duration::from_millis(50)).unwrap());
        let _next = begin(&broker, 310_000);
    }

    #[test]
    fn engine_data_folder_is_owner_only_even_when_it_already_exists() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("hoh-desktop-private-{}", std::process::id()));
        let folder = base.join("engine");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&folder).unwrap();
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(&base).unwrap();
        create_private_dir(&folder).unwrap();
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn rules_folder_is_made_real_before_the_engine_and_the_person_see_it() {
        let base = std::env::temp_dir().join(format!("hoh-desktop-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let workspace = Workspace {
            default: base.join("Documents/HoH"),
            config_dir: base.join("config"),
        };
        let mut params = json!({"internet":"ask","folder":workspace::DEFAULT_LABEL});
        let folder = normalize_rules_folder(&mut params, &workspace).unwrap();
        assert_eq!(folder, workspace.default);
        assert!(folder.is_dir());
        assert_eq!(params["folder"], json!(folder));
        let picked = base.join("project");
        std::fs::create_dir_all(&picked).unwrap();
        let mut params = json!({"internet":"public","folder":picked});
        assert_eq!(
            normalize_rules_folder(&mut params, &workspace).unwrap(),
            picked
        );
        for refused in [
            json!("Документы/Другая"),
            json!("/нет/такой/папки"),
            json!(null),
        ] {
            let mut params = json!({"internet":"ask","folder":refused});
            assert!(normalize_rules_folder(&mut params, &workspace).is_err());
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn engine_path_requires_the_real_build_variable() {
        let home = std::path::Path::new("/data/engine");
        assert!(select_engine_launch(None, None, home).is_err());
        assert_eq!(
            select_engine_launch(None, Some(OsString::from("/opt/hoh/bin/hoh")), home).unwrap(),
            EngineLaunch {
                path: PathBuf::from("/opt/hoh/bin/hoh"),
                args: vec!["--profile".into(), "window".into()],
                env: vec![("DSH_HOME".into(), "/data/engine".into())],
                provider: "openai-compatible",
                model: "local",
            }
        );
    }

    #[test]
    fn test_engine_is_the_only_stub_override_and_gets_no_profile_arguments() {
        let home = std::path::Path::new("/data/engine");
        assert_eq!(
            select_engine_launch(
                Some(OsString::from("/tmp/hoh-window-stub")),
                Some(OsString::from("/opt/hoh/bin/hoh")),
                home,
            )
            .unwrap(),
            EngineLaunch {
                path: PathBuf::from("/tmp/hoh-window-stub"),
                args: Vec::new(),
                env: Vec::new(),
                provider: "mock",
                model: "mock",
            }
        );
        assert!(select_engine_launch(Some(OsString::new()), None, home).is_err());
    }
}
