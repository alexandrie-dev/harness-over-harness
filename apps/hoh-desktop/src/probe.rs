//! Feature-gated native hooks for headless WebKit and approval probes.
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;
use webkit2gtk::WebViewExt;

static APPROVAL_WINDOW_CLOSED: AtomicBool = AtomicBool::new(false);

pub(crate) fn mark_approval_window_closed() {
    trace("approval:ready-close-marker:set");
    APPROVAL_WINDOW_CLOSED.store(true, Ordering::Release);
}

pub fn approval_requested() -> bool {
    std::env::var_os("HOH_APPROVAL_PROBE").is_some()
        || std::env::args().any(|argument| argument == "--approval-probe")
}

fn requested() -> bool {
    approval_requested() || std::env::var_os("HOH_WEBKIT_PROBE").is_some()
}

pub fn trace(message: impl AsRef<str>) {
    if requested() {
        eprintln!("HOH_PROBE_TRACE:{}", message.as_ref());
    }
}

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    trace("setup:probe-hooks:start");
    let approval_probe = approval_requested();
    let webkit_probe = std::env::var_os("HOH_WEBKIT_PROBE").is_some() && !approval_probe;
    if !webkit_probe {
        trace("setup:native-host:start");
        crate::native::setup(app)?;
        trace("setup:native-host:complete");
    }
    if approval_probe {
        trace("main:lookup:start");
        let window = app.get_webview_window("main").ok_or("Нет главного окна")?;
        trace("main:created");
        let handle = app.handle().clone();
        window.with_webview(|webview| {
            webview.inner().connect_title_notify(move |view| {
                if let Some(title) = view.title() {
                    trace(if title.starts_with("HOH_APPROVAL_PROBE:") {
                        "main:title:probe-result"
                    } else {
                        "main:title:changed"
                    });
                    if let Some(result) = title.strip_prefix("HOH_APPROVAL_PROBE:") {
                        let pass = result.contains("\"pass\":true")
                            && APPROVAL_WINDOW_CLOSED.load(Ordering::Acquire);
                        if pass {
                            println!("HOH_APPROVAL_PROBE_RESULT:{result}");
                        } else {
                            println!(
                                "HOH_APPROVAL_PROBE_RESULT:{{\"pass\":false,\"detail\":\"approval window did not reach ready-close\"}}"
                            );
                        }
                        trace(if pass {
                            "exit:approval-probe:success"
                        } else {
                            "exit:approval-probe:failure"
                        });
                        handle.exit(if pass { 0 } else { 1 });
                    }
                }
            });
        })?;
        trace("main:title-observer:attached");
    }
    if webkit_probe {
        trace("webkit:lookup:start");
        let window = app.get_webview_window("main").ok_or("Нет главного окна")?;
        window.with_webview(|webview| {
            let view = webview.inner();
            view.connect_title_notify(|view| {
                if let Some(title) = view.title()
                    && let Some(result) = title.strip_prefix("HOH_PROBE:")
                {
                    println!("HOH_PROBE_RESULT:{result}");
                    trace("exit:webkit-probe");
                    std::process::exit(if result.contains("\"pass\":true") {
                        0
                    } else {
                        1
                    });
                }
            });
            // These C getters take no arguments, touch no caller-owned memory,
            // and report the loaded WebKitGTK library version.
            let version = unsafe {
                format!(
                    "{}.{}.{}",
                    webkit2gtk::ffi::webkit_get_major_version(),
                    webkit2gtk::ffi::webkit_get_minor_version(),
                    webkit2gtk::ffi::webkit_get_micro_version()
                )
            };
            trace("webkit:navigate:probe-page");
            view.load_uri(&format!("tauri://localhost/probe.html?webkitgtk={version}"));
        })?;
    }
    trace("setup:probe-hooks:complete");
    Ok(())
}
