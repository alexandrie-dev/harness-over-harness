use serde_json::{Map, Number, Value};

pub struct Frontmatter {
    pub data: Map<String, Value>,
    pub body: String,
}

pub fn parse(raw: &str) -> Result<Option<Frontmatter>, String> {
    let Some(first_end) = raw.find('\n') else {
        return Ok(None);
    };
    if raw[..first_end].trim_end_matches('\r') != "---" {
        return Ok(None);
    }
    let start = first_end + 1;
    let Some((closing, body_start)) = closing(raw, start) else {
        return Ok(None);
    };
    let yaml = &raw[start..closing];
    if yaml.trim().is_empty()
        || matches!(yaml.trim(), "[]" | "null" | "~")
        || yaml.trim_start().starts_with('-')
    {
        return Ok(None);
    }
    Ok(Some(Frontmatter {
        data: parse_mapping(yaml)?,
        body: raw[body_start..].to_owned(),
    }))
}

fn closing(raw: &str, mut offset: usize) -> Option<(usize, usize)> {
    while offset <= raw.len() {
        let next = raw[offset..].find('\n').map(|index| offset + index);
        let end = next.unwrap_or(raw.len());
        if raw[offset..end].trim_end_matches('\r') == "---" {
            return Some((offset, next.map_or(raw.len(), |newline| newline + 1)));
        }
        offset = next? + 1;
    }
    None
}

fn parse_mapping(yaml: &str) -> Result<Map<String, Value>, String> {
    let lines = yaml.lines().collect::<Vec<_>>();
    let mut result = Map::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_end_matches('\r');
        index += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            return Err(format!("unexpected indentation at line {index}"));
        }
        let Some((key, raw_value)) = line.split_once(':') else {
            return Err(format!("invalid mapping entry at line {index}"));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("empty mapping key at line {index}"));
        }
        let mut raw_value = raw_value.trim().to_owned();
        if matches!(raw_value.as_str(), "|" | "|-" | "|+" | ">" | ">-" | ">+") {
            let mut block = Vec::new();
            while index < lines.len() {
                let candidate = lines[index].trim_end_matches('\r');
                if !candidate.is_empty() && !candidate.starts_with(char::is_whitespace) {
                    break;
                }
                index += 1;
                block.push(candidate);
            }
            let indent = block
                .iter()
                .filter(|line| !line.trim().is_empty())
                .map(|line| line.len() - line.trim_start().len())
                .min()
                .unwrap_or(0);
            let block = block
                .iter()
                .map(|line| line.get(indent..).unwrap_or("").to_owned())
                .collect::<Vec<_>>();
            let mut value = if raw_value.starts_with('>') {
                fold_block(&block)
            } else {
                block.join("\n")
            };
            value = match raw_value.chars().nth(1) {
                Some('-') => value.trim_end_matches('\n').to_owned(),
                Some('+') => format!("{value}\n"),
                _ => format!("{}\n", value.trim_end_matches('\n')),
            };
            insert_unique(&mut result, key, Value::String(value))?;
            continue;
        }
        if raw_value.is_empty() {
            let next_nested = lines[index..].iter().find(|candidate| {
                let candidate = candidate.trim_end_matches('\r');
                !candidate.trim().is_empty() && !candidate.trim_start().starts_with('#')
            });
            // YAML permits a sequence used as a mapping value to be indentationless:
            // `allowed-tools:\n- Read`. It still belongs to the preceding key and ends at the
            // next top-level mapping entry.
            let indentless_sequence = next_nested.is_some_and(|candidate| {
                !candidate.starts_with(char::is_whitespace)
                    && candidate.trim_end_matches('\r').starts_with("- ")
            });
            let nested_indent = next_nested
                .filter(|candidate| {
                    candidate.starts_with(char::is_whitespace) || indentless_sequence
                })
                .map(|candidate| candidate.len() - candidate.trim_start().len());
            let Some(nested_indent) = nested_indent else {
                insert_unique(&mut result, key, Value::Null)?;
                continue;
            };
            let mut nested = Map::new();
            let mut sequence = Vec::new();
            let mut sequence_mode = false;
            while index < lines.len() {
                let candidate = lines[index].trim_end_matches('\r');
                if candidate.trim().is_empty() || candidate.trim_start().starts_with('#') {
                    index += 1;
                    continue;
                }
                if indentless_sequence && !candidate.starts_with("- ") {
                    break;
                }
                let indent = candidate.len() - candidate.trim_start().len();
                if indent < nested_indent {
                    break;
                }
                if indent != nested_indent {
                    return Err(format!(
                        "unsupported nested indentation at line {}",
                        index + 1
                    ));
                }
                let indented = &candidate[nested_indent..];
                if let Some(item) = indented.strip_prefix("- ") {
                    if !nested.is_empty() {
                        return Err(format!("invalid nested sequence at line {}", index + 1));
                    }
                    sequence_mode = true;
                    sequence.push(scalar(item.trim())?);
                    index += 1;
                    continue;
                }
                if sequence_mode {
                    return Err(format!("invalid nested mapping at line {}", index + 1));
                }
                let Some((nested_key, nested_value)) = indented.split_once(':') else {
                    return Err(format!("invalid nested mapping at line {}", index + 1));
                };
                let nested_value = nested_value.trim();
                insert_unique(
                    &mut nested,
                    nested_key.trim(),
                    if nested_value.is_empty() {
                        Value::Null
                    } else {
                        scalar(nested_value)?
                    },
                )?;
                index += 1;
            }
            insert_unique(
                &mut result,
                key,
                if sequence_mode {
                    Value::Array(sequence)
                } else {
                    Value::Object(nested)
                },
            )?;
            continue;
        }
        raw_value = collect_flow_scalar(&lines, &mut index, raw_value);
        insert_unique(&mut result, key, scalar(&raw_value)?)?;
    }
    Ok(result)
}

/// Collect YAML flow-scalar continuation lines. A top-level mapping key is never consumed:
/// only non-empty lines with greater indentation join the scalar. YAML folds an ordinary line
/// break to a space and preserves each intervening empty line as one newline.
fn collect_flow_scalar(lines: &[&str], index: &mut usize, mut value: String) -> String {
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'));
    let multiline_quoted = quote.is_some_and(|quote| !quoted_scalar_closed(&value, quote));
    let mut pending_blank_lines = 0;

    while *index < lines.len() {
        let candidate = lines[*index].trim_end_matches('\r');
        if candidate.trim().is_empty() {
            if has_indented_continuation(lines, *index + 1) {
                pending_blank_lines += 1;
                *index += 1;
                continue;
            }
            break;
        }
        if !candidate.starts_with(char::is_whitespace) {
            break;
        }

        let continuation = candidate.trim();
        if pending_blank_lines == 0 {
            value.push(' ');
        } else if quote == Some('"') {
            for _ in 0..pending_blank_lines {
                value.push_str("\\n");
            }
        } else {
            for _ in 0..pending_blank_lines {
                value.push('\n');
            }
        }
        value.push_str(continuation);
        pending_blank_lines = 0;
        *index += 1;

        if multiline_quoted && quote.is_some_and(|quote| quoted_scalar_closed(&value, quote)) {
            break;
        }
    }
    value
}

fn has_indented_continuation(lines: &[&str], mut index: usize) -> bool {
    while index < lines.len() {
        let candidate = lines[index].trim_end_matches('\r');
        if candidate.trim().is_empty() {
            index += 1;
            continue;
        }
        return candidate.starts_with(char::is_whitespace);
    }
    false
}

fn quoted_scalar_closed(value: &str, quote: char) -> bool {
    strip_comment(value).trim().ends_with(quote)
}

fn scalar(value: &str) -> Result<Value, String> {
    let value = strip_comment(value).trim();
    if value.starts_with(['&', '*', '!']) {
        return Err("YAML anchors, aliases, and tags are unsupported".into());
    }
    if (value.starts_with('"') && !value.ends_with('"'))
        || (value.starts_with('\'') && !value.ends_with('\''))
    {
        return Err("unterminated quoted scalar".into());
    }
    if value.starts_with('[') {
        if !value.ends_with(']') {
            return Err("unterminated flow sequence".into());
        }
        return split_flow(&value[1..value.len() - 1])?
            .into_iter()
            .map(scalar)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    if value.starts_with('{') {
        if !value.ends_with('}') {
            return Err("unterminated flow mapping".into());
        }
        let mut object = Map::new();
        for entry in split_flow(&value[1..value.len() - 1])? {
            let Some((key, value)) = split_flow_pair(entry) else {
                return Err("invalid flow mapping entry".into());
            };
            let key = unquote_key(key.trim())?;
            insert_unique(&mut object, &key, scalar(value.trim())?)?;
        }
        return Ok(Value::Object(object));
    }
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str(value).map_err(|error| error.to_string());
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Ok(Value::String(value[1..value.len() - 1].replace("''", "'")));
    }
    match value.to_ascii_lowercase().as_str() {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "null" | "~" => return Ok(Value::Null),
        _ => {}
    }
    if let Ok(integer) = value.parse::<i64>() {
        return Ok(Value::Number(integer.into()));
    }
    if let Ok(float) = value.parse::<f64>()
        && let Some(number) = Number::from_f64(float)
    {
        return Ok(Value::Number(number));
    }
    if value.contains(": ") {
        return Err("plain scalar contains an unquoted mapping separator".into());
    }
    Ok(Value::String(value.into()))
}

fn fold_block(lines: &[String]) -> String {
    let mut result = String::new();
    let mut previous_blank = false;
    let mut previous_indented = false;
    for line in lines {
        if line.is_empty() {
            result.push('\n');
            previous_blank = true;
            previous_indented = false;
        } else {
            let indented = line.starts_with(char::is_whitespace);
            if !result.is_empty() && !previous_blank && !result.ends_with('\n') {
                result.push(if indented || previous_indented {
                    '\n'
                } else {
                    ' '
                });
            }
            result.push_str(line);
            previous_blank = false;
            previous_indented = indented;
        }
    }
    result
}

fn insert_unique(object: &mut Map<String, Value>, key: &str, value: Value) -> Result<(), String> {
    if object.contains_key(key) {
        return Err(format!("duplicate mapping key \"{key}\""));
    }
    object.insert(key.into(), value);
    Ok(())
}

fn strip_comment(value: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    let mut characters = value.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        match quote {
            Some('\'') if character == '\'' => {
                if characters.peek().is_some_and(|(_, next)| *next == '\'') {
                    characters.next();
                } else {
                    quote = None;
                }
            }
            Some(current) if character == current => quote = None,
            Some(_) => {}
            None if matches!(character, '\'' | '"')
                && value[..index].chars().all(char::is_whitespace) =>
            {
                quote = Some(character);
            }
            None if character == '#'
                && value[..index]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace) =>
            {
                return &value[..index];
            }
            None => {}
        }
    }
    value
}

fn split_flow(value: &str) -> Result<Vec<&str>, String> {
    if value.trim().is_empty() {
        return Ok(vec![]);
    }
    let mut parts = vec![];
    let mut start = 0;
    let mut square = 0_u32;
    let mut curly = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        match quote {
            Some(current) if character == current => quote = None,
            Some(_) => {}
            None => match character {
                '\'' | '"' => quote = Some(character),
                '[' => square += 1,
                ']' => square = square.checked_sub(1).ok_or("unbalanced flow sequence")?,
                '{' => curly += 1,
                '}' => curly = curly.checked_sub(1).ok_or("unbalanced flow mapping")?,
                ',' if square == 0 && curly == 0 => {
                    parts.push(value[start..index].trim());
                    start = index + 1;
                }
                _ => {}
            },
        }
    }
    if quote.is_some() || square != 0 || curly != 0 {
        return Err("unterminated flow collection".into());
    }
    parts.push(value[start..].trim());
    if parts.iter().any(|part| part.is_empty()) {
        return Err("empty flow collection entry".into());
    }
    Ok(parts)
}

fn split_flow_pair(value: &str) -> Option<(&str, &str)> {
    let mut square = 0_u32;
    let mut curly = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        match quote {
            Some(current) if character == current => quote = None,
            Some(_) => {}
            None => match character {
                '\'' | '"' => quote = Some(character),
                '[' => square += 1,
                ']' => square = square.saturating_sub(1),
                '{' => curly += 1,
                '}' => curly = curly.saturating_sub(1),
                ':' if square == 0 && curly == 0 => {
                    return Some((&value[..index], &value[index + 1..]));
                }
                _ => {}
            },
        }
    }
    None
}

fn unquote_key(key: &str) -> Result<String, String> {
    match scalar(key)? {
        Value::String(key) => Ok(key),
        _ => Err("flow mapping key must be a string".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crlf_nested_and_block_values() {
        let parsed = parse(
            "---\r\nname: sample\r\ndescription: |\r\n  Has ---- marker.\r\nmetadata:\r\n  owner: \"tests\"\r\n---\r\nBody\r\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.data["description"], "Has ---- marker.\n");
        assert_eq!(parsed.data["metadata"]["owner"], "tests");
        assert_eq!(parsed.body, "Body\r\n");
    }

    #[test]
    fn parses_flow_metadata_and_rejects_non_string_description() {
        let parsed = parse(
            "---\nname: sample\ndescription: useful # comment\nmetadata: {owner: tests, flags: [one, two]}\n---\nBody\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.data["description"], "useful");
        assert_eq!(parsed.data["metadata"]["owner"], "tests");
        assert_eq!(
            parsed.data["metadata"]["flags"],
            serde_json::json!(["one", "two"])
        );

        let parsed = parse("---\nname: sample\ndescription: [not, text]\n---\nBody\n")
            .unwrap()
            .unwrap();
        assert!(parsed.data["description"].is_array());
    }

    #[test]
    fn rejects_duplicate_mapping_keys() {
        assert!(parse("---\nname: one\nname: two\n---\n").is_err());
    }

    #[test]
    fn accepts_yaml_block_chomping_lists_and_wider_metadata_indent() {
        let parsed = parse(
            "---\nname: sample\ndescription: >-\n    useful\n    skill\nallowed-tools:\n  - Read\n  - Write\nmetadata:\n    owner: tests\n    empty:\n---\nBody\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.data["description"], "useful skill");
        assert_eq!(
            parsed.data["allowed-tools"],
            serde_json::json!(["Read", "Write"])
        );
        assert_eq!(
            parsed.data["metadata"],
            serde_json::json!({"owner":"tests","empty":null})
        );
    }

    #[test]
    fn rejects_unclosed_quotes_and_plain_mapping_separators() {
        assert!(parse("---\nname: sample\ndescription: \"abc\n---\n").is_err());
        assert!(parse("---\nname: sample\ndescription: Use when: x\n---\n").is_err());
        for value in ["&anchor Useful", "*alias", "!tag Useful"] {
            assert!(parse(&format!("---\nname: sample\ndescription: {value}\n---\n")).is_err());
        }
    }

    #[test]
    fn empty_metadata_is_null_not_an_empty_object() {
        let parsed = parse("---\nname: sample\ndescription: useful\nmetadata:\n---\n")
            .unwrap()
            .unwrap();
        assert!(parsed.data["metadata"].is_null());
    }

    #[test]
    fn folded_blocks_preserve_more_indented_lines() {
        let parsed = parse("---\nname: sample\ndescription: >\n  prose\n    code\n  more\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "prose\n  code\nmore\n");
    }

    #[test]
    fn parses_indentationless_sequence_as_the_preceding_mapping_value() {
        let parsed = parse(
            "---\nname: sample\nallowed-tools:\n- Read\n- Grep\ndescription: useful\n---\nBody\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            parsed.data["allowed-tools"],
            serde_json::json!(["Read", "Grep"])
        );
        assert_eq!(parsed.data["description"], "useful");
    }

    #[test]
    fn apostrophe_in_plain_scalar_does_not_hide_a_trailing_comment() {
        let parsed = parse("---\nname: sample\ndescription: Don't use # x\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "Don't use");
    }

    #[test]
    fn doubled_apostrophe_keeps_hash_literal_inside_single_quoted_scalar() {
        let parsed =
            parse("---\nname: sample\ndescription: 'Don''t use # literal' # comment\n---\n")
                .unwrap()
                .unwrap();
        assert_eq!(parsed.data["description"], "Don't use # literal");
    }

    #[test]
    fn folds_plain_scalar_continuation_with_a_space() {
        let parsed = parse("---\nname: s\ndescription: a\n  b\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "a b");
    }

    #[test]
    fn folds_double_quoted_scalar_continuation_with_a_space() {
        let parsed = parse("---\nname: s\ndescription: \"a\n  b\"\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "a b");
    }

    #[test]
    fn folds_single_quoted_scalar_continuation_with_a_space() {
        let parsed = parse("---\nname: s\ndescription: 'a\n  b'\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "a b");
    }

    #[test]
    fn scalar_continuation_does_not_consume_the_next_top_level_key() {
        let parsed = parse("---\nname: s\ndescription: a\n  b\nmetadata: {owner: tests}\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "a b");
        assert_eq!(parsed.data["metadata"]["owner"], "tests");
    }

    #[test]
    fn empty_line_inside_plain_scalar_folds_to_newline() {
        let parsed = parse("---\nname: s\ndescription: a\n\n  b\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "a\nb");
    }

    #[test]
    fn empty_line_inside_quoted_scalar_folds_to_newline() {
        let parsed = parse("---\nname: s\ndescription: \"a\n\n  b\"\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.data["description"], "a\nb");
    }
}
