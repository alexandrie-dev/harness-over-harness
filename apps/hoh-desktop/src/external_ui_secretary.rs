use crate::engine::Engine;
use gtk::{glib, prelude::*};
use hoh_protocol::{SECRETARY_PROPOSE_METHOD, validate_params};
use javascriptcore::ValueExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use webkit2gtk::{LoadEvent, UserContentManager, UserContentManagerExt, WebView, WebViewExt};

const HANDLER: &str = "hohSecretaryPropose";
const MAX_REQUEST_BYTES: usize = 128 * 1024;
const GESTURE_LIFETIME: Duration = Duration::from_millis(750);
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: u32,
    params: Value,
}

impl Request {
    fn parse(json: &str) -> Option<Self> {
        (json.len() <= MAX_REQUEST_BYTES)
            .then(|| serde_json::from_str(json).ok())
            .flatten()
    }

    fn has_valid_params(&self) -> bool {
        validate_params(SECRETARY_PROPOSE_METHOD, &self.params).is_ok()
    }
}

struct RequestSlot;

impl RequestSlot {
    fn acquire() -> Option<Self> {
        IN_FLIGHT
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for RequestSlot {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct State {
    activation: Cell<Option<Instant>>,
    document: Cell<u64>,
}

impl State {
    fn reset_document(&self) {
        self.activation.take();
        self.document.set(self.document.get().wrapping_add(1));
    }

    fn take_activation(&self) -> bool {
        self.activation
            .take()
            .is_some_and(|at| at.elapsed() < GESTURE_LIFETIME)
    }
}

fn active(view: &WebView) -> bool {
    view.is_mapped() && super::linux::owns_workspace_view(view)
}

fn publish(view: &WebView, reply: Value) {
    view.evaluate_javascript(
        &format!(
            "window.dispatchEvent(new CustomEvent('hoh-secretary-result', {{detail: {reply}}}));"
        ),
        None,
        None,
        None::<&gtk::gio::Cancellable>,
        |_| {},
    );
}

fn error(id: u32, code: &str, message: &str) -> Value {
    json!({"id": id, "error": {"code": code, "message": message}})
}

fn request_model(engine: &Engine, request: Request) -> Value {
    let id = request.id;
    if let Ok(reply) = engine.request(SECRETARY_PROPOSE_METHOD, request.params)
        && let Some(text) = reply.get("text").and_then(Value::as_str)
    {
        return json!({"id": id, "text": text});
    }
    model_unavailable(id)
}

fn model_unavailable(id: u32) -> Value {
    error(
        id,
        "model_unavailable",
        "Не удалось получить ответ. Проверьте настройки модели HoH и повторите запрос.",
    )
}

pub(super) fn attach(
    view: &WebView,
    manager: &UserContentManager,
    engine: Arc<Engine>,
) -> Result<(), String> {
    let state = Rc::new(State::default());
    let pointer = state.clone();
    view.connect_button_press_event(move |view, event| {
        if event.button() == 1 && active(view) {
            pointer.activation.set(Some(Instant::now()));
        }
        glib::Propagation::Proceed
    });
    let keyboard = state.clone();
    view.connect_key_press_event(move |view, event| {
        use gtk::gdk::keys::constants::{KP_Enter, Return, space};
        if [Return, KP_Enter, space].contains(&event.keyval()) && active(view) {
            keyboard.activation.set(Some(Instant::now()));
        }
        glib::Propagation::Proceed
    });
    let hidden = state.clone();
    view.connect_unmap(move |_| {
        hidden.activation.take();
    });
    let unfocused = state.clone();
    view.connect_focus_out_event(move |_, _| {
        unfocused.activation.take();
        glib::Propagation::Proceed
    });
    let loading = state.clone();
    view.connect_load_changed(move |_, event| {
        if event == LoadEvent::Started {
            loading.reset_document();
        }
    });
    let terminated = state.clone();
    view.connect_web_process_terminated(move |_, _| terminated.reset_document());

    let owner = view.downgrade();
    manager.connect_script_message_received(Some(HANDLER), move |_, result| {
        let Some(view) = owner.upgrade().filter(super::linux::owns_workspace_view) else {
            return;
        };
        let Some(value) = result.js_value().filter(|value| value.is_string()) else {
            return;
        };
        let Some(request) = Request::parse(value.to_str().as_str()) else {
            return;
        };
        let id = request.id;
        if !state.take_activation() || !view.is_mapped() {
            return publish(
                &view,
                error(
                    id,
                    "activation_required",
                    "Отправьте запрос кнопкой или клавишей Enter.",
                ),
            );
        }
        if !request.has_valid_params() {
            return publish(&view, error(id, "invalid_request", "Некорректный запрос."));
        }
        let Some(slot) = RequestSlot::acquire() else {
            return publish(
                &view,
                error(id, "busy", "Дождитесь ответа на предыдущий запрос."),
            );
        };
        let engine = engine.clone();
        let owner = owner.clone();
        let document = state.document.get();
        let state = Rc::downgrade(&state);
        glib::MainContext::default().spawn_local(async move {
            let reply = tauri::async_runtime::spawn_blocking(move || {
                let _slot = slot;
                request_model(&engine, request)
            })
            .await
            .unwrap_or_else(|_| model_unavailable(id));
            if let (Some(view), Some(state)) = (owner.upgrade(), state.upgrade())
                && state.document.get() == document
                && super::linux::owns_workspace_view(&view)
            {
                publish(&view, reply);
            }
        });
    });
    if !manager.register_script_message_handler(HANDLER) {
        return Err("The plugin secretary bridge could not be registered.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_fields_and_counts_utf8_bytes() {
        let params = json!({"message": "я".repeat(2048), "instructions": "Rules", "context": {}});
        let mut request = Request::parse(&json!({"id": 1, "params": params}).to_string()).unwrap();
        assert!(request.has_valid_params());
        request.params["message"] = "я".repeat(2049).into();
        assert!(!request.has_valid_params());
        request.params["message"] = "Message".into();
        request.params["context"] = json!([]);
        assert!(!request.has_valid_params());
        request.params["context"] = json!({});
        request.params["extra"] = json!(true);
        assert!(!request.has_valid_params());
        assert!(Request::parse(&" ".repeat(MAX_REQUEST_BYTES + 1)).is_none());
    }

    #[test]
    fn activation_is_consumed_and_reset_when_the_document_changes() {
        let state = State::default();
        assert!(!state.take_activation());
        state.activation.set(Some(Instant::now()));
        assert!(state.take_activation());
        assert!(!state.take_activation());
        state.activation.set(Some(Instant::now()));
        state.reset_document();
        assert!(!state.take_activation());
        assert_eq!(state.document.get(), 1);
    }

    #[test]
    fn request_slot_releases_when_the_worker_finishes() {
        let slot = RequestSlot::acquire().unwrap();
        assert!(RequestSlot::acquire().is_none());
        drop(slot);
        assert!(RequestSlot::acquire().is_some());
    }
}
