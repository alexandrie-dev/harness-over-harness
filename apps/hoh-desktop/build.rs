fn main() {
    // The production engine path is baked in with option_env!; rebuild when it changes.
    println!("cargo:rerun-if-env-changed=HOH_ENGINE_PATH");
    #[cfg(feature = "desktop")]
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "engine_request",
            "pick_folder",
            "browse_folders",
            "read_image_asset",
            "open_link",
            "question_answer",
            "approval_history",
            "approval_current",
            "approval_decide",
        ]),
    ))
    .expect("desktop build configuration");
}
