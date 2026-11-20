use hoh_llm::UserMessage;
use hoh_util_values::is_js_white_space;

/// `/name` gesture tokens from claimed user messages, deduplicated in first-seen order.
pub(crate) fn invoked_skill_names(messages: &[UserMessage]) -> Vec<String> {
    let mut names = Vec::new();
    for message in messages {
        if message.source.kind != "user" {
            continue;
        }
        for block in &message.content {
            if block.r#type != "text" {
                continue;
            }
            let Some(text) = block.text_value() else {
                continue;
            };
            scan(text, &mut names);
        }
    }
    names
}

fn scan(text: &str, names: &mut Vec<String>) {
    for (slash, character) in text.char_indices() {
        if character != '/' {
            continue;
        }
        let prefix = &text[..slash];
        if prefix
            .chars()
            .next_back()
            .is_some_and(|character| !is_js_white_space(character))
        {
            continue;
        }
        let rest = &text[slash + 1..];
        let mut end = 0;
        for (offset, character) in rest.char_indices() {
            if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-' {
                end = offset + character.len_utf8();
            } else {
                break;
            }
        }
        if end == 0 {
            continue;
        }
        let name = &rest[..end];
        if !hoh_skill::is_skill_name(name) {
            continue;
        }
        if rest[end..]
            .chars()
            .next()
            .is_some_and(|character| !is_js_white_space(character))
        {
            continue;
        }
        if !names.iter().any(|existing| existing == name) {
            names.push(name.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};

    fn user(text: &str) -> UserMessage {
        create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(text)],
            source: MessageSource::new("user"),
        })
    }

    #[test]
    fn recognizes_whitespace_bounded_tokens_and_deduplicates() {
        let messages = [user("use /alpha now /alpha\n/beta"), user("/gamma")];
        assert_eq!(invoked_skill_names(&messages), ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn rejects_paths_fractions_and_broken_boundaries() {
        let messages = [user(
            "/usr/bin 5/8 //alpha x/alpha /alpha, /UPPER /bad--name",
        )];
        assert!(invoked_skill_names(&messages).is_empty());
    }

    #[test]
    fn scans_only_text_blocks_from_direct_user_sources() {
        let forged = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("/forged")],
            source: MessageSource::new("skill-catalog"),
        });
        let mixed = create_user_message(NewUserMessage {
            content: vec![
                ContentBlock::reasoning("/hidden"),
                ContentBlock::text("/visible"),
            ],
            source: MessageSource::new("user"),
        });
        assert_eq!(invoked_skill_names(&[forged, mixed]), ["visible"]);
    }
}
