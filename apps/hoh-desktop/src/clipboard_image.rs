//! GTK supplies an image when a real paste reaches WebKit with no clipboard formats.
use gtk::{gdk, glib::Propagation, prelude::*};
use serde::Serialize;
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const MAX_BYTES: i32 = 10 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct ImagePasteGesture(Arc<Mutex<Option<Instant>>>);

impl ImagePasteGesture {
    pub fn install(&self, view: &webkit2gtk::WebView) {
        let gesture = self.clone();
        view.connect_key_press_event(move |_, event| {
            use gdk::{ModifierType, keys::constants as key};
            let modifiers = event.state()
                & (ModifierType::CONTROL_MASK
                    | ModifierType::SHIFT_MASK
                    | ModifierType::MOD1_MASK
                    | ModifierType::SUPER_MASK
                    | ModifierType::META_MASK);
            if modifiers == ModifierType::CONTROL_MASK
                && event.keyval() == key::v
                && let Ok(mut ticket) = gesture.0.lock()
            {
                *ticket = Some(Instant::now());
            }
            Propagation::Proceed
        });
    }

    fn consume(&self) -> bool {
        self.0
            .lock()
            .ok()
            .and_then(|mut ticket| ticket.take())
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardImage {
    data_base64: String,
    mime_type: &'static str,
    name: &'static str,
}

const FORMATS: &[(&str, &str)] = &[
    ("image/png", "clipboard.png"),
    ("image/jpeg", "clipboard.jpg"),
    ("image/webp", "clipboard.webp"),
    ("image/gif", "clipboard.gif"),
];

#[tauri::command]
pub async fn read_pasted_image(
    window: tauri::WebviewWindow,
    gesture: tauri::State<'_, ImagePasteGesture>,
) -> Result<Option<ClipboardImage>, String> {
    if window.label() != "main" || !window.is_focused().unwrap_or(false) || !gesture.consume() {
        return Ok(None);
    }
    let (send, receive) = mpsc::channel();
    window
        .with_webview(move |platform| {
            let clipboard = platform.inner().clipboard(&gdk::SELECTION_CLIPBOARD);
            clipboard.request_contents(&gdk::Atom::intern("TARGETS"), move |clipboard, data| {
                let targets = data.targets().unwrap_or_default();
                let Some(&(mime, name)) = FORMATS
                    .iter()
                    .find(|(mime, _)| targets.contains(&gdk::Atom::intern(mime)))
                else {
                    let _ = send.send(Ok(None));
                    return;
                };
                clipboard.request_contents(&gdk::Atom::intern(mime), move |_, data| {
                    let result = match data.length() {
                        ..=0 => Ok(None),
                        length if length > MAX_BYTES => {
                            Err("Clipboard images can be up to 10 MiB.".to_owned())
                        }
                        _ => Ok(Some((data.data(), mime, name))),
                    };
                    let _ = send.send(result);
                });
            });
        })
        .map_err(|_| "Could not open the clipboard.".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some((bytes, mime_type, name)) = receive
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "The clipboard did not respond.".to_owned())??
        else {
            return Ok(None);
        };
        let data_base64 = crate::image_asset::encode_clipboard_image(&bytes, mime_type)?;
        Ok(Some(ClipboardImage {
            data_base64,
            mime_type,
            name,
        }))
    })
    .await
    .map_err(|_| "The clipboard read was interrupted.".to_owned())?
}
