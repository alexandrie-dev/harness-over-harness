//! Native operations used behind the trusted desktop commands.

#[cfg(all(feature = "desktop", target_os = "linux"))]
pub(crate) fn pick_folder(window: &tauri::WebviewWindow) -> Result<Option<String>, String> {
    use gtk::prelude::*;

    let parent = window
        .gtk_window()
        .map_err(|_| "Нет родительского окна".to_owned())?;
    let dialog = gtk::FileChooserNative::new(
        Some("Папка для работы"),
        Some(&parent),
        gtk::FileChooserAction::SelectFolder,
        Some("Выбрать"),
        Some("Отмена"),
    );
    dialog.set_local_only(true);
    let result = (dialog.run() == gtk::ResponseType::Accept)
        .then(|| dialog.filename())
        .flatten()
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map_err(|_| "Имя папки должно быть в UTF-8".to_owned())
        })
        .transpose();
    dialog.destroy();
    result
}

#[cfg(all(feature = "desktop", target_os = "macos"))]
pub(crate) fn pick_folder(_: &tauri::WebviewWindow) -> Result<Option<String>, String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::NSString;
    use std::ffi::CStr;

    let main = MainThreadMarker::new().ok_or("Выбор папки прерван")?;
    let panel = NSOpenPanel::openPanel(main);
    panel.setTitle(Some(&NSString::from_str("Папка для работы")));
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);
    if panel.runModal() != NSModalResponseOK {
        return Ok(None);
    }
    let url = panel.URL().ok_or("Выбранная папка недоступна")?;
    // NSURL keeps its null-terminated file-system representation live for the conversion.
    let path = unsafe { CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) };
    path.to_str()
        .map(|path| Some(path.to_owned()))
        .map_err(|_| "Имя папки должно быть в UTF-8".to_owned())
}

#[cfg(all(
    feature = "desktop",
    not(any(target_os = "linux", target_os = "macos"))
))]
pub(crate) fn pick_folder(_: &tauri::WebviewWindow) -> Result<Option<String>, String> {
    Err("Системный выбор папки недоступен на этой платформе".into())
}
