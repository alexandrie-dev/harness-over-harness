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
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, ipc::Channel};

type EngineNotifications = Arc<Mutex<Option<Channel<Value>>>>;
#[cfg(target_os = "linux")]
use webkit2gtk::{URIRequestExt, WebViewExt};

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
    notifications: EngineNotifications,
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
    let value = test_engine.or(configured_engine).ok_or_else(|| {
        "Путь к Помощнику не задан при сборке. Переустановите приложение".to_owned()
    })?;
    if value.is_empty() {
        return Err("Путь к Помощнику пуст. Переустановите приложение".into());
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

/// The window icon in several sizes (`_NET_WM_ICON`): the task bar and the window switcher pick the one that fits.
#[cfg(target_os = "linux")]
fn set_window_icons(window: &tauri::WebviewWindow) {
    use gtk::gdk_pixbuf::PixbufLoader;
    use gtk::prelude::*;
    const ICONS: [&[u8]; 6] = [
        include_bytes!("../icons/32x32.png"),
        include_bytes!("../icons/48x48.png"),
        include_bytes!("../icons/64x64.png"),
        include_bytes!("../icons/128x128.png"),
        include_bytes!("../icons/256x256.png"),
        include_bytes!("../icons/512x512.png"),
    ];
    let pixbufs: Vec<_> = ICONS
        .iter()
        .filter_map(|bytes| {
            let loader = PixbufLoader::new();
            loader.write(bytes).ok()?;
            loader.close().ok()?;
            loader.pixbuf()
        })
        .collect();
    if let Ok(gtk_window) = window.gtk_window() {
        gtk_window.set_icon_list(&pixbufs);
    }
}

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "probe")]
    crate::probe::trace("native:setup:start");
    #[cfg(target_os = "linux")]
    if let Some(window) = app.get_webview_window("main") {
        set_window_icons(&window);
    }
    let legacy = legacy_shell_requested(std::env::var_os("HOH_WINDOW_SHELL").as_deref());
    if legacy && let Some(window) = app.get_webview_window("main") {
        let page = window.url()?.join("legacy.html")?;
        window.navigate(page)?;
    }
    // The T3 client answers the engine's permission requests in its conversation; the previous
    // shell keeps the separate approval window.
    let in_chat = !legacy;
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
    let default_folder = workspace::usable_default(&documents, &config_dir)?;
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
    let notifications = EngineNotifications::default();
    let event_notifications = notifications.clone();
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
                in_chat,
                &event_notifications,
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
        notifications,
    });
    #[cfg(feature = "probe")]
    crate::probe::trace("native:setup:complete");
    Ok(())
}

/// What the page gets of an engine notification. The text of an `approval.pending` request
/// (`humanText`, `reason`, `details`) goes through as the engine wrote it: a mask over the engine's
/// own text could hide a nested command from the person who is asked. The mask stays in the shell's
/// own settings text (`approval::begin_preflight`) and in the journal.
fn window_frame(frame: Value) -> Value {
    frame
}

fn handle_event(
    handle: &tauri::AppHandle,
    approvals: &Arc<Broker>,
    questions: &Arc<QuestionBroker>,
    ready: &Arc<AtomicBool>,
    in_chat: bool,
    notifications: &EngineNotifications,
    event: HostEvent,
) {
    match event {
        HostEvent::Notification(frame) => {
            if frame["method"] == "approval.resolved"
                && let Some(id) = frame["params"]["approvalId"].as_str()
            {
                approvals.withdraw_chat(id);
            }
            if frame["method"] == "user-question.cancelled"
                && let Some(id) = frame["params"]["requestId"].as_str()
            {
                questions.cancel(id);
            }
            let frame = window_frame(frame);
            let channel = notifications
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(channel) = channel {
                let _ = channel.send(frame);
            } else {
                let _ = handle.emit_to("main", "engine-notification", frame);
            }
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
            for (label, resolution) in cancel_for_status(approvals, is_ready) {
                dispatch_resolution(handle, resolution);
                destroy_window(handle, &label);
            }
            if let Some(window) = handle.get_webview_window("main") {
                // The window's name is HoH (founder 22.11 19:16); while the engine is not ready the title says why.
                let title = if is_ready {
                    "HoH".to_owned()
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
            let now = now_ms();
            let begun = if in_chat {
                approvals.begin_engine_in_chat(
                    generation,
                    id.clone(),
                    &method,
                    params,
                    now,
                    Instant::now(),
                )
            } else {
                approvals.begin_engine_or_grant(generation, id.clone(), &method, params, now)
            };
            match begun {
                Ok(Begun::Window(open)) if in_chat => {
                    schedule_expiry(handle, approvals, open.label, open.view.expires_at);
                }
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
    .title("HoH — Confirmation")
    .center()
    .inner_size(640.0, 620.0)
    .min_inner_size(520.0, 520.0)
    .resizable(true)
    .focused(true)
    // The protection window stays above the main window while a decision is waiting for the person.
    .always_on_top(true)
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

    schedule_expiry(handle, approvals, label, open.view.expires_at);
    Ok(())
}

/// The request not answered in time is cancelled with a valid signature just before its deadline.
fn schedule_expiry(
    handle: &tauri::AppHandle,
    approvals: &Arc<Broker>,
    label: String,
    expires_at: u64,
) {
    let expiry_app = handle.clone();
    let expiry_broker = approvals.clone();
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

/// What the main window may load: the T3 client and nothing else of the app (review 222000, CEO
/// 22.11 22:40). `legacy_shell`: the previous shell was asked for (HOH_WINDOW_SHELL=legacy), only
/// then `/legacy.html`. `is_app_file`: the app has this file (`/approval.html`).
pub fn main_navigation_allowed(
    url: &tauri::Url,
    legacy_shell: bool,
    is_app_file: impl Fn(&str) -> bool,
) -> bool {
    if !trusted_app_origin(url) {
        return false;
    }
    match url.path() {
        // Tauri canonicalizes `WebviewUrl::App("index.html")` to its bare
        // application origin before invoking navigation hooks.
        "" | "/" | "/index.html" => true,
        "/legacy.html" => legacy_shell,
        #[cfg(feature = "probe")]
        "/probe.html" => true,
        // A page of the T3 client's own router (/settings/general, /<env>/<thread>) when the window
        // reloads there. Tauri (`manager::get_asset`) decodes `%XX`, then serves the first file of
        // `{p}`, `{p}.html`, `{p}/index.html`, and only then index.html: the route is allowed only
        // when none of the three is a file, so `/approval` never opens approval.html.
        path => {
            is_app_route(path)
                && [
                    path.to_owned(),
                    format!("{path}.html"),
                    format!("{path}/index.html"),
                ]
                .iter()
                .all(|file| !is_app_file(file))
        }
    }
}

/// A route path of the window's router, not a file of the app: `/a/b-c/$d`, no dots, no empty
/// segments, no `%` (Tauri would decode it; no route of T3 needs it, so nothing is left to decode).
fn is_app_route(path: &str) -> bool {
    path.len() <= 512
        && path.strip_prefix('/').is_some_and(|rest| {
            !rest.is_empty()
                && rest.split('/').all(|segment| {
                    !segment.is_empty()
                        && segment
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'$'))
                })
        })
}

/// The window opens the T3 client (dist/index.html); `HOH_WINDOW_SHELL=legacy` opens the previous shell
/// (dist/legacy.html) instead, until that shell is removed.
pub fn legacy_shell_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "legacy")
}

pub fn main_security_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let legacy = legacy_shell_requested(std::env::var_os("HOH_WINDOW_SHELL").as_deref());
    let builder =
        tauri::plugin::Builder::new("main-security").on_navigation(move |webview, url| {
            let allowed = webview.label() != "main" || {
                // The files built into the app, as Tauri looks them up (key `/approval.html`).
                let assets = webview.app_handle().asset_resolver();
                crate::html_preview::navigation_allowed(url)
                    || main_navigation_allowed(url, legacy, |file| {
                        assets.iter().any(|(key, _)| key == file)
                    })
            };
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
            let dir = webview.app_handle().path().app_data_dir().ok();
            let previews = webview
                .state::<crate::html_preview::HtmlPreviews>()
                .inner()
                .clone();
            let paste = webview
                .state::<crate::clipboard_image::ImagePasteGesture>()
                .inner()
                .clone();
            let _ = webview.with_webview(move |platform| {
                paste.install(&platform.inner());
                crate::html_preview::install(&platform.inner(), previews);
                install_zoom(&platform.inner(), dir);
                // No second page ever opens in the window; a link the person clicked goes to the system browser.
                platform.inner().connect_create(|_, action| {
                    let url = action.request().and_then(|request| request.uri());
                    if let Some(link) = url
                        .and_then(|url| crate::opener::clicked_link(&url, action.is_user_gesture()))
                    {
                        let _ = crate::opener::open(&link);
                    }
                    None
                });
            });
        }
    });
    builder.build()
}

/// Ctrl+`+`/`=`, Ctrl+`-`, Ctrl+0 and Ctrl+wheel zoom the page as in desktop T3 (`zoom.rs`); the level is
/// restored at start and saved on every change.
#[cfg(target_os = "linux")]
fn install_zoom(view: &webkit2gtk::WebView, dir: Option<PathBuf>) {
    use gtk::gdk::{self, keys::constants as key};
    use gtk::glib::Propagation;
    use gtk::prelude::*;
    use std::{cell::Cell, rc::Rc};
    let level = Rc::new(Cell::new(dir.as_deref().map_or(0.0, crate::zoom::load)));
    view.set_zoom_level(crate::zoom::factor(level.get()));
    let apply = {
        let (view, level) = (view.clone(), level.clone());
        move |direction: crate::zoom::Zoom| {
            level.set(crate::zoom::next_level(level.get(), direction));
            view.set_zoom_level(crate::zoom::factor(level.get()));
            if let Some(dir) = &dir {
                crate::zoom::save(dir, level.get());
            }
        }
    };
    let on_key = apply.clone();
    view.connect_key_press_event(move |_, event| {
        if !event.state().contains(gdk::ModifierType::CONTROL_MASK) {
            return Propagation::Proceed;
        }
        let pressed = event.keyval();
        let direction = if pressed == key::plus || pressed == key::equal || pressed == key::KP_Add {
            crate::zoom::Zoom::In
        } else if pressed == key::minus || pressed == key::KP_Subtract || pressed == key::underscore
        {
            crate::zoom::Zoom::Out
        } else if pressed == key::_0 || pressed == key::KP_0 {
            crate::zoom::Zoom::Reset
        } else {
            return Propagation::Proceed;
        };
        on_key(direction);
        Propagation::Stop
    });
    view.connect_scroll_event(move |_, event| {
        if !event.state().contains(gdk::ModifierType::CONTROL_MASK) {
            return Propagation::Proceed;
        }
        let direction = match event.direction() {
            gdk::ScrollDirection::Up => crate::zoom::Zoom::In,
            gdk::ScrollDirection::Down => crate::zoom::Zoom::Out,
            gdk::ScrollDirection::Smooth => {
                let dy = event.delta().1;
                if dy < 0.0 {
                    crate::zoom::Zoom::In
                } else if dy > 0.0 {
                    crate::zoom::Zoom::Out
                } else {
                    return Propagation::Stop;
                }
            }
            _ => return Propagation::Proceed,
        };
        apply(direction);
        Propagation::Stop
    });
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

fn cancel_for_status(approvals: &Broker, is_ready: bool) -> Vec<(String, Resolution)> {
    if is_ready {
        Vec::new()
    } else {
        approvals.cancel_all()
    }
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
pub fn subscribe_engine_notifications(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    on_message: Channel<Value>,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Подписка доступна только в главном окне".into());
    }
    *state
        .notifications
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(on_message);
    Ok(())
}

#[tauri::command]
pub async fn engine_request(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    method: String,
    params: Value,
) -> Result<Value, crate::errors::HostError> {
    #[cfg(feature = "probe")]
    crate::probe::trace(format!("invoke:engine_request:received:{method}"));
    let label = window.label().to_owned();
    let engine = state.engine.clone();
    let approvals = state.approvals.clone();
    let ready = state.ready.clone();
    let workspace = state.workspace.clone();
    let handle = window.app_handle().clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
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
                    return Err(crate::errors::CONNECTING.into());
                }
                // `check_main` has passed: the person's own action in the main window confirms it.
                let session_executor = (method == "session/working-directory/set")
                    .then(|| session_executor(&engine, params))
                    .flatten();
                if crate::approval::main_window_confirms(
                    &method,
                    params,
                    session_executor.as_deref(),
                ) {
                    return approvals.confirm_from_main_window(
                        &method,
                        params,
                        session_executor.as_deref(),
                        now_ms(),
                    );
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
                        Err(crate::errors::REJECTED.into())
                    }
                    Err(_) => {
                        #[cfg(feature = "probe")]
                        crate::probe::trace(format!("preflight:{method}:complete:timeout"));
                        if let Some(resolution) = approvals.reject(&approval_label, now_ms()) {
                            dispatch_resolution(&handle, resolution);
                            destroy_window(&handle, &approval_label);
                        }
                        Err(crate::errors::NOT_RECEIVED.into())
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
    .map_err(|_| "Запрос прерван".to_owned())
    .and_then(|answer| answer);
    // The window gets the reason as a code (`errors::HostError`), not only as words.
    outcome.map_err(crate::errors::host_error)
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
    // The main window answers only the request shown in its conversation (`Broker::show_in_chat`).
    if label == "main" {
        let decision =
            state
                .approvals
                .decide_in_chat(&approval_id, outcome, now_ms(), Instant::now())?;
        dispatch_resolution(window.app_handle(), decision.resolution);
        return decision.command_result;
    }
    let decision =
        state
            .approvals
            .decide(&label, &approval_id, outcome, now_ms(), Instant::now())?;
    dispatch_resolution(window.app_handle(), decision.resolution);
    destroy_window(window.app_handle(), &label);
    decision.command_result
}

pub fn shutdown(handle: &tauri::AppHandle, host: &Host) {
    for (label, resolution) in host.approvals.cancel_all() {
        dispatch_resolution(handle, resolution);
        destroy_window(handle, &label);
    }
    host.engine.stop();
}

/// The executor that runs a conversation, as the engine reports it (`hoh/executor/access/get`):
/// `None` for the built-in agent loop, or when the engine cannot tell — then the window asks.
fn session_executor(engine: &Engine, params: &Value) -> Option<String> {
    let session_id = params["sessionId"].as_str()?;
    let answer = engine
        .request(
            "hoh/executor/access/get",
            json!({ "executor": crate::approval::MAIN_WINDOW_EXECUTOR, "sessionId": session_id }),
        )
        .ok()?;
    answer["session"]["executor"].as_str().map(str::to_owned)
}

/// Open a link of an answer. Only the main window; the session's folder is asked from the engine,
/// never taken from the window.
#[tauri::command]
pub async fn open_link(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    kind: String,
    target: String,
    session_id: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Команда доступна только главному окну".into());
    }
    let kind = crate::opener::Kind::parse(&kind)?;
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = if kind == crate::opener::Kind::Url {
            PathBuf::new()
        } else {
            let answer = engine.request(
                "session/working-directory/get",
                json!({ "sessionId": session_id }),
            )?;
            PathBuf::from(answer["cwd"].as_str().ok_or("Рабочая папка неизвестна")?)
        };
        crate::opener::open(&crate::opener::resolve(kind, &target, &folder)?)
    })
    .await
    .map_err(|_| "Запрос прерван".to_owned())?
}

/// Subfolders for the T3 client's «Add project → Local folder» (lead 22.11 20:43): names of folders
/// only, never files, and only to the main window (`folders::list_folders`).
#[tauri::command]
pub async fn browse_folders(
    window: tauri::WebviewWindow,
    partial_path: String,
    cwd: Option<String>,
) -> Result<crate::folders::FolderListing, String> {
    browse_folders_for(window.label(), partial_path, cwd).await
}

/// The window check of `browse_folders`, apart from Tauri's window type so it is tested.
pub async fn browse_folders_for(
    label: &str,
    partial_path: String,
    cwd: Option<String>,
) -> Result<crate::folders::FolderListing, String> {
    if label != "main" {
        return Err("Folder browsing is available to the main window only.".into());
    }
    let home = folder_browser_home(
        std::env::var_os("HOH_WINDOW_HOME"),
        std::env::var_os("HOME"),
    )?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::folders::list_folders(&partial_path, cwd.as_deref(), &home)
    })
    .await
    .map_err(|_| "Request interrupted.".to_owned())?
}

/// Resolve `~` for the folder browser. Isolated window launches can give Claude the person's real
/// HOME while pinning browser discovery to the demo root. An explicit root is a security boundary:
/// malformed or missing roots fail closed instead of silently falling back to ambient HOME.
pub fn folder_browser_home(
    explicit: Option<OsString>,
    ambient: Option<OsString>,
) -> Result<PathBuf, String> {
    if let Some(value) = explicit {
        if value.is_empty() {
            return Err("The window home folder is empty.".into());
        }
        let root = PathBuf::from(value);
        if !root.is_absolute() {
            return Err("The window home folder must be an absolute path.".into());
        }
        let root = root
            .canonicalize()
            .map_err(|_| "The window home folder does not exist or cannot be read.".to_owned())?;
        if !root.is_dir() {
            return Err("The window home folder is not a directory.".into());
        }
        return Ok(root);
    }
    ambient
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "The home folder is unknown.".to_owned())
}

/// Read one raster referenced by a conversation. The page supplies only the session id and path;
/// the engine supplies that session's authoritative project root, and `image_asset` owns every
/// filesystem/type/size decision.
#[tauri::command]
pub async fn read_image_asset(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    session_id: String,
    path: String,
) -> Result<crate::image_asset::ImageAsset, String> {
    if window.label() != "main" {
        return Err("Image unavailable: images are available to the main window only.".into());
    }
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (root, identity, home) = preview_root(&engine, session_id)?;
        crate::image_asset::read_project_image(&root, identity, &path, &home)
    })
    .await
    .map_err(|_| "Image unavailable: the read was interrupted.".to_owned())?
}

/// The OS dialog names the one file whose bytes will be copied into this chat's window cache.
#[tauri::command]
pub async fn confirm_external_image(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    session_id: String,
    path: String,
) -> Result<Option<crate::image_asset::ImageAsset>, String> {
    if window.label() != "main" || !window.is_focused().unwrap_or(false) {
        return Ok(None);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state, session_id, path);
        Ok(None)
    }
    #[cfg(target_os = "linux")]
    {
        crate::image_asset::external_image_path(&path)?;
        static CONFIRMING: AtomicBool = AtomicBool::new(false);
        if CONFIRMING.swap(true, Ordering::AcqRel) {
            return Err("An image confirmation is already open.".into());
        }
        struct DialogSlot;
        impl Drop for DialogSlot {
            fn drop(&mut self) {
                CONFIRMING.store(false, Ordering::Release);
            }
        }
        let _slot = DialogSlot;
        let engine = state.engine.clone();
        let (_, _, home) =
            tauri::async_runtime::spawn_blocking(move || preview_root(&engine, session_id))
                .await
                .map_err(|_| "Could not verify the conversation folder.".to_owned())??;
        let app = window.app_handle().clone();
        let shown_path = path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            use gtk::prelude::*;
            let result = window
                .gtk_window()
                .map_err(|_| "Could not open the image confirmation.".to_owned())
                .map(|parent| {
                    let dialog = gtk::MessageDialog::new(
                        Some(&parent),
                        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
                        gtk::MessageType::Question,
                        gtk::ButtonsType::None,
                        "Show image from outside the project?",
                    );
                    dialog.set_secondary_text(Some(&shown_path));
                    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
                    dialog.add_button("Show image", gtk::ResponseType::Accept);
                    dialog.set_default_response(gtk::ResponseType::Cancel);
                    let accepted = dialog.run() == gtk::ResponseType::Accept;
                    dialog.close();
                    accepted
                });
            let _ = tx.send(result);
        })
        .map_err(|_| "Could not open the image confirmation.".to_owned())?;
        tauri::async_runtime::spawn_blocking(move || {
            if !rx
                .recv()
                .map_err(|_| "The image confirmation was interrupted.".to_owned())??
            {
                return Ok(None);
            }
            crate::image_asset::read_confirmed_image(&path, &home).map(Some)
        })
        .await
        .map_err(|_| "The image read was interrupted.".to_owned())?
    }
}

fn preview_root(
    engine: &Engine,
    session_id: String,
) -> Result<(PathBuf, crate::image_asset::ProjectRootIdentity, PathBuf), String> {
    let answer = engine.request(
        "session/working-directory/get",
        json!({ "sessionId": session_id }),
    )?;
    let root = answer["cwd"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "Image unavailable: the conversation project folder is unknown.".to_owned()
        })?;
    let identity = answer
        .get("rootIdentity")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            "Image unavailable: the conversation project folder identity is unknown.".to_owned()
        })?;
    let parse_identity = |field: &str| -> Result<u64, String> {
        let value = identity
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 20
                    && value.bytes().all(|byte| byte.is_ascii_digit())
            })
            .ok_or_else(|| {
                "Image unavailable: the conversation project folder identity is invalid.".to_owned()
            })?;
        let parsed = value.parse::<u64>().map_err(|_| {
            "Image unavailable: the conversation project folder identity is invalid.".to_owned()
        })?;
        if parsed.to_string() != value {
            return Err(
                "Image unavailable: the conversation project folder identity is invalid."
                    .to_owned(),
            );
        }
        Ok(parsed)
    };
    let expected_root = crate::image_asset::ProjectRootIdentity {
        device: parse_identity("device")?,
        inode: parse_identity("inode")?,
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "Image unavailable: the home folder is unknown.".to_owned())?;
    Ok((PathBuf::from(root), expected_root, home))
}

#[tauri::command]
pub async fn preview_html(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Host>,
    previews: tauri::State<'_, crate::html_preview::HtmlPreviews>,
    session_id: String,
    path: String,
) -> Result<String, String> {
    if window.label() != "main" {
        return Err("HTML preview is available to the main window only.".into());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state, previews, session_id, path);
        Err("HTML preview is unavailable on this platform.".into())
    }
    #[cfg(target_os = "linux")]
    {
        let engine = state.engine.clone();
        let document = tauri::async_runtime::spawn_blocking(move || {
            let (root, identity, home) = preview_root(&engine, session_id)?;
            crate::image_asset::read_project_html(&root, identity, &path, &home)
        })
        .await
        .map_err(|_| "The HTML read was interrupted.".to_owned())?
        .map_err(|error| error.replacen("Image unavailable", "HTML preview unavailable", 1))?;
        previews.store(document)
    }
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
    fn the_engines_approval_text_reaches_the_window_without_a_mask() {
        let url = "https://h.example/p?token=$(touch /x)&n=1#`id`";
        let frame = json!({"method":"approval.pending","params":{
            "humanText": format!("Запустить curl «{url}»?"),
            "reason": format!("Адрес {url}"),
            "details": format!("curl {url}")}});
        assert_eq!(window_frame(frame.clone()), frame);
        // the journal still closes the same text, and says so
        let (journal, nested) = crate::approval::redact_text_checked(&format!("curl {url}"));
        assert!(nested && !journal.contains("touch"), "{journal}");
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
        let (_label, resolution) = cancel_for_status(&broker, false).pop().unwrap();
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
