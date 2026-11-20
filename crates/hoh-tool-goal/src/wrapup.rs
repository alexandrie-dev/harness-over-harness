//! Model-visible wrap-up instruction for a terminal autonomous goal update.

use hoh_llm::ContentBlock;

const GROUNDING: &str = "Report only what earlier rounds and tool results in this session actually establish; when a detail is not in the session, say so instead of inventing it. ";

/// Render a fresh one-block closing instruction for `ToolRunContext::defer_context`.
pub fn render_wrapup_context(objective: &str, blocked_reason: Option<&str>) -> Vec<ContentBlock> {
    let heading = format!(
        "Objective: {}\n",
        hoh_util_values::js_json_stringify(&serde_json::Value::String(objective.into()))
    );
    let text = match blocked_reason {
        None => format!(
            "<goal_complete>\n{heading}The goal is marked complete and this autonomous run is ending. Write the closing message to the user now: state the outcome, summarize what was done and how it was verified, and point to the concrete results (files, commits, or other artifacts). {GROUNDING}Note anything the user should review or do next. Address the user directly. Do not call any more tools in this run; further work waits for the user's next instruction.\n</goal_complete>"
        ),
        Some(reason) => {
            let reason = hoh_util_values::js_json_stringify(&Value::String(reason.into()));
            format!(
                "<goal_blocked>\n{heading}Blocked: {reason}\nThe goal is marked blocked and this autonomous run is ending. Write the closing message to the user now: state what has been completed so far, describe the concrete blocking condition and what you tried, and say exactly what you need from the user to continue. {GROUNDING}Address the user directly. Do not call any more tools in this run; further work waits for the user's next instruction.\n</goal_blocked>"
            )
        }
    };
    vec![ContentBlock::text(text)]
}

use serde_json::Value;
