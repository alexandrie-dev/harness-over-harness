fn main() {
    // The production engine path is baked in with option_env!; rebuild when it changes.
    println!("cargo:rerun-if-env-changed=HOH_ENGINE_PATH");
    #[cfg(feature = "desktop")]
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "engine_request",
            "environment_platform",
            "subscribe_engine_notifications",
            "pick_folder",
            "browse_folders",
            "project_git",
            "project_list_entries",
            "project_read_file",
            "project_open_in_editor",
            "project_available_editors",
            "read_image_asset",
            "confirm_external_image",
            "read_pasted_image",
            "preview_html",
            "list_external_ui_plugins",
            "install_external_ui_plugin",
            "open_external_ui_plugin",
            "layout_external_ui_workspace",
            "reply_external_ui_secretary",
            "update_external_ui_panel",
            "open_link",
            "question_answer",
            "question_secret_answer",
            "store_chat_secret",
            "approval_history",
            "approval_current",
            "approval_decide",
        ]),
    ))
    .expect("desktop build configuration");
}
