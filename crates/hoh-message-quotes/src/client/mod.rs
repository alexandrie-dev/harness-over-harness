//! Browser-independent quote behavior consumed by a Tauri UI adapter.

mod locales;
mod quote_rule;
mod quote_text;
mod selection;

pub use locales::{MESSAGE_QUOTES_NAMESPACE, quote_add_label};
pub use quote_rule::{UserTextNode, evaluate_quote_text, quote_rule_match};
pub use quote_text::{MessageQuoteRange, message_quote_ranges, quote_text_of};
pub use selection::{
    QuoteActionResult, QuoteInteractionTrigger, SessionQuoteState, SubagentMode,
    activate_quote_action, admit_quote_interaction, quote_selection_enabled,
};

/// Browser plugin name.
pub const NAME: &str = "message-quotes";
/// Source client services required by the browser contribution.
pub const INJECT: [&str; 4] = ["slots", "locale", "sessions", "uiConversation"];
/// Conversation interaction id and keyed Slot contribution.
pub const QUOTE_INTERACTION_ID: &str = "quote";
/// Quote rule order relative to other UserText rules.
pub const QUOTE_RULE_ORDER: i32 = -10;
/// Slot that renders the quote selection action.
pub const QUOTE_ACTION_SLOT: &str = "conversation.chat.text-interaction";
/// Slot that renders decoded quote nodes.
pub const QUOTED_TEXT_SLOT: &str = "conversation.user.text.node";
/// Source priority of the quote-node renderer contribution.
pub const QUOTED_TEXT_PRIORITY: i32 = 100;
/// Source styling for the floating quote action, retained for the Tauri adapter.
pub const QUOTE_SELECTION_CSS: &str = include_str!("../../assets/quote-selection.css");
