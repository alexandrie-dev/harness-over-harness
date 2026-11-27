use tauri::Manager;

fn setup_application(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Tauri creates configured windows before setup, so probes can safely attach
    // observers to an already-loaded main WebView.
    #[cfg(all(feature = "probe", target_os = "linux"))]
    {
        hoh_desktop::probe::trace("setup:application:start");
        hoh_desktop::probe::setup(app)
    }
    #[cfg(not(all(feature = "probe", target_os = "linux")))]
    {
        hoh_desktop::native::setup(app)
    }
}

fn main() {
    #[cfg(all(feature = "probe", target_os = "linux"))]
    hoh_desktop::probe::trace("process:start");
    let builder = tauri::Builder::default();
    #[cfg(target_os = "linux")]
    let builder = builder.manage(hoh_desktop::clipboard_image::ImagePasteGesture::default());
    builder
        .manage(hoh_desktop::html_preview::HtmlPreviews::default())
        .plugin(hoh_desktop::native::main_security_plugin())
        .on_permission_request(hoh_desktop::native::webview_permission)
        .invoke_handler(tauri::generate_handler![
            hoh_desktop::native::engine_request,
            hoh_desktop::native::environment_platform,
            hoh_desktop::native::subscribe_engine_notifications,
            hoh_desktop::native::pick_folder,
            hoh_desktop::native::browse_folders,
            hoh_desktop::git_workspace::project_git,
            hoh_desktop::native::project_list_entries,
            hoh_desktop::native::project_read_file,
            hoh_desktop::native::project_open_in_editor,
            hoh_desktop::native::project_available_editors,
            hoh_desktop::native::read_image_asset,
            hoh_desktop::native::confirm_external_image,
            hoh_desktop::native::preview_html,
            hoh_desktop::external_ui::list_external_ui_plugins,
            hoh_desktop::external_ui::install_external_ui_plugin,
            hoh_desktop::external_ui::open_external_ui_plugin,
            hoh_desktop::external_ui::layout_external_ui_workspace,
            hoh_desktop::external_ui::reply_external_ui_secretary,
            hoh_desktop::external_ui::update_external_ui_panel,
            #[cfg(target_os = "linux")]
            hoh_desktop::clipboard_image::read_pasted_image,
            hoh_desktop::native::open_link,
            hoh_desktop::native::question_answer,
            hoh_desktop::native::question_secret_answer,
            hoh_desktop::native::store_chat_secret,
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
                #[cfg(all(feature = "probe", target_os = "linux"))]
                hoh_desktop::probe::trace("process:exit-event");
                if let Some(host) = app.try_state::<hoh_desktop::native::Host>() {
                    hoh_desktop::native::shutdown(app, &host);
                }
            }
        });
}
