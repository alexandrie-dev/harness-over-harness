//! What the window gets when `engine_request` fails: the reason as a CODE next to the words, so the window never has to
//! recognise a refusal by its wording (HOH-35: after «Отклонить» nobody sees «Неизвестная ошибка»).
//!
//! The host's own failures carry the strings below; the engine's errors are reduced to a few codes in `engine.rs` (its text and
//! data can hold secrets and never travel on). The window reads `code`; `message` stays the words the host chose, in Russian,
//! and is the only thing older code (`error.message`) looks at, so the chat keeps working with either form.

use serde::Serialize;

/// The person pressed «Отклонить» in the protection window.
pub const REJECTED: &str = "Действие отклонено";
/// No answer in time: the confirmation expired and was rejected by the host.
pub const NOT_RECEIVED: &str = "Подтверждение не получено";
/// The engine is still starting.
pub const CONNECTING: &str = "Помощник ещё подключается";
pub const RECONNECTING: &str = "Помощник переподключается";
pub const CONNECTION_INTERRUPTED: &str = "Связь с Помощником прервана. Запрос не повторён";
/// The engine refused the request (its own text is never passed on).
pub const REFUSED: &str = "Помощник отклонил запрос";
/// The engine refused to change the folder of a chat that runs through an executor (`EXECUTOR_FOLDER_FIXED`).
pub const FOLDER_FIXED: &str = "executor-folder-fixed";
pub const FOLDER_CHANGED: &str = "conversation-folder-changed";
pub const TRUST_REVOKE_BUSY: &str = "trust-revoke-busy";
pub const CHAT_DELETE_BUSY: &str = "chat-delete-busy";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostError {
    /// `rejected` | `expired` | `connecting` | `unavailable` | `folder-fixed` | `refused` | `failed`.
    pub code: &'static str,
    pub message: String,
}

/// The code of a failure of the host or the engine. Only the host's own constants are recognised; any other text is `failed`.
pub fn host_error(message: String) -> HostError {
    let code = match message.as_str() {
        REJECTED => "rejected",
        NOT_RECEIVED => "expired",
        CONNECTING | RECONNECTING | CONNECTION_INTERRUPTED => "connecting",
        crate::engine::METHOD_UNAVAILABLE => "unavailable",
        FOLDER_FIXED => "folder-fixed",
        FOLDER_CHANGED => "folder-changed",
        TRUST_REVOKE_BUSY => "trust-revoke-busy",
        CHAT_DELETE_BUSY => "chat-delete-busy",
        REFUSED => "refused",
        _ => "failed",
    };
    // The folder refusal is shown in the person's words; the other messages are what they always were.
    let message = if code == "folder-fixed" {
        "Чат с Claude работает в одной папке. Чтобы работать в другой, начните новый чат".to_owned()
    } else {
        message
    };
    HostError { code, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_host_refusal_has_its_own_code_and_the_form_the_window_gets() {
        for (message, code) in [
            (REJECTED, "rejected"),
            (NOT_RECEIVED, "expired"),
            (CONNECTING, "connecting"),
            (crate::engine::METHOD_UNAVAILABLE, "unavailable"),
            (REFUSED, "refused"),
            ("Метод не разрешён окну", "failed"),
        ] {
            let sent = serde_json::to_value(host_error(message.to_owned())).unwrap();
            assert_eq!(sent, json!({"code": code, "message": message}));
        }
    }

    #[test]
    fn the_folder_refusal_comes_with_the_words_for_the_person() {
        let sent = serde_json::to_value(host_error(FOLDER_FIXED.to_owned())).unwrap();
        assert_eq!(sent["code"], "folder-fixed");
        assert_eq!(
            sent["message"],
            "Чат с Claude работает в одной папке. Чтобы работать в другой, начните новый чат"
        );
    }
}
