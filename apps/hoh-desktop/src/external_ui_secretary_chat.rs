//! A gesture-gated bridge from one isolated Workspace page to the main chat UI.
use super::linux;
use gtk::prelude::*;
use javascriptcore::ValueExt;
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};
use tauri::ipc::Channel;
use webkit2gtk::{LoadEvent, UserContentManager, UserContentManagerExt, WebView, WebViewExt};

const HANDLER: &str = "hohSecretaryChat";
const MAX_REQUEST_BYTES: usize = 128 * 1024;
const ACTIVATION_WINDOW: Duration = Duration::from_millis(750);
const REPLY_WINDOW: Duration = Duration::from_secs(15);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SecretaryContext {
    revision: String,
    instructions: String,
    context: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Request {
    OpenChat {
        request_id: u32,
        input: SecretaryContext,
    },
    GetProposal {
        request_id: u32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Action {
    OpenChat,
    GetProposal,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum WorkspaceEvent {
    SecretaryChat {
        request_id: u32,
        action: Action,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<SecretaryContext>,
    },
}

impl Request {
    fn parse(wire: &str) -> Option<WorkspaceEvent> {
        if wire.len() > MAX_REQUEST_BYTES {
            return None;
        }
        let (request_id, action, input) = match serde_json::from_str(wire).ok()? {
            Self::OpenChat { request_id, input }
                if (1..=128).contains(&input.revision.len())
                    && !input.revision.chars().any(char::is_control)
                    && !input.instructions.trim().is_empty() =>
            {
                (request_id, Action::OpenChat, Some(input))
            }
            Self::GetProposal { request_id } => (request_id, Action::GetProposal, None),
            _ => return None,
        };
        (request_id != 0).then_some(WorkspaceEvent::SecretaryChat {
            request_id,
            action,
            input,
        })
    }
}

#[derive(Clone, Copy)]
struct Pending {
    request_id: u32,
    page_request_id: u32,
    action: Action,
    started: Instant,
}

pub(super) struct Bridge {
    id: Cell<u32>,
    events: RefCell<Channel<WorkspaceEvent>>,
    activation: Cell<Option<Instant>>,
    pending: Cell<Option<Pending>>,
    sequence: Cell<u32>,
}

impl Bridge {
    pub(super) fn new(id: u32, events: Channel<WorkspaceEvent>) -> Rc<Self> {
        Rc::new(Self {
            id: Cell::new(id),
            events: RefCell::new(events),
            activation: Cell::new(None),
            pending: Cell::new(None),
            sequence: Cell::new(0),
        })
    }

    pub(super) fn replace_channel(&self, view: &WebView, id: u32, events: Channel<WorkspaceEvent>) {
        self.id.set(id);
        self.events.replace(events);
        self.activation.set(None);
        if let Some(pending) = self.pending.take() {
            publish(
                view,
                pending.page_request_id,
                None,
                Some("The Cabinet page was reopened."),
            );
        }
    }

    fn request(&self, view: &WebView, wire: &str) {
        if !view.is_mapped() || !linux::owns_workspace_view(view) {
            return;
        }
        let Some(WorkspaceEvent::SecretaryChat {
            request_id: page_request_id,
            action,
            input,
        }) = Request::parse(wire)
        else {
            return;
        };
        if !self
            .activation
            .take()
            .is_some_and(|at| at.elapsed() < ACTIVATION_WINDOW)
        {
            publish(
                view,
                page_request_id,
                None,
                Some("Click the Cabinet chat action to continue."),
            );
            return;
        }
        if self
            .pending
            .get()
            .is_some_and(|pending| pending.started.elapsed() < REPLY_WINDOW)
        {
            publish(
                view,
                page_request_id,
                None,
                Some("A Cabinet chat action is already in progress."),
            );
            return;
        }
        // A reloaded plug-in can reuse its JS request IDs. Native IDs survive that reload.
        let Some(request_id) = self.sequence.get().checked_add(1) else {
            publish(
                view,
                page_request_id,
                None,
                Some("Reopen the Cabinet to continue."),
            );
            return;
        };
        self.sequence.set(request_id);
        self.pending.set(Some(Pending {
            request_id,
            page_request_id,
            action,
            started: Instant::now(),
        }));
        if self
            .events
            .borrow()
            .send(WorkspaceEvent::SecretaryChat {
                request_id,
                action,
                input,
            })
            .is_err()
        {
            self.pending.set(None);
            publish(
                view,
                page_request_id,
                None,
                Some("The HoH chat could not be reached."),
            );
        }
    }

    pub(super) fn reply(
        &self,
        view: &WebView,
        id: u32,
        request_id: u32,
        value: Option<String>,
        error: Option<String>,
    ) {
        if self.id.get() != id {
            return;
        }
        if let Some(pending) = self.pending.get()
            && pending.request_id == request_id
            && pending.started.elapsed() < REPLY_WINDOW
            && (pending.action == Action::GetProposal || value.is_none())
        {
            self.pending.set(None);
            publish(
                view,
                pending.page_request_id,
                value.as_deref(),
                error.as_deref(),
            );
        }
    }
}

fn publish(view: &WebView, request_id: u32, value: Option<&str>, error: Option<&str>) {
    let detail = serde_json::json!({ "requestId": request_id, "value": value, "error": error });
    view.evaluate_javascript(
        &format!(
            "window.dispatchEvent(new CustomEvent('hoh-secretary-chat-reply', {{detail: {detail}}}));"
        ),
        None,
        None,
        None::<&gtk::gio::Cancellable>,
        |_| {},
    );
}

pub(super) fn attach(
    view: &WebView,
    manager: &UserContentManager,
    bridge: Rc<Bridge>,
) -> Result<(), String> {
    if !manager.register_script_message_handler(HANDLER) {
        return Err("The Cabinet chat bridge could not be registered.".into());
    }
    let owner = view.downgrade();
    let incoming = Rc::clone(&bridge);
    manager.connect_script_message_received(Some(HANDLER), move |_, result| {
        if let Some(view) = owner.upgrade()
            && let Some(value) = result.js_value().filter(|value| value.is_string())
        {
            incoming.request(&view, value.to_str().as_str());
        }
    });
    let pointer = Rc::clone(&bridge);
    view.connect_button_press_event(move |view, event| {
        if event.button() == 1 && view.is_mapped() && linux::owns_workspace_view(view) {
            pointer.activation.set(Some(Instant::now()));
        }
        gtk::glib::Propagation::Proceed
    });
    let keyboard = Rc::clone(&bridge);
    view.connect_key_press_event(move |view, event| {
        use gtk::gdk::keys::constants as key;
        if [key::Return, key::KP_Enter, key::space].contains(&event.keyval())
            && view.is_mapped()
            && linux::owns_workspace_view(view)
        {
            keyboard.activation.set(Some(Instant::now()));
        }
        gtk::glib::Propagation::Proceed
    });
    let on_unmap = Rc::clone(&bridge);
    view.connect_unmap(move |_| on_unmap.activation.set(None));
    let on_focus_out = Rc::clone(&bridge);
    view.connect_focus_out_event(move |_, _| {
        on_focus_out.activation.set(None);
        gtk::glib::Propagation::Proceed
    });
    let on_load = Rc::clone(&bridge);
    view.connect_load_changed(move |_, event| {
        if event == LoadEvent::Started {
            on_load.activation.set(None);
            on_load.pending.set(None);
        }
    });
    view.connect_web_process_terminated(move |_, _| {
        bridge.activation.set(None);
        bridge.pending.set(None);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_scoped_operations_are_forwarded() {
        let open = r#"{"action":"openChat","requestId":1,"input":{"revision":"abc","instructions":"Draft","context":{}}}"#;
        let get = r#"{"action":"getProposal","requestId":2}"#;
        assert_eq!(
            serde_json::to_value(Request::parse(open).unwrap()).unwrap(),
            serde_json::json!({
                "type": "secretaryChat", "action": "openChat", "requestId": 1,
                "input": { "revision": "abc", "instructions": "Draft", "context": {} }
            })
        );
        assert_eq!(
            serde_json::to_value(Request::parse(get).unwrap()).unwrap(),
            serde_json::json!({"type": "secretaryChat", "action": "getProposal", "requestId": 2})
        );
        for invalid in [
            open.replace("{}", "[]"),
            open.replace(r#""abc""#, r#""""#),
            get.replace("getProposal", "readThread"),
            get.replace("}", r#", "threadId":"other"}"#),
            get.replace(":2", ":0"),
            format!("{get}{}", " ".repeat(MAX_REQUEST_BYTES)),
        ] {
            assert!(Request::parse(&invalid).is_none());
        }
    }
}
