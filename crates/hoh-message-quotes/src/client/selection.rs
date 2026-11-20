//! Pure admission and click outcomes for the quote selection action.

/// Subagent mode relevant to whether its conversation remains writable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentMode {
    Continuable,
    OneShot,
    Unknown,
}

/// Session facts consumed by the source quote interaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionQuoteState {
    pub removed: bool,
    pub subagent_mode: Option<SubagentMode>,
}

/// Browser gesture that requested a text interaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteInteractionTrigger {
    Selection,
    ContextMenu,
    Other,
}

/// Whether the quote action may remain visible for this Session.
pub fn quote_selection_enabled(session: SessionQuoteState) -> bool {
    !session.removed
        && matches!(
            session.subagent_mode,
            None | Some(SubagentMode::Continuable)
        )
}

/// Admit only readable selections with a live writable Session binding.
pub fn admit_quote_interaction(
    trigger: QuoteInteractionTrigger,
    selected_text: Option<&str>,
    session: Option<SessionQuoteState>,
) -> bool {
    trigger == QuoteInteractionTrigger::Selection
        && selected_text.is_some()
        && session.is_some_and(quote_selection_enabled)
}

/// Observable effects of rendering or activating the quote action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteActionResult {
    pub visible: bool,
    pub insert_text: Option<String>,
    pub clear_selection: bool,
    pub dismiss: bool,
}

/// Model the source button's invalidation and click decisions.
pub fn activate_quote_action(
    session: SessionQuoteState,
    selected_text: Option<&str>,
    insert_succeeded: bool,
) -> QuoteActionResult {
    if !quote_selection_enabled(session) {
        return QuoteActionResult {
            visible: false,
            insert_text: None,
            clear_selection: false,
            dismiss: true,
        };
    }
    let Some(text) = selected_text else {
        return QuoteActionResult {
            visible: true,
            insert_text: None,
            clear_selection: false,
            dismiss: true,
        };
    };
    QuoteActionResult {
        visible: true,
        insert_text: Some(text.into()),
        clear_selection: insert_succeeded,
        dismiss: insert_succeeded,
    }
}
