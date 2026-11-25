use tauri::Manager;

fn setup_application(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Tauri creates configured windows before setup, so probes can safely attach
    // observers to an already-loaded main WebView.
    #[cfg(feature = "probe")]
    {
        hoh_desktop::probe::trace("setup:application:start");
        hoh_desktop::probe::setup(app)
    }
    #[cfg(not(feature = "probe"))]
    {
        hoh_desktop::native::setup(app)
    }
}

fn main() {
    #[cfg(feature = "probe")]
    hoh_desktop::probe::trace("process:start");
    let builder = tauri::Builder::default();
    #[cfg(target_os = "linux")]
    let builder = builder.manage(hoh_desktop::clipboard_image::ImagePasteGesture::default());
    builder
        .manage(hoh_desktop::html_preview::HtmlPreviews::default())
        .plugin(hoh_desktop::native::main_security_plugin())
        .invoke_handler(tauri::generate_handler![
            hoh_desktop::native::engine_request,
            hoh_desktop::native::pick_folder,
            hoh_desktop::native::browse_folders,
            hoh_desktop::native::read_image_asset,
            hoh_desktop::native::confirm_external_image,
            hoh_desktop::native::preview_html,
            #[cfg(target_os = "linux")]
            hoh_desktop::clipboard_image::read_pasted_image,
            hoh_desktop::native::open_link,
            hoh_desktop::native::question_answer,
            hoh_desktop::native::approval_history,
            hoh_desktop::native::approval_current,
            hoh_desktop::native::approval_decide
        ])
        .on_window_event(|window, event| {
            if window.label() == "main"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                window.app_handle().exit(0);
            }
        })
        .setup(setup_application)
        .build(tauri::generate_context!())
        .expect("build HoH desktop")
        .run(|app, event| {
            let exiting = matches!(
                event,
                tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
            );
            if exiting {
                #[cfg(feature = "probe")]
                hoh_desktop::probe::trace("process:exit-event");
                if let Some(host) = app.try_state::<hoh_desktop::native::Host>() {
                    hoh_desktop::native::shutdown(app, &host);
                }
            }
        });
}
