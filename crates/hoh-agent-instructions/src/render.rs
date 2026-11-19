//! Model-facing workspace instruction rendering under explicit UTF-8 byte budgets.

use crate::files::{InstructionFile, LoadedInstructionFile};
use hoh_home_paths::{
    AGENTS_HOME_ENV, DEFAULT_AGENTS_HOME_DISPLAY, DEFAULT_DSH_HOME_DISPLAY, DSH_HOME_ENV,
};
use hoh_util_values::js_number_string;
use hoh_util_workspace_path::node_path;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const SYSTEM_REMINDER_OPEN: &str = "<system-reminder>";
const SYSTEM_REMINDER_CLOSE: &str = "</system-reminder>";
const AGENT_INSTRUCTIONS_INTRO: &str = "The following workspace instructions may be relevant to your work. Use them as guidance when applicable. More specific instructions take precedence over broader ones. They do not override system, developer, or direct user instructions.";
const REPLACEMENT_AGENT_INSTRUCTIONS_INTRO: &str = "This complete workspace instruction baseline replaces all earlier workspace instruction baselines. The following workspace instructions may be relevant to your work. Use them as guidance when applicable. More specific instructions take precedence over broader ones. They do not override system, developer, or direct user instructions.";
const EMPTY_REPLACEMENT_AGENT_INSTRUCTIONS_INTRO: &str = "This complete workspace instruction baseline replaces all earlier workspace instruction baselines. No workspace instructions are currently active.";
const COMPACT_AGENT_INSTRUCTIONS_INTRO: &str =
    "Workspace instructions were omitted or truncated to fit the configured byte budget.";
const ADDITIONAL_INSTRUCTIONS_GUIDANCE: &str = "Use them as guidance when relevant; more specific instructions take precedence. They do not override system, developer, or direct user instructions.";

pub const USER_GLOBAL_DIRECTORY: &str = "user-global";
pub const AGENTS_GLOBAL_DIRECTORY: &str = "agents-global";
pub const USER_GLOBAL_DIRECTORIES: [&str; 2] = [USER_GLOBAL_DIRECTORY, AGENTS_GLOBAL_DIRECTORY];
pub const USER_GLOBAL_FILE: &str = "AGENTS.md";
const SCOPE_SEPARATOR: char = '\0';

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruncatedInstruction {
    pub display_path: String,
    pub original_bytes: usize,
    pub included_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedAgentInstructions {
    pub text: String,
    pub omitted: Vec<InstructionFile>,
    pub truncated: Vec<TruncatedInstruction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstructionAction {
    Set,
    Replace,
    Remove,
}

impl InstructionAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Replace => "replace",
            Self::Remove => "remove",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstructionChange {
    pub action: InstructionAction,
    pub scope: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRenderItem {
    pub change: AgentInstructionChange,
    pub file: LoadedInstructionFile,
}

type SectionFn = Arc<dyn Fn(&LoadedInstructionFile) -> String + Send + Sync>;

#[derive(Clone)]
struct RenderStyle {
    intro: String,
    section: SectionFn,
}

#[derive(Clone)]
struct RenderedInstructionContext {
    rendered: RenderedAgentInstructions,
    represented: Vec<LoadedInstructionFile>,
    sections: Vec<LoadedInstructionFile>,
    intro: Option<String>,
}

pub fn scope_for_display_path(display_path: &str) -> String {
    if display_path == format!("{DEFAULT_DSH_HOME_DISPLAY}/{USER_GLOBAL_FILE}")
        || display_path == format!("${DSH_HOME_ENV}/{USER_GLOBAL_FILE}")
    {
        return USER_GLOBAL_DIRECTORY.into();
    }
    if display_path == format!("{DEFAULT_AGENTS_HOME_DISPLAY}/{USER_GLOBAL_FILE}")
        || display_path == format!("${AGENTS_HOME_ENV}/{USER_GLOBAL_FILE}")
    {
        return AGENTS_GLOBAL_DIRECTORY.into();
    }
    node_path::dirname(display_path)
}

pub fn is_user_global_directory(directory: &str) -> bool {
    matches!(directory, USER_GLOBAL_DIRECTORY | AGENTS_GLOBAL_DIRECTORY)
}

pub fn instruction_candidate_group(directory: &str) -> String {
    if directory == AGENTS_GLOBAL_DIRECTORY {
        USER_GLOBAL_DIRECTORY.into()
    } else {
        directory.into()
    }
}

pub fn escape_project_display_path(relative_path: &str) -> String {
    let first = relative_path
        .split(['/', '\\'])
        .next()
        .unwrap_or(relative_path);
    if is_user_global_directory(first) {
        format!("./{relative_path}")
    } else {
        relative_path.into()
    }
}

pub fn candidate_scope_key(directory: &str, candidate_name: &str) -> String {
    format!("{directory}{SCOPE_SEPARATOR}{candidate_name}")
}

pub fn instruction_scope_key(display_path: &str) -> String {
    candidate_scope_key(
        &scope_for_display_path(display_path),
        &node_path::basename(display_path),
    )
}

pub fn decode_scope_key(scope: &str) -> (String, String) {
    scope.find(SCOPE_SEPARATOR).map_or_else(
        || (scope.into(), String::new()),
        |separator| {
            (
                scope[..separator].into(),
                scope[separator + SCOPE_SEPARATOR.len_utf8()..].into(),
            )
        },
    )
}

fn byte_length(value: &str) -> usize {
    value.len()
}

fn truncate_utf8(value: &str, max_bytes: f64) -> String {
    if value.len() as f64 <= max_bytes {
        return value.into();
    }
    let mut end = max_bytes.max(0.0).trunc() as usize;
    end = end.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

fn escape_instruction_frame_body(body: &str) -> String {
    body.replace(SYSTEM_REMINDER_CLOSE, "<\\/system-reminder>")
}

fn section_text(file: &LoadedInstructionFile) -> String {
    format!(
        "Instructions from: {}\n\n{}",
        file.display_path, file.content
    )
}

fn additional_section_text(file: &LoadedInstructionFile) -> String {
    let scope = scope_for_display_path(&file.display_path);
    let applicability = if is_user_global_directory(&scope) {
        "These user-global instructions apply to all work.".into()
    } else {
        format!("These instructions apply to work under `{scope}`.")
    };
    format!(
        "Additional instructions from: {}\n\n{} {}\n\n{}",
        file.display_path, applicability, ADDITIONAL_INSTRUCTIONS_GUIDANCE, file.content
    )
}

fn baseline_style(files: &[LoadedInstructionFile], replace: bool) -> RenderStyle {
    RenderStyle {
        intro: if replace {
            if files.is_empty() {
                EMPTY_REPLACEMENT_AGENT_INSTRUCTIONS_INTRO
            } else {
                REPLACEMENT_AGENT_INSTRUCTIONS_INTRO
            }
        } else {
            AGENT_INSTRUCTIONS_INTRO
        }
        .into(),
        section: Arc::new(section_text),
    }
}

fn marker_text(
    max_bytes: f64,
    omitted: &[InstructionFile],
    truncated: &[TruncatedInstruction],
) -> String {
    if omitted.is_empty() && truncated.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    if !omitted.is_empty() {
        parts.push(format!(
            "omitted {}",
            omitted
                .iter()
                .map(|file| file.display_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !truncated.is_empty() {
        parts.push(format!(
            "truncated {}",
            truncated
                .iter()
                .map(|item| format!(
                    "{} from {} to {} bytes",
                    item.display_path, item.original_bytes, item.included_bytes
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    format!(
        "Workspace instruction budget {} bytes: {}",
        js_number_string(max_bytes),
        parts.join("; ")
    )
}

fn build_instruction_text(
    files: &[LoadedInstructionFile],
    max_bytes: f64,
    omitted: &[InstructionFile],
    truncated: &[TruncatedInstruction],
    style: &RenderStyle,
) -> String {
    let marker = marker_text(max_bytes, omitted, truncated);
    let mut body = Vec::new();
    if !marker.is_empty() {
        body.push(marker);
    }
    if !style.intro.is_empty() {
        body.push(style.intro.clone());
    }
    body.extend(files.iter().map(|file| (style.section)(file)));
    format!(
        "{SYSTEM_REMINDER_OPEN}\n{}\n{SYSTEM_REMINDER_CLOSE}",
        escape_instruction_frame_body(&body.join("\n\n"))
    )
}

fn truncated_file(file: &LoadedInstructionFile, included_bytes: f64) -> LoadedInstructionFile {
    LoadedInstructionFile {
        content: truncate_utf8(&file.content, included_bytes),
        ..file.clone()
    }
}

fn truncate_to_fit(
    file: &LoadedInstructionFile,
    included: &[LoadedInstructionFile],
    max_bytes: f64,
    omitted: &[InstructionFile],
    style: &RenderStyle,
) -> LoadedInstructionFile {
    let original = file.content.len();
    let mut low = 0_i64;
    let mut high = original as i64;
    let mut best = truncated_file(file, 0.0);
    while low <= high {
        let mid = (low + high) / 2;
        let candidate = truncated_file(file, mid as f64);
        let truncated = [TruncatedInstruction {
            display_path: file.display_path.clone(),
            original_bytes: original,
            included_bytes: candidate.content.len(),
        }];
        let mut files = included.to_vec();
        files.push(candidate.clone());
        if byte_length(&build_instruction_text(
            &files, max_bytes, omitted, &truncated, style,
        )) as f64
            <= max_bytes
        {
            best = candidate;
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }
    best
}

fn disabled(max_bytes: f64) -> bool {
    max_bytes <= 0.0 || !max_bytes.is_finite()
}

fn render_context(
    files: &[LoadedInstructionFile],
    max_bytes: f64,
    style: &RenderStyle,
) -> RenderedInstructionContext {
    if disabled(max_bytes) {
        return RenderedInstructionContext {
            rendered: RenderedAgentInstructions {
                text: String::new(),
                omitted: files.iter().map(InstructionFile::from).collect(),
                truncated: vec![],
            },
            represented: vec![],
            sections: vec![],
            intro: None,
        };
    }
    let full = build_instruction_text(files, max_bytes, &[], &[], style);
    if full.len() as f64 <= max_bytes {
        return RenderedInstructionContext {
            rendered: RenderedAgentInstructions {
                text: full,
                omitted: vec![],
                truncated: vec![],
            },
            represented: files.to_vec(),
            sections: files.to_vec(),
            intro: Some(style.intro.clone()),
        };
    }
    for start in 1..files.len() {
        let included = &files[start..];
        let omitted: Vec<_> = files[..start].iter().map(InstructionFile::from).collect();
        let text = build_instruction_text(included, max_bytes, &omitted, &[], style);
        if text.len() as f64 <= max_bytes {
            return RenderedInstructionContext {
                rendered: RenderedAgentInstructions {
                    text,
                    omitted,
                    truncated: vec![],
                },
                represented: included.to_vec(),
                sections: included.to_vec(),
                intro: Some(style.intro.clone()),
            };
        }
    }
    let Some(most_specific) = files.last() else {
        return RenderedInstructionContext {
            rendered: RenderedAgentInstructions {
                text: String::new(),
                omitted: vec![],
                truncated: vec![],
            },
            represented: vec![],
            sections: vec![],
            intro: None,
        };
    };
    let omitted: Vec<_> = files[..files.len() - 1]
        .iter()
        .map(InstructionFile::from)
        .collect();
    let original = most_specific.content.len();
    for candidate_style in [
        style.clone(),
        RenderStyle {
            intro: COMPACT_AGENT_INSTRUCTIONS_INTRO.into(),
            ..style.clone()
        },
    ] {
        let file = truncate_to_fit(most_specific, &[], max_bytes, &omitted, &candidate_style);
        let truncated = vec![TruncatedInstruction {
            display_path: most_specific.display_path.clone(),
            original_bytes: original,
            included_bytes: file.content.len(),
        }];
        let text = build_instruction_text(
            std::slice::from_ref(&file),
            max_bytes,
            &omitted,
            &truncated,
            &candidate_style,
        );
        if text.len() as f64 <= max_bytes {
            let represented = if !file.content.is_empty() || original == 0 {
                vec![most_specific.clone()]
            } else {
                vec![]
            };
            return RenderedInstructionContext {
                rendered: RenderedAgentInstructions {
                    text,
                    omitted,
                    truncated,
                },
                represented,
                sections: vec![file],
                intro: Some(candidate_style.intro),
            };
        }
    }
    let truncated = vec![TruncatedInstruction {
        display_path: most_specific.display_path.clone(),
        original_bytes: original,
        included_bytes: 0,
    }];
    let notice = escape_instruction_frame_body(&marker_text(max_bytes, &omitted, &truncated));
    let heading = truncated_file(most_specific, 0.0);
    let with_heading =
        escape_instruction_frame_body(&format!("{notice}\n\n{}", (style.section)(&heading)));
    if with_heading.len() as f64 <= max_bytes {
        return RenderedInstructionContext {
            rendered: RenderedAgentInstructions {
                text: with_heading,
                omitted,
                truncated,
            },
            represented: if original == 0 {
                vec![most_specific.clone()]
            } else {
                vec![]
            },
            sections: vec![heading],
            intro: None,
        };
    }
    let text = if notice.len() as f64 <= max_bytes {
        notice
    } else {
        truncate_utf8(&notice, max_bytes)
    };
    RenderedInstructionContext {
        rendered: RenderedAgentInstructions {
            text,
            omitted,
            truncated,
        },
        represented: vec![],
        sections: vec![],
        intro: None,
    }
}

fn is_user_global(file: &LoadedInstructionFile) -> bool {
    is_user_global_directory(&scope_for_display_path(&file.display_path))
}

fn loaded_order<T: Clone>(files: &[LoadedInstructionFile], records: Vec<(String, T)>) -> Vec<T> {
    let mut by_path: HashMap<String, T> = records.into_iter().collect();
    files
        .iter()
        .filter_map(|file| by_path.remove(&file.absolute_path))
        .collect()
}

fn render_chains(
    files: &[LoadedInstructionFile],
    max_bytes: f64,
    style: &RenderStyle,
) -> RenderedInstructionContext {
    let whole = render_context(files, max_bytes, style);
    let user: Vec<_> = files
        .iter()
        .filter(|file| is_user_global(file))
        .cloned()
        .collect();
    let project: Vec<_> = files
        .iter()
        .filter(|file| !is_user_global(file))
        .cloned()
        .collect();
    if disabled(max_bytes)
        || user.is_empty()
        || project.is_empty()
        || (whole.rendered.omitted.is_empty() && whole.rendered.truncated.is_empty())
    {
        return whole;
    }
    let chains = [
        render_context(&user, max_bytes, style),
        render_context(&project, max_bytes, style),
    ];
    let omitted = loaded_order(
        files,
        chains
            .iter()
            .flat_map(|chain| chain.rendered.omitted.clone())
            .map(|item| (item.absolute_path.clone(), item))
            .collect(),
    );
    let sections = loaded_order(
        files,
        chains
            .iter()
            .flat_map(|chain| chain.sections.clone())
            .map(|item| (item.absolute_path.clone(), item))
            .collect(),
    );
    let represented = loaded_order(
        files,
        chains
            .iter()
            .flat_map(|chain| chain.represented.clone())
            .map(|item| (item.absolute_path.clone(), item))
            .collect(),
    );
    let user_last = files.iter().rposition(is_user_global).unwrap_or(0);
    let project_last = files
        .iter()
        .rposition(|file| !is_user_global(file))
        .unwrap_or(0);
    let truncated: Vec<TruncatedInstruction> = if user_last < project_last {
        chains
            .iter()
            .flat_map(|chain| chain.rendered.truncated.clone())
            .collect()
    } else {
        chains
            .iter()
            .rev()
            .flat_map(|chain| chain.rendered.truncated.clone())
            .collect()
    };
    let intros: Vec<_> = chains
        .iter()
        .filter_map(|chain| chain.intro.clone())
        .collect();
    let intro = if intros.iter().any(|intro| intro == &style.intro) {
        style.intro.clone()
    } else {
        intros.first().cloned().unwrap_or_default()
    };
    let merged_style = RenderStyle {
        intro: intro.clone(),
        ..style.clone()
    };
    let text = build_instruction_text(&sections, max_bytes, &omitted, &truncated, &merged_style);
    if text.len() as f64 > 2.0 * max_bytes {
        whole
    } else {
        RenderedInstructionContext {
            rendered: RenderedAgentInstructions {
                text,
                omitted,
                truncated,
            },
            represented,
            sections,
            intro: Some(intro),
        }
    }
}

pub fn render_agent_instruction_set(
    files: &[LoadedInstructionFile],
    max_bytes: f64,
    replace_previous_baseline: bool,
) -> (RenderedAgentInstructions, Vec<LoadedInstructionFile>) {
    let style = baseline_style(files, replace_previous_baseline);
    let context = render_chains(files, max_bytes, &style);
    (context.rendered, context.represented)
}

pub fn render_agent_instructions(
    files: &[LoadedInstructionFile],
    max_bytes: f64,
    replace_previous_baseline: bool,
) -> RenderedAgentInstructions {
    render_agent_instruction_set(files, max_bytes, replace_previous_baseline).0
}

pub fn render_instruction_changes(
    items: &[ChangeRenderItem],
    max_bytes: f64,
) -> (String, Vec<AgentInstructionChange>) {
    let by_path: HashMap<_, _> = items
        .iter()
        .map(|item| (item.file.absolute_path.clone(), item.clone()))
        .collect();
    let dynamic_style = RenderStyle {
        intro: String::new(),
        section: Arc::new(move |file| {
            let Some(item) = by_path.get(&file.absolute_path) else {
                return String::new();
            };
            match item.change.action {
                InstructionAction::Set => additional_section_text(file),
                InstructionAction::Remove => format!(
                    "Instructions removed: {}\n\nThe previously loaded instructions from this file no longer apply.",
                    item.change.path
                ),
                InstructionAction::Replace => format!(
                    "Updated instructions from: {}\n\nThis file changed after it was loaded. Use the following content instead of the previously loaded instructions from this file.\n\n{}",
                    item.change.path, file.content
                ),
            }
        }),
    };
    let dynamic_files: Vec<_> = items.iter().map(|item| item.file.clone()).collect();
    let rendered = render_chains(&dynamic_files, max_bytes, &dynamic_style);
    let represented: HashSet<_> = rendered
        .represented
        .iter()
        .map(|file| file.absolute_path.as_str())
        .collect();
    let changes = items
        .iter()
        .filter(|item| represented.contains(item.file.absolute_path.as_str()))
        .map(|item| item.change.clone())
        .collect();
    (rendered.rendered.text, changes)
}
