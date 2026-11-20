use std::collections::HashSet;

use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_llm::{
    ContentBlock, MessageId, MessageSource, NewUserMessage, UserMessage, create_user_message,
};
use hoh_session::SessionSeq;
use hoh_skill::{NativeError, SkillSummary, escape_text};
use hoh_util_values::{is_js_white_space, js_json_stringify};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{SkillCatalogEntry, SkillCatalogSource};

pub(crate) fn catalog_source_entries(
    skills: &[SkillSummary],
    description_max_length: f64,
) -> Vec<SkillCatalogEntry> {
    skills
        .iter()
        .map(|skill| SkillCatalogEntry {
            name: skill.text("name").unwrap_or_default().into(),
            description: catalog_description(
                skill.text("description").unwrap_or_default(),
                description_max_length,
            ),
        })
        .collect()
}

pub(crate) fn render_catalog_message(
    entries: Vec<SkillCatalogEntry>,
    incomplete: bool,
) -> UserMessage {
    let mut lines = vec![
        "<system-reminder>".into(),
        if entries.is_empty() {
            "No skills could be confirmed in this discovery attempt.".into()
        } else {
            "A skill is a reusable set of task-specific instructions. The following skills are available in this session:".into()
        },
        String::new(),
        "<available_skills>".into(),
    ];
    lines.extend(render_catalog_entries(&entries));
    lines.push("</available_skills>".into());
    lines.push(String::new());
    if incomplete {
        lines.push(incomplete_catalog_notice().into());
        lines.push(String::new());
    }
    lines.push(
        if entries.is_empty() {
            "Wait for an updated catalog before calling the `skill` tool. Do not infer skill instructions from an empty catalog."
        } else {
            "If the user names a skill, or the task clearly matches a skill's description, call the `skill` tool with the exact skill name before taking task actions. Load all applicable skills, then follow their full instructions. This catalog contains summaries only; do not infer or follow a skill's instructions until it has been loaded."
        }
        .into(),
    );
    lines.push("A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.".into());
    lines.push("</system-reminder>".into());
    catalog_message(lines.join("\n"), entries, false, incomplete)
}

pub(crate) fn render_catalog_update(
    entries: Vec<SkillCatalogEntry>,
    incomplete: bool,
) -> UserMessage {
    let mut lines = vec![
        "<system-reminder>".into(),
        if incomplete {
            "Skill discovery is incomplete. This catalog of currently available skills replaces every earlier available-skills list in this session:".into()
        } else {
            "The available skill catalog changed. This complete catalog replaces every earlier available-skills list in this session:".into()
        },
        String::new(),
        "<available_skills>".into(),
    ];
    lines.extend(render_catalog_entries(&entries));
    lines.push("</available_skills>".into());
    lines.push(String::new());
    if incomplete {
        lines.push(incomplete_catalog_notice().into());
        lines.push(String::new());
    }
    if entries.is_empty() {
        lines.push(
            if incomplete {
                "No skills could be confirmed in this discovery attempt. Do not use names from earlier skill catalogs."
            } else {
                "No skills are currently available through the `skill` tool. Do not use names from earlier skill catalogs."
            }
            .into(),
        );
        lines.push("A user may still invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool for it.".into());
    } else {
        lines.push("Use only names in this replacement catalog. If the user names a listed skill, or the task clearly matches its description, call the `skill` tool with the exact name before acting.".into());
        lines.push("A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.".into());
    }
    lines.push("</system-reminder>".into());
    catalog_message(lines.join("\n"), entries, true, incomplete)
}

fn incomplete_catalog_notice() -> &'static str {
    "Some skills could not be checked. Only confirmed skills are listed; a lower-priority definition of the same name may be selected when a preferred source is unavailable. Discovery will be retried on the next request."
}

fn render_catalog_entries(entries: &[SkillCatalogEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| format!("- `{}`: {}", entry.name, escape_text(&entry.description)))
        .collect()
}

fn catalog_message(
    text: String,
    entries: Vec<SkillCatalogEntry>,
    update: bool,
    incomplete: bool,
) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: SkillCatalogSource {
            kind: SkillCatalogSource::KIND.into(),
            form: SkillCatalogSource::FORM.into(),
            update: update.then_some(true),
            entries,
            incomplete: incomplete.then_some(true),
        }
        .into(),
    })
}

pub(crate) fn digest_catalog_entries(entries: &[SkillCatalogEntry], incomplete: bool) -> String {
    let canonical = entries
        .iter()
        .map(|entry| js_json_stringify(&json!([entry.name, entry.description])))
        .collect::<Vec<_>>()
        .join("\n");
    let mut hash = Sha256::new();
    if incomplete {
        hash.update(b"incomplete\n");
    }
    hash.update(canonical.as_bytes());
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) struct CatalogHistory {
    pub visible_digest: Option<String>,
    pub published: bool,
}

pub(crate) fn catalog_history(agent: &dyn Agent) -> Result<CatalogHistory, BoxError> {
    let visible: HashSet<SessionSeq> = agent.session().surface()?.nodes()?.into_iter().collect();
    let length = u64::from(agent.session().seq());
    let mut published = false;
    for index in (0..length).rev() {
        let seq = SessionSeq::new(index)?;
        let Some(event) = agent.session().event_at(seq) else {
            return Err(NativeError::Error(format!(
                "skill catalog cannot read seq {index} below the current Session length"
            ))
            .into());
        };
        if event.event_type() != "user/message"
            || event.data()["source"]["kind"] != SkillCatalogSource::KIND
        {
            continue;
        }
        let Some(entries) = read_catalog_entries(&event.data()["source"]) else {
            continue;
        };
        let incomplete = event.data()["source"]["incomplete"] == true;
        let digest = digest_catalog_entries(&entries, incomplete);
        published = true;
        if visible.contains(&event.seq()) {
            return Ok(CatalogHistory {
                visible_digest: Some(digest),
                published,
            });
        }
    }
    Ok(CatalogHistory {
        visible_digest: None,
        published,
    })
}

pub(crate) struct ProposedCatalog {
    pub message_id: MessageId,
    pub entries: Vec<SkillCatalogEntry>,
    pub incomplete: bool,
}

pub(crate) fn catalog_message_in(messages: &[UserMessage]) -> Option<ProposedCatalog> {
    for message in messages {
        if message.source.kind != SkillCatalogSource::KIND {
            continue;
        }
        let source = Value::Object({
            let mut source = message.source.data.clone();
            source.insert("kind".into(), Value::String(message.source.kind.clone()));
            source
        });
        if let Some(entries) = read_catalog_entries(&source) {
            return Some(ProposedCatalog {
                message_id: message.id.clone(),
                entries,
                incomplete: source["incomplete"] == true,
            });
        }
    }
    None
}

fn read_catalog_entries(source: &Value) -> Option<Vec<SkillCatalogEntry>> {
    let entries = source.get("entries")?.as_array()?;
    entries
        .iter()
        .map(|entry| {
            let name = entry.get("name")?.as_str()?;
            let description = entry.get("description")?.as_str()?;
            if name.is_empty() {
                return None;
            }
            Some(SkillCatalogEntry {
                name: name.into(),
                description: description.into(),
            })
        })
        .collect()
}

pub(crate) fn catalog_description(value: &str, max_length: f64) -> String {
    let mut normalized = String::new();
    let mut whitespace = false;
    for character in value.chars() {
        if is_js_white_space(character) {
            whitespace = !normalized.is_empty();
        } else {
            if whitespace {
                normalized.push(' ');
                whitespace = false;
            }
            normalized.push(character);
        }
    }
    if normalized.encode_utf16().count() as f64 <= max_length {
        return normalized;
    }
    let keep = (max_length - 3.0) as usize;
    let mut end = 0;
    let mut units = 0;
    for (offset, character) in normalized.char_indices() {
        let next = units + character.len_utf16();
        if next > keep {
            break;
        }
        units = next;
        end = offset + character.len_utf8();
    }
    format!("{}...", &normalized[..end])
}

impl From<SkillCatalogSource> for MessageSource {
    fn from(source: SkillCatalogSource) -> Self {
        let mut result = Self::new(SkillCatalogSource::KIND);
        result.data.insert(
            "form".into(),
            Value::String(SkillCatalogSource::FORM.into()),
        );
        if source.update == Some(true) {
            result.data.insert("update".into(), Value::Bool(true));
        }
        result.data.insert(
            "entries".into(),
            serde_json::to_value(source.entries).expect("catalog entries are JSON"),
        );
        if source.incomplete == Some(true) {
            result.data.insert("incomplete".into(), Value::Bool(true));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_js_whitespace_and_truncates_in_utf16_units() {
        assert_eq!(catalog_description("  one\u{feff}\ttwo  ", 50.0), "one two");
        assert_eq!(catalog_description("ab😀cd", 5.0), "ab...");
        assert_eq!(catalog_description("a😀bcd", 5.0), "a...");
    }

    #[test]
    fn digest_includes_boundaries_and_completeness() {
        let left = vec![SkillCatalogEntry {
            name: "a".into(),
            description: "b\nc".into(),
        }];
        let right = vec![SkillCatalogEntry {
            name: "a\nb".into(),
            description: "c".into(),
        }];
        assert_ne!(
            digest_catalog_entries(&left, false),
            digest_catalog_entries(&right, false)
        );
        assert_ne!(
            digest_catalog_entries(&left, false),
            digest_catalog_entries(&left, true)
        );
    }
}
