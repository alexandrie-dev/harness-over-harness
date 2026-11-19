//! Tauri command adapters. Security decisions live in Rust, never in the page.
use crate::{
    approval::{ApprovalView, Broker, OpenApproval, Resolution, now_ms},
    bridge,
    engine::{Engine, HostEvent, ProcessConfig},
};
use hoh_protocol::ApprovalOutcome;
use serde_json::{Value, json};
use std::{
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

pub struct Host {
    pub engine: Arc<Engine>,
    pub approvals: Arc<Broker>,
    ready: Arc<AtomicBool>,
}

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "probe")]
    crate::probe::trace("native:setup:start");
    let mut key = [0_u8; 32];
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(&mut key)?;
    let approvals = Arc::new(Broker::new(key)?);
    let ready = Arc::new(AtomicBool::new(false));
    let path = app.path().app_config_dir()?.join("engine.json");
    let config = if path.exists() {
        serde_json::from_slice::<ProcessConfig>(&std::fs::read(path)?)?
    } else {
        let folder = std::env::current_exe()?
            .parent()
            .ok_or("Нет каталога приложения")?
            .to_owned();
        let engine = folder.join("hoh");
        ProcessConfig {
            path: std::env::var_os("HOH_ENGINE_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    if engine.exists() {
                        engine
                    } else {
                        folder.join("hoh-window-stub")
                    }
                }),
            args: Vec::new(),
            cwd: app.path().home_dir()?,
            provider: "mock".into(),
            model: "mock".into(),
        }
    };
    let handle = app.handle().clone();
    let event_approvals = approvals.clone();
    let event_ready = ready.clone();
    let init = json!({"cwd":config.cwd,"provider":config.provider,"model":config.model,"capabilities":{"approvals":"host"},"hoh":{"protocolVersion":1,"approvalKey":hoh_protocol::approval::hex(&key)}});
    let engine = Engine::start(
        Arc::new(config),
        init,
        Arc::new(move |event| handle_event(&handle, &event_approvals, &event_ready, event)),
    );
    app.manage(Host {
        engine,
        approvals,
        ready,
    });
    #[cfg(feature = "probe")]
    crate::probe::trace("native:setup:complete");
    Ok(())
}

fn handle_event(
    handle: &tauri::AppHandle,
    approvals: &Arc<Broker>,
    ready: &Arc<AtomicBool>,
    event: HostEvent,
) {
    match event {
        HostEvent::Notification(frame) => {
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
        } => match approvals.begin_engine(generation, id.clone(), &method, params, now_ms()) {
            Ok(open) => schedule_window(handle, approvals, open),
            Err(_) => respond_error(handle, generation, id, "Подтверждение недоступно"),
        },
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
            expires_at.saturating_sub(now_ms()).min(300_000),
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
    approvals.reject(label, now)
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
    let handle = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        bridge::request_with_approval(&engine, &label, &method, params, |params| {
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
        })
    })
    .await
    .map_err(|_| "Запрос прерван".to_owned())?
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
    use std::sync::mpsc;

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
}
