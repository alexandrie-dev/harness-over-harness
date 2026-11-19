fn main() {
    #[cfg(feature = "desktop")]
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "engine_request",
            "pick_folder",
            "approval_current",
            "approval_decide",
        ]),
    ))
    .expect("desktop build configuration");
}
