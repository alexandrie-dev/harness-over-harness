//! Human-facing `/goal` command over the persisted same-session goal domain.

use hoh_commands::{
    CommandDefinition, CommandDefinitionId, CommandInputDescriptor, CommandInvocation,
    CommandResult, Commands,
};
use hoh_error::{BoxError, HarnessError};
use hoh_goal::{
    CreateGoalRequest, EditGoalRequest, GoalActivation, GoalError, GoalPhase, GoalService,
    GoalView, Goals,
};
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_llm::{ContentBlock, NewUserMessage, create_user_message};

/// Cordis plugin name.
pub const NAME: &str = "command-goal";
/// Required service names, in source declaration order.
pub const INJECT: &[&str] = &["commands", "goals"];
pub const DEFINITION_ID: &str = "@deepseek-ai/dsh-command-goal";
pub const COMMAND_NAME: &str = "goal";
pub const DESCRIPTION: &str = "Set or view the goal for a long-running task";
pub const INPUT_HINT: &str = "[<objective>|clear|edit <objective>|pause|resume]";
pub const USAGE: &str = "Usage: /goal [<objective>|clear|edit <objective>|pause|resume]";

#[derive(Clone, Debug, PartialEq, Eq)]
enum GoalCommand {
    Show,
    Create(String),
    Edit(String),
    InvalidEdit,
    Pause,
    Resume,
    Clear,
}

fn parse_goal_command(raw_input: &str) -> GoalCommand {
    let input = hoh_util_values::js_trim(raw_input);
    if input.is_empty() {
        return GoalCommand::Show;
    }
    if input.eq_ignore_ascii_case("clear") {
        return GoalCommand::Clear;
    }
    if input.eq_ignore_ascii_case("pause") {
        return GoalCommand::Pause;
    }
    if input.eq_ignore_ascii_case("resume") {
        return GoalCommand::Resume;
    }
    if input.eq_ignore_ascii_case("edit") {
        return GoalCommand::InvalidEdit;
    }
    if input
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("edit"))
        && input[4..]
            .chars()
            .next()
            .is_some_and(hoh_util_values::is_js_white_space)
    {
        return GoalCommand::Edit(hoh_util_values::js_trim(&input[4..]).to_owned());
    }
    GoalCommand::Create(input.to_owned())
}

fn phase_label(phase: GoalPhase) -> &'static str {
    match phase {
        GoalPhase::Active => "active",
        GoalPhase::Paused => "paused",
        GoalPhase::Blocked => "blocked",
        GoalPhase::Complete => "complete",
    }
}

fn activation_label(activation: GoalActivation) -> &'static str {
    match activation {
        GoalActivation::Armed => "armed",
        GoalActivation::Disarmed => "disarmed",
    }
}

fn command_hint(goal: &GoalView) -> &'static str {
    match goal.goal.phase {
        GoalPhase::Active if goal.activation == GoalActivation::Armed => {
            "/goal edit <objective>, /goal pause, /goal clear"
        }
        GoalPhase::Active => "/goal edit <objective>, /goal resume, /goal clear",
        GoalPhase::Paused | GoalPhase::Blocked => {
            "/goal edit <objective>, /goal resume, /goal clear"
        }
        GoalPhase::Complete => "/goal <objective>, /goal clear",
    }
}

fn render_goal(title: &str, goal: &GoalView) -> CommandResult {
    let mut lines = vec![
        title.to_owned(),
        format!("Status: {}", phase_label(goal.goal.phase)),
    ];
    if let Some(reason) = &goal.goal.blocked_reason {
        lines.push(format!("Blocker: {}: {}", reason.code, reason.message));
    }
    lines.extend([
        format!("Objective: {}", goal.goal.objective),
        format!(
            "Rounds: {}/{}",
            goal.rounds_started, goal.goal.max_goal_rounds
        ),
        format!("Activation: {}", activation_label(goal.activation)),
        String::new(),
        format!("Commands: {}", command_hint(goal)),
    ]);
    CommandResult::success(Some(lines.join("\n")))
}

fn missing_goal(action: &str) -> CommandResult {
    CommandResult::error(format!(
        "No goal is currently set; /goal {action} requires one. {USAGE}"
    ))
}

fn submit_objective_attachments(invocation: &CommandInvocation) -> Result<(), BoxError> {
    if invocation.attachments.is_empty() {
        return Ok(());
    }
    let mut content = invocation.attachments.to_vec();
    content.push(ContentBlock::text(
        "Reference attachments for the goal objective.",
    ));
    invocation
        .agent
        .followup(create_user_message(NewUserMessage {
            content,
            source: hoh_llm::MessageSource::new("user"),
        }))
}

fn is_expected_goal_error(error: &(dyn std::error::Error + 'static)) -> bool {
    HarnessError::of(error).is_some_and(|error| {
        error.is_named(GoalError::NAME) && error.code() != "GOAL_STATE_CORRUPT"
    })
}

fn execute_goal_command(
    goals: &GoalService,
    invocation: &CommandInvocation,
) -> Result<CommandResult, BoxError> {
    let command = parse_goal_command(&invocation.raw_input);
    if !invocation.attachments.is_empty()
        && !matches!(command, GoalCommand::Create(_) | GoalCommand::Edit(_))
    {
        return Ok(CommandResult::error(
            "Attachments only accompany a goal objective: /goal <objective> or /goal edit <objective>.",
        ));
    }
    let execute = || -> Result<CommandResult, BoxError> {
        let current = goals.get(&invocation.agent)?;
        Ok(match command {
            GoalCommand::Show => current.map_or_else(
                || CommandResult::success(Some(format!("No goal is currently set.\n{USAGE}"))),
                |goal| render_goal("Goal", &goal),
            ),
            GoalCommand::InvalidEdit => CommandResult::error(format!(
                "Goal editing requires a replacement objective.\n{USAGE}"
            )),
            GoalCommand::Create(objective) => {
                if let Some(goal) = current
                    && goal.goal.phase != GoalPhase::Complete
                {
                    return Ok(CommandResult::error(format!(
                        "A goal is already {}. Use /goal edit <objective> to change it or /goal clear before replacing it.",
                        phase_label(goal.goal.phase)
                    )));
                }
                let created = goals.create(
                    &invocation.agent,
                    CreateGoalRequest {
                        objective,
                        max_goal_rounds: None,
                    },
                )?;
                submit_objective_attachments(invocation)?;
                render_goal("Goal created", &created)
            }
            GoalCommand::Edit(objective) => {
                let Some(current) = current else {
                    return Ok(missing_goal("edit"));
                };
                let updated = if current.goal.phase == GoalPhase::Complete {
                    goals.create(
                        &invocation.agent,
                        CreateGoalRequest {
                            objective,
                            max_goal_rounds: None,
                        },
                    )?
                } else {
                    goals.edit(
                        &invocation.agent,
                        &current.as_ref(),
                        EditGoalRequest {
                            objective: Some(objective),
                            max_goal_rounds: None,
                        },
                    )?
                };
                submit_objective_attachments(invocation)?;
                render_goal(
                    if current.goal.phase == GoalPhase::Complete {
                        "Goal created"
                    } else {
                        "Goal updated"
                    },
                    &updated,
                )
            }
            GoalCommand::Pause => {
                let Some(current) = current else {
                    return Ok(missing_goal("pause"));
                };
                render_goal(
                    "Goal paused",
                    &goals.pause(&invocation.agent, &current.as_ref())?,
                )
            }
            GoalCommand::Resume => {
                let Some(current) = current else {
                    return Ok(missing_goal("resume"));
                };
                render_goal(
                    "Goal resumed",
                    &goals.resume(&invocation.agent, &current.as_ref())?,
                )
            }
            GoalCommand::Clear => {
                let Some(current) = current else {
                    return Ok(CommandResult::success(Some("No goal to clear.")));
                };
                goals.clear(&invocation.agent, &current.as_ref())?;
                CommandResult::success(Some("Goal cleared."))
            }
        })
    };
    match execute() {
        Err(error) if is_expected_goal_error(error.as_ref()) => Ok(CommandResult::error(
            "The goal command is not valid for the current state. Run /goal to view available commands.",
        )),
        result => result,
    }
}

/// Register the Codex-shaped `/goal` command.
pub fn apply(
    ctx: &Context,
    commands: &hoh_commands::CommandRuntime,
    goals: GoalService,
) -> Result<(), BoxError> {
    let mut definition = CommandDefinition::new(COMMAND_NAME, DESCRIPTION, move |invocation| {
        let result = execute_goal_command(&goals, &invocation);
        async move { result }
    });
    definition.definition_id = Some(CommandDefinitionId::from(DEFINITION_ID));
    definition.input = Some(CommandInputDescriptor::new(INPUT_HINT).with_attachments());
    commands.register(ctx, definition)?;
    Ok(())
}

#[derive(Inject)]
pub struct Deps {
    commands: Service<Commands>,
    goals: Service<Goals>,
}

pub struct CommandGoal;

impl Plugin for CommandGoal {
    const NAME: &'static str = NAME;
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        apply(&ctx, &deps.commands, (*deps.goals).clone())
    }
}
