//! Source-span edits for the profile patch. Parsed/anchored patches never become the saved text.
use crate::LoadError;
use saphyr_parser::{Event, Parser, Span};
use serde_json::Value;
use std::{collections::VecDeque, ops::Range, path::Path};

struct Node {
    range: Range<usize>,
    column: usize,
    kind: Kind,
    flow: bool,
}
enum Kind {
    Atom,
    Map(Vec<(Node, Node)>),
    Seq(Vec<Node>),
}
struct Syntax<'a> {
    events: VecDeque<(Event<'a>, Span)>,
    bytes: Vec<usize>,
}
impl Syntax<'_> {
    fn node(&mut self) -> Result<Node, &'static str> {
        let (event, span) = self.events.pop_front().ok_or("expected YAML node")?;
        let start = self.bytes[span.start.index()];
        let mut end = self.bytes[span.end.index()];
        let kind = match event {
            Event::MappingStart(..) => {
                let mut fields = vec![];
                while !matches!(self.events.front(), Some((Event::MappingEnd, _))) {
                    fields.push((self.node()?, self.node()?));
                }
                let (_, span) = self.events.pop_front().ok_or("expected mapping end")?;
                end = self.bytes[span.end.index()];
                Kind::Map(fields)
            }
            Event::SequenceStart(..) => {
                let mut rows = vec![];
                while !matches!(self.events.front(), Some((Event::SequenceEnd, _))) {
                    rows.push(self.node()?);
                }
                let (_, span) = self.events.pop_front().ok_or("expected sequence end")?;
                end = self.bytes[span.end.index()];
                Kind::Seq(rows)
            }
            Event::Scalar(..) | Event::Alias(..) => Kind::Atom,
            _ => return Err("expected YAML node"),
        };
        Ok(Node {
            range: start..end,
            column: span.start.col(),
            kind,
            flow: false,
        })
    }
}
fn syntax(text: &str, file: &Path) -> Result<Node, LoadError> {
    let mut events = VecDeque::new();
    for event in Parser::new_from_str(text) {
        let (event, span) = event.map_err(|error| LoadError::Yaml {
            file: file.into(),
            line: error.marker().line(),
            column: error.marker().col() + 1,
            message: error.info().into(),
        })?;
        if !matches!(
            event,
            Event::StreamStart | Event::DocumentStart(..) | Event::DocumentEnd | Event::StreamEnd
        ) {
            events.push_back((event, span));
        }
    }
    let mut bytes: Vec<_> = text.char_indices().map(|(index, _)| index).collect();
    bytes.push(text.len());
    let mut root = Syntax { events, bytes }
        .node()
        .map_err(|message| shape(file, message))?;
    fn styles(text: &str, node: &mut Node) {
        node.flow = matches!(text.as_bytes().get(node.range.start), Some(b'{' | b'['));
        match &mut node.kind {
            Kind::Map(fields) => {
                for (key, value) in fields {
                    styles(text, key);
                    styles(text, value);
                }
            }
            Kind::Seq(rows) => {
                for row in rows {
                    styles(text, row);
                }
            }
            Kind::Atom => {}
        }
    }
    styles(text, &mut root);
    Ok(root)
}
fn shape(file: &Path, message: impl Into<String>) -> LoadError {
    LoadError::Shape {
        file: file.into(),
        line: 1,
        column: 1,
        message: message.into(),
    }
}
fn line_start(text: &str, index: usize) -> usize {
    text[..index].rfind('\n').map_or(0, |index| index + 1)
}
fn line_end(text: &str, index: usize) -> usize {
    text[index..]
        .find('\n')
        .map_or(text.len(), |end| index + end + 1)
}
fn end(text: &str, node: &Node) -> usize {
    if matches!(node.kind, Kind::Atom) {
        return node.range.start
            + text[node.range.clone()]
                .trim_end_matches([' ', '\t', '\r', '\n'])
                .len();
    }
    if node.flow {
        return node.range.end;
    }
    let mut boundary = node.range.end;
    let mut offset = line_end(text, node.range.start);
    while offset < boundary {
        let next = line_end(text, offset).min(boundary);
        let line = &text[offset..next];
        let content = line.trim_start_matches([' ', '\t']);
        let indent = line.len() - content.len();
        // Dedented comments belong to the following field/row, outside this node's span.
        if !content.trim_end_matches(['\r', '\n']).is_empty() && indent < node.column {
            boundary = offset;
            break;
        }
        offset = next;
    }
    let trimmed = text[node.range.start..boundary].trim_end_matches([' ', '\t', '\r', '\n']);
    node.range.start + trimmed.len()
}
fn indent(text: &str, n: usize) -> String {
    text.lines()
        .map(|line| format!("{}{line}", " ".repeat(n)))
        .collect::<Vec<_>>()
        .join("\n")
}
fn map_text(row: &Value, column: usize) -> String {
    let value = crate::yaml::write_yaml(row);
    let mut lines = value.trim_end_matches('\n').lines();
    let mut text = lines.next().unwrap_or("").to_owned();
    for line in lines {
        text.push('\n');
        text.push_str(&" ".repeat(column));
        text.push_str(line);
    }
    text
}
fn value_text(value: &Value, column: usize) -> String {
    let text = crate::yaml::write_yaml(value);
    if block_value(value) {
        format!("\n{}", indent(text.trim_end_matches('\n'), column + 2))
    } else {
        format!(" {}", text.trim_end_matches('\n'))
    }
}
fn block_value(value: &Value) -> bool {
    (value.as_object().is_some_and(|object| !object.is_empty())
        && crate::expression(value).is_none())
        || value.as_array().is_some_and(|array| !array.is_empty())
}
fn block_root(text: &str, root: &Node) -> String {
    let Kind::Seq(rows) = &root.kind else {
        return text.into();
    };
    if !root.flow {
        return text.into();
    }
    fn gap(text: &str, out: &mut String) -> String {
        let mut attributes = String::new();
        for line in text.split_inclusive('\n') {
            let line = line.trim_start_matches([',', ' ', '\t', '\r', '\n']);
            let comment = line.char_indices().find_map(|(index, ch)| {
                (ch == '#' && (index == 0 || matches!(line.as_bytes()[index - 1], b' ' | b'\t')))
                    .then_some(index)
            });
            let prefix = if let Some(index) = comment {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(&line[index..]);
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                &line[..index]
            } else {
                line
            };
            if !prefix.trim().is_empty() {
                attributes.push_str(prefix.trim());
                attributes.push(' ');
            }
        }
        attributes
    }
    let mut out = text[..root.range.start].to_owned();
    let mut cursor = root.range.start + 1;
    for row in rows {
        let attributes = gap(&text[cursor..row.range.start], &mut out);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("- ");
        out.push_str(&attributes);
        out.push_str(&text[row.range.clone()]);
        cursor = row.range.end;
    }
    gap(&text[cursor..root.range.end - 1], &mut out);
    if rows.is_empty() {
        out.push_str("[]");
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    let suffix = &text[root.range.end..];
    out.push_str(if rows.is_empty() {
        suffix
    } else {
        suffix.strip_prefix('\n').unwrap_or(suffix)
    });
    out
}

pub(crate) fn edit_config(
    text: &str,
    file: &Path,
    id: &str,
    name: &str,
    next: &Value,
    inherited: bool,
) -> Result<String, LoadError> {
    let raw = crate::yaml::parse_value(text, file)?;
    let root = syntax(text, file)?;
    let text = block_root(text, &root);
    let root = syntax(&text, file)?;
    let Kind::Seq(rows) = &root.kind else {
        return Err(shape(file, "Profile patch must be a YAML sequence"));
    };
    let values = raw
        .as_array()
        .ok_or_else(|| shape(file, "Profile patch must be a YAML sequence"))?;
    let matches = |value: &Value| value["id"].as_str() == Some(id) && value.get("insert").is_none();
    let selected = values.iter().enumerate().rposition(|(index, value)| {
        matches!(rows[index].kind, Kind::Map(_))
            && matches(value)
            && value
                .get("name")
                .is_none_or(|value| value.as_str() == Some(name))
    });
    let mut edits: Vec<(Range<usize>, String)> = vec![];
    let mut removed = 0;
    for (index, (row, value)) in rows.iter().zip(values).enumerate() {
        if !(if inherited {
            matches(value)
        } else {
            selected == Some(index)
        }) {
            continue;
        }
        let Kind::Map(fields) = &row.kind else {
            continue;
        };
        let mut updated = value.clone();
        let Some(map) = updated.as_object_mut() else {
            continue;
        };
        if inherited {
            map.remove("config");
        } else {
            map.insert("config".into(), next.clone());
        }
        if inherited && map.keys().all(|key| key == "id" || key == "name") {
            edits.push((
                line_start(&text, row.range.start)..line_end(&text, end(&text, row)),
                String::new(),
            ));
            removed += 1;
            continue;
        }
        let field = value
            .as_object()
            .and_then(|map| map.keys().position(|key| key == "config"));
        if row.flow
            && inherited
            && let Some(position) = field
        {
            let (key, node) = &fields[position];
            let span = if let Some((next, _)) = fields.get(position + 1) {
                key.range.start..next.range.start
            } else if position > 0 {
                end(&text, &fields[position - 1].1)..end(&text, node)
            } else {
                key.range.start..end(&text, node)
            };
            edits.push((span, String::new()));
        } else if (row.flow && block_value(next)) || (!row.flow && field == Some(0)) {
            let start = line_start(&text, row.range.start);
            let prefix = &text[start..row.range.start];
            let attributes = prefix
                .find("- ")
                .map(|dash| (start + dash + 2, &prefix[dash + 2..]));
            if let Some((position, attributes)) =
                attributes.filter(|(_, attributes)| !attributes.trim().is_empty())
            {
                let column = position - start;
                edits.push((
                    position..end(&text, row),
                    format!(
                        "{}\n{}{}",
                        attributes.trim_end(),
                        " ".repeat(column),
                        map_text(&updated, column)
                    ),
                ));
            } else {
                edits.push((
                    row.range.start..end(&text, row),
                    map_text(&updated, row.column),
                ));
            }
        } else if let Some(position) = field {
            let (key, node) = &fields[position];
            if inherited {
                edits.push((
                    line_start(&text, key.range.start)..line_end(&text, end(&text, node)),
                    String::new(),
                ));
            } else {
                let colon = key.range.end
                    + text[key.range.end..]
                        .find(':')
                        .ok_or_else(|| shape(file, "expected mapping colon"))?
                    + 1;
                edits.push((
                    colon..end(&text, node).max(colon),
                    value_text(next, key.column),
                ));
            }
        } else if !inherited {
            if row.flow {
                let position = fields
                    .last()
                    .map(|(_, value)| end(&text, value))
                    .unwrap_or(row.range.start + 1);
                edits.push((
                    position..position,
                    format!(", config:{}", value_text(next, 0)),
                ));
                continue;
            }
            let position = line_end(&text, end(&text, row));
            let newline = if position > 0 && text.as_bytes()[position - 1] == b'\n' {
                ""
            } else {
                "\n"
            };
            edits.push((
                position..position,
                format!(
                    "{newline}{}config:{}\n",
                    " ".repeat(row.column),
                    value_text(next, row.column)
                ),
            ));
        }
    }
    if !inherited && selected.is_none() {
        let new =
            crate::yaml::write_yaml(&serde_json::json!([{"id":id,"name":name,"config":next}]));
        if rows.is_empty() {
            edits.push((root.range.clone(), new.trim_end_matches('\n').into()));
        } else {
            let position = rows
                .last()
                .map(|row| line_end(&text, end(&text, row)))
                .unwrap_or(root.range.end);
            let newline = if position > 0 && text.as_bytes()[position - 1] == b'\n' {
                ""
            } else {
                "\n"
            };
            edits.push((position..position, format!("{newline}{new}")));
        }
    }
    if removed > 0
        && removed == rows.len()
        && let Some((_, replacement)) = edits.last_mut()
    {
        *replacement = "[]\n".into();
    }
    fn expressions(node: &Node, value: &Value, edits: &mut Vec<(Range<usize>, String)>) {
        if edits
            .iter()
            .any(|(span, _)| span.start <= node.range.start && span.end >= node.range.end)
        {
            return;
        }
        match (&node.kind, value) {
            (Kind::Map(_), _) if crate::expression(value).is_some() => {
                edits.push((
                    node.range.clone(),
                    crate::yaml::write_yaml(value).trim_end_matches('\n').into(),
                ));
            }
            (Kind::Map(fields), Value::Object(map)) => {
                for ((_, child), value) in fields.iter().zip(map.values()) {
                    expressions(child, value, edits);
                }
            }
            (Kind::Seq(children), Value::Array(values)) => {
                for (child, value) in children.iter().zip(values) {
                    expressions(child, value, edits);
                }
            }
            _ => {}
        }
    }
    expressions(&root, &raw, &mut edits);
    edits.sort_by_key(|(span, _)| (span.start, span.end));
    let mut output = String::with_capacity(text.len());
    let mut boundary = 0;
    for (span, replacement) in edits {
        if span.start < boundary {
            return Err(shape(file, "overlapping YAML edits"));
        }
        output.push_str(&text[boundary..span.start]);
        output.push_str(&replacement);
        boundary = span.end;
    }
    output.push_str(&text[boundary..]);
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn edit(text: &str, next: Value, inherited: bool) -> String {
        edit_config(
            text,
            Path::new("/fixture/cordis.patch.yml"),
            "x",
            "objects",
            &next,
            inherited,
        )
        .unwrap()
    }
    #[test]
    fn flow_rows_and_a_flow_root_accept_block_config_nodes() {
        assert_eq!(
            edit(
                "# before\n- {id: x, config: {old: true}}\n# after\n",
                json!({"title":"edited"}),
                false
            ),
            "# before\n- id: x\n  config:\n    title: edited\n# after\n"
        );
        assert_eq!(
            edit(
                "# before\n[{id: x, config: 1}, {id: y, disabled: true}]\n",
                json!(2),
                false
            ),
            "# before\n- {id: x, config: 2}\n- {id: y, disabled: true}\n"
        );
    }
    #[test]
    fn missing_and_empty_config_nodes_preserve_following_comments_and_keys() {
        assert_eq!(
            edit(
                "- id: x\n  name: objects\n# after\n",
                json!({"title":"edited"}),
                false
            ),
            "- id: x\n  name: objects\n  config:\n    title: edited\n# after\n"
        );
        assert_eq!(
            edit(
                "- id: x\n  config:\n  disabled: false\n",
                json!({"title":"edited"}),
                false
            ),
            "- id: x\n  config:\n    title: edited\n  disabled: false\n"
        );
    }
    #[test]
    fn replacing_a_block_scalar_keeps_the_following_row_separate() {
        assert_eq!(
            edit(
                "- id: x\n  config: |\n    old text\n- id: y\n  disabled: true\n",
                json!({"title":"edited"}),
                false
            ),
            "- id: x\n  config:\n    title: edited\n- id: y\n  disabled: true\n"
        );
    }
    #[test]
    fn unrelated_expression_marker_maps_are_changed_only_at_their_own_span() {
        assert_eq!(
            edit(
                "# retain\n- id: y\n  config: {value: {__jsExpr: \"'old'\"}}\n- id: x\n  config: 1\n",
                json!(2),
                false
            ),
            "# retain\n- id: y\n  config: {value: !!js \"'old'\"}\n- id: x\n  config: 2\n"
        );
    }
    #[test]
    fn removing_all_overrides_keeps_the_document_loadable() {
        assert_eq!(
            edit(
                "# keep\n- {id: x, config: 1}\n- {id: x, name: objects, config: 2}\n",
                json!({}),
                true
            ),
            "# keep\n[]\n"
        );
    }
    #[test]
    fn forcing_a_flow_root_to_block_preserves_inter_item_comments() {
        let result = edit(
            "# before\n[\n  {id: x, config: 1}, # inline\n  # between\n  {id: y, disabled: true}\n]\n# after\n",
            json!(2),
            false,
        );
        assert!(result.contains("# inline"), "{result}");
        assert!(result.contains("# between"), "{result}");
        assert!(result.starts_with("# before\n"));
        assert!(result.ends_with("# after\n"));
        assert!(
            crate::parse_patches(&result, Path::new("/fixture/cordis.patch.yml")).is_ok(),
            "{result}"
        );
    }
    #[test]
    fn scalar_edits_select_the_mapping_declaration_instead_of_its_row_alias() {
        assert_eq!(
            edit("- &row {id: x, config: 1}\n- *row\n", json!(2), false),
            "- &row {id: x, config: 2}\n- *row\n"
        );
    }
}
