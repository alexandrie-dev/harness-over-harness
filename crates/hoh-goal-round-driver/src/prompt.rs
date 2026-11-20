use hoh_goal::GoalView;
use hoh_llm::ContentBlock;
use serde_json::Value;

/// Render the complete goal-round instruction retained in session history.
pub fn render_goal_round_prompt(goal: &GoalView, round: u64) -> Vec<ContentBlock> {
    let objective = hoh_util_values::js_json_stringify(&Value::String(goal.goal.objective.clone()));
    vec![ContentBlock::text(format!(
        "<goal_round>\n\
Objective: {objective}\n\
Round: {round}/{}\n\n\
Continue working toward the objective in this same session. Treat the current workspace, tool results, and durable session state as authoritative; inspect them instead of assuming earlier narration is still current. Make concrete progress and verify the result. Before claiming completion, gather evidence that the whole objective is achieved, read the current goal, and mark it complete. If work remains, leave the goal active for the next round. Follow the configured goal-tool policy before reporting a blocker.\n\
</goal_round>",
        goal.goal.max_goal_rounds
    ))]
}
