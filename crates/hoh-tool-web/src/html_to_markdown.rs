//! A self-contained Turndown 7.2/GFM-style renderer over html5ever.

use std::{panic::resume_unwind, thread};

use hoh_util_values::{is_js_white_space, js_trim};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

const REMOVED: &[&str] = &[
    "script", "style", "noscript", "template", "iframe", "object", "embed",
];

pub fn convert(html: &str) -> String {
    thread::scope(|scope| {
        let conversion = thread::Builder::new()
            .name("hoh-html-to-markdown".into())
            .stack_size(8 * 1024 * 1024)
            .spawn_scoped(scope, || convert_on_bounded_stack(html))
            .expect("spawn HTML conversion thread");
        match conversion.join() {
            Ok(markdown) => markdown,
            Err(panic) => resume_unwind(panic),
        }
    })
}

fn convert_on_bounded_stack(html: &str) -> String {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    normalize_whitespace(
        &dom.document,
        &mut WhitespaceFlow {
            at_boundary: true,
            ..WhitespaceFlow::default()
        },
    );
    cleanup(render_children(&dom.document, &RenderState::default()))
}

#[derive(Default)]
struct WhitespaceFlow {
    at_boundary: bool,
    ends_with_space: bool,
    previous_text: Option<Handle>,
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "html"
            | "body"
            | "main"
            | "article"
            | "section"
            | "header"
            | "footer"
            | "nav"
            | "figure"
            | "figcaption"
            | "address"
            | "aside"
            | "audio"
            | "canvas"
            | "form"
            | "fieldset"
            | "center"
            | "dir"
            | "frameset"
            | "noframes"
            | "hgroup"
            | "isindex"
            | "menu"
            | "output"
            | "div"
            | "p"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "hr"
            | "blockquote"
            | "pre"
            | "ul"
            | "ol"
            | "li"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "th"
            | "td"
            | "dl"
            | "dt"
            | "dd"
    )
}

fn whitespace_boundary(flow: &mut WhitespaceFlow) {
    if let Some(previous) = flow.previous_text.take()
        && let NodeData::Text { contents } = &previous.data
    {
        let trimmed = contents
            .borrow()
            .trim_end_matches(is_ascii_html_whitespace)
            .to_owned();
        *contents.borrow_mut() = trimmed.into();
    }
    flow.at_boundary = true;
    flow.ends_with_space = false;
}

fn normalize_whitespace(node: &Handle, flow: &mut WhitespaceFlow) {
    match &node.data {
        NodeData::Document => {
            for child in node.children.borrow().iter() {
                normalize_whitespace(child, flow);
            }
        }
        NodeData::Text { contents } => {
            let collapsed = collapse_whitespace(&contents.borrow());
            let collapsed =
                if (flow.at_boundary || flow.ends_with_space) && collapsed.starts_with(' ') {
                    collapsed[1..].to_owned()
                } else {
                    collapsed
                };
            *contents.borrow_mut() = collapsed.clone().into();
            if collapsed.is_empty() {
                return;
            }
            flow.at_boundary = false;
            flow.ends_with_space = collapsed.ends_with(' ');
            flow.previous_text = Some(node.clone());
        }
        NodeData::Element { .. } => {
            let name = element_name(node).unwrap_or_default();
            if hidden(node, name) {
                return;
            }
            if name == "pre" {
                whitespace_boundary(flow);
                whitespace_boundary(flow);
                return;
            }
            let block = is_block(name) || name == "br";
            if block {
                whitespace_boundary(flow);
            }
            if matches!(name, "img" | "input") {
                flow.at_boundary = false;
                flow.ends_with_space = false;
                flow.previous_text = None;
            } else {
                for child in node.children.borrow().iter() {
                    normalize_whitespace(child, flow);
                }
            }
            if block {
                whitespace_boundary(flow);
            }
        }
        NodeData::Doctype { .. }
        | NodeData::Comment { .. }
        | NodeData::ProcessingInstruction { .. } => {}
    }
}

#[derive(Clone, Default)]
struct RenderState {
    in_link: bool,
    list_depth: usize,
}

fn element_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref()),
        _ => None,
    }
}

fn attr(node: &Handle, wanted: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|item| item.name.local.to_string().eq_ignore_ascii_case(wanted))
        .map(|item| item.value.to_string())
}

fn has_attr(node: &Handle, wanted: &str) -> bool {
    let NodeData::Element { attrs, .. } = &node.data else {
        return false;
    };
    attrs
        .borrow()
        .iter()
        .any(|item| item.name.local.to_string().eq_ignore_ascii_case(wanted))
}

fn hidden(node: &Handle, name: &str) -> bool {
    if REMOVED.contains(&name) || has_attr(node, "hidden") {
        return true;
    }
    if attr(node, "aria-hidden").is_some_and(|value| value.eq_ignore_ascii_case("true")) {
        return true;
    }
    if name == "input"
        && attr(node, "type").is_some_and(|value| value.eq_ignore_ascii_case("hidden"))
    {
        return true;
    }
    attr(node, "style").is_some_and(|style| {
        style.split(';').any(|declaration| {
            let Some((property, value)) = declaration.split_once(':') else {
                return false;
            };
            let property = property.trim().to_ascii_lowercase();
            let mut value = value.trim().to_ascii_lowercase();
            if let Some(stripped) = value.strip_suffix("!important") {
                value = stripped.trim().to_owned();
            }
            (property == "display" && value == "none")
                || (property == "visibility" && matches!(value.as_str(), "hidden" | "collapse"))
        })
    })
}

fn render_children(node: &Handle, state: &RenderState) -> String {
    node.children
        .borrow()
        .iter()
        .map(|child| render(child, state))
        .collect()
}

fn render(node: &Handle, state: &RenderState) -> String {
    match &node.data {
        NodeData::Document => render_children(node, state),
        NodeData::Text { contents } => {
            let text = contents.borrow();
            escape_text(&text, state.in_link)
        }
        NodeData::Element { .. } => render_element(node, state),
        NodeData::Doctype { .. }
        | NodeData::Comment { .. }
        | NodeData::ProcessingInstruction { .. } => String::new(),
    }
}

fn render_element(node: &Handle, state: &RenderState) -> String {
    let name = element_name(node).unwrap_or_default();
    if hidden(node, name) {
        return String::new();
    }
    match name {
        "html" | "body" | "main" | "article" | "section" | "header" | "footer" | "nav"
        | "figure" | "figcaption" | "address" | "aside" | "audio" | "canvas" | "form"
        | "fieldset" | "center" | "dir" | "frameset" | "noframes" | "hgroup" | "isindex"
        | "menu" | "output" => block(render_children(node, state)),
        "div" => {
            render_highlighted_code(node).unwrap_or_else(|| block(render_children(node, state)))
        }
        "p" => block(render_children(node, state)),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = name[1..].parse::<usize>().unwrap_or(1);
            let rendered = render_children(node, state);
            let content = trim_ascii_html_whitespace(&rendered);
            if content.is_empty() {
                String::new()
            } else {
                block(format!("{} {content}", "#".repeat(level)))
            }
        }
        "br" => "  \n".into(),
        "hr" => "\n\n* * *\n\n".into(),
        "strong" | "b" => wrap_inline("**", render_children(node, state)),
        "em" | "i" => wrap_inline("_", render_children(node, state)),
        "del" | "s" | "strike" => wrap_inline("~~", render_children(node, state)),
        "mark" => render_children(node, state),
        "blockquote" => {
            let content = cleanup(render_children(node, state));
            block(
                content
                    .lines()
                    .map(|line| {
                        if line.is_empty() {
                            "> ".into()
                        } else {
                            format!("> {line}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        "a" => render_link(node, state),
        "img" => render_image(node),
        "code" => render_inline_code(&raw_text(node)),
        "pre" => render_pre(node, state),
        "ul" => render_list(node, state, false),
        "ol" => render_list(node, state, true),
        "li" => render_children(node, state),
        "table" => render_table(node, state),
        "thead" | "tbody" | "tfoot" | "tr" | "th" | "td" => render_children(node, state),
        "dl" => block(render_children(node, state)),
        "dt" => block(render_children(node, state)),
        "dd" => block(render_children(node, state)),
        "input" => render_task_input(node),
        _ => render_children(node, state),
    }
}

fn block(content: String) -> String {
    let content = content.trim_matches(|ch: char| ch == '\n' || ch == ' ' || ch == '\t');
    if content.is_empty() {
        String::new()
    } else {
        format!("\n\n{content}\n\n")
    }
}

fn wrap_inline(marker: &str, content: String) -> String {
    let leading = content.starts_with(is_ascii_html_whitespace);
    let trailing = content.ends_with(is_ascii_html_whitespace);
    let trimmed = trim_ascii_html_whitespace(&content);
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(
            "{}{marker}{trimmed}{marker}{}",
            if leading { " " } else { "" },
            if trailing { " " } else { "" }
        )
    }
}

fn is_ascii_html_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\r' | '\n' | '\t')
}

fn trim_ascii_html_whitespace(value: &str) -> &str {
    value.trim_matches(is_ascii_html_whitespace)
}

fn collapse_whitespace(text: &str) -> String {
    let mut output = String::new();
    let mut whitespace = false;
    let mut saw_whitespace = false;
    for ch in text.chars() {
        if is_ascii_html_whitespace(ch) {
            whitespace = true;
            saw_whitespace = true;
        } else {
            if whitespace && !output.is_empty() {
                output.push(' ');
            }
            output.push(ch);
            whitespace = false;
        }
    }
    if whitespace && !output.is_empty() {
        output.push(' ');
    }
    if output.is_empty() && saw_whitespace {
        output.push(' ');
    }
    output
}

fn escape_text(text: &str, _in_link: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '*' | '_' | '`' | '[' | ']') {
            out.push('\\');
        }
        out.push(ch);
    }
    for marker in ["-", "+ ", "~~~", ">"] {
        if out.starts_with(marker) {
            out.insert(0, '\\');
            return out;
        }
    }
    let equals = out.bytes().take_while(|byte| *byte == b'=').count();
    if equals > 0 {
        out.insert(0, '\\');
        return out;
    }
    let hashes = out.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&hashes) && out.as_bytes().get(hashes) == Some(&b' ') {
        out.insert(0, '\\');
        return out;
    }
    let digits = out.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && out.as_bytes().get(digits..digits + 2) == Some(b". ") {
        out.insert(digits, '\\');
    }
    out
}

fn render_link(node: &Handle, state: &RenderState) -> String {
    let mut child_state = state.clone();
    child_state.in_link = true;
    let rendered = render_children(node, &child_state);
    let leading = rendered.starts_with(is_ascii_html_whitespace);
    let trailing = rendered.ends_with(is_ascii_html_whitespace);
    let content = trim_ascii_html_whitespace(&rendered).to_owned();
    let Some(href) = attr(node, "href") else {
        return rendered;
    };
    if content.is_empty() || href.is_empty() {
        return rendered;
    }
    let destination = href.replace('(', "\\(").replace(')', "\\)");
    let link = match attr(node, "title").filter(|title| !title.is_empty()) {
        Some(title) => format!(
            "[{content}]({destination} \"{}\")",
            title.replace('"', "\\\"")
        ),
        None => format!("[{content}]({destination})"),
    };
    format!(
        "{}{link}{}",
        if leading { " " } else { "" },
        if trailing { " " } else { "" }
    )
}

/// Turndown's `cleanAttribute`: `/(\n+\s*)+/g` becomes one newline.
fn clean_attribute(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('\n') {
        out.push_str(&rest[..at]);
        out.push('\n');
        rest = rest[at..].trim_start_matches(is_js_white_space);
    }
    out.push_str(rest);
    out
}

fn render_image(node: &Handle) -> String {
    let Some(src) = attr(node, "src").filter(|src| !src.is_empty()) else {
        return String::new();
    };
    let alt = clean_attribute(&attr(node, "alt").unwrap_or_default());
    match Some(clean_attribute(&attr(node, "title").unwrap_or_default()))
        .filter(|title| !title.is_empty())
    {
        Some(title) => format!("![{alt}]({src} \"{title}\")"),
        None => format!("![{alt}]({src})"),
    }
}

fn raw_text(node: &Handle) -> String {
    node.children
        .borrow()
        .iter()
        .map(|child| match &child.data {
            NodeData::Text { contents } => contents.borrow().to_string(),
            _ => raw_text(child),
        })
        .collect()
}

fn longest_run(text: &str, wanted: char) -> usize {
    text.split(|ch| ch != wanted)
        .map(str::len)
        .max()
        .unwrap_or_default()
}

fn render_inline_code(text: &str) -> String {
    let text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
    if text.is_empty() {
        return String::new();
    }
    let delimiter = "`".repeat(longest_run(&text, '`') + 1);
    let padding = if text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' '))
    {
        " "
    } else {
        ""
    };
    format!("{delimiter}{padding}{text}{padding}{delimiter}")
}

/// Turndown's `fencedCodeBlock` applies only when the first child node of `pre` is `code`.
fn first_child_code(node: &Handle) -> Option<Handle> {
    node.children
        .borrow()
        .first()
        .filter(|child| element_name(child) == Some("code"))
        .cloned()
}

/// A fence longer than every run of three or more backticks that starts a line of `code`
/// (`^`{3,}` with the multiline flag; JavaScript line terminators).
fn fence_for(code: &str) -> String {
    let mut size = 3;
    for line in code.split(['\n', '\r', '\u{2028}', '\u{2029}']) {
        let run = line.bytes().take_while(|byte| *byte == b'`').count();
        if run >= 3 && run >= size {
            size = run + 1;
        }
    }
    "`".repeat(size)
}

fn render_pre(node: &Handle, state: &RenderState) -> String {
    if node.children.borrow().is_empty() || !has_element_child(node, "code") {
        return block(raw_text(node));
    }
    let Some(code) = first_child_code(node) else {
        return block(render_children(node, state));
    };
    let raw = raw_text(&code);
    let text = raw.strip_suffix('\n').unwrap_or(&raw);
    let fence = fence_for(&raw);
    let language = attr(&code, "class")
        .and_then(|classes| {
            // `/language-(\S+)/`: the first occurrence followed by a non-space character.
            classes.match_indices("language-").find_map(|(at, _)| {
                let rest = &classes[at + "language-".len()..];
                let end = rest.find(is_js_white_space).unwrap_or(rest.len());
                (end > 0).then(|| rest[..end].to_owned())
            })
        })
        .unwrap_or_default();
    format!("\n\n{fence}{language}\n{text}\n{fence}\n\n")
}

fn has_element_child(node: &Handle, wanted: &str) -> bool {
    node.children
        .borrow()
        .iter()
        .any(|child| element_name(child) == Some(wanted))
}

/// The language of `/highlight-(?:text|source)-([a-z0-9]+)/`, searched anywhere in the class text.
fn highlight_language(classes: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = classes[from..].find("highlight-") {
        let after = from + found + "highlight-".len();
        let rest = &classes[after..];
        let tail = rest
            .strip_prefix("text-")
            .or_else(|| rest.strip_prefix("source-"));
        if let Some(tail) = tail {
            let length = tail
                .bytes()
                .take_while(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                .count();
            if length > 0 {
                return Some(tail[..length].to_owned());
            }
        }
        from = after;
    }
    None
}

fn is_empty_text(node: &Handle) -> bool {
    matches!(&node.data, NodeData::Text { contents } if contents.borrow().is_empty())
}

fn render_highlighted_code(node: &Handle) -> Option<String> {
    let language = highlight_language(&attr(node, "class")?)?;
    // Whitespace-only text nodes are removed before the rules run in Turndown, so the first
    // child is the first node that is not an emptied text node.
    let pre = node
        .children
        .borrow()
        .iter()
        .find(|child| !is_empty_text(child))
        .filter(|child| element_name(child) == Some("pre"))
        .cloned()?;
    Some(format!("\n\n```{language}\n{}\n```\n\n", raw_text(&pre)))
}

fn render_task_input(node: &Handle) -> String {
    if !attr(node, "type").is_some_and(|value| value.eq_ignore_ascii_case("checkbox")) {
        return String::new();
    }
    if has_attr(node, "checked") {
        "[x] ".into()
    } else {
        "[ ] ".into()
    }
}

fn render_list(node: &Handle, state: &RenderState, ordered: bool) -> String {
    let start = attr(node, "start")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(1);
    let mut next_state = state.clone();
    next_state.list_depth += 1;
    let indent = "    ".repeat(state.list_depth);
    let mut lines = Vec::new();
    let mut position = 0_i64;
    for child in node.children.borrow().iter() {
        if element_name(child) != Some("li") {
            continue;
        }
        let marker = if ordered {
            format!("{}.  ", start + position)
        } else {
            "-   ".to_owned()
        };
        position += 1;
        let content = cleanup(render_children(child, &next_state));
        if content.is_empty() {
            continue;
        }
        let content_lines = content.lines().collect::<Vec<_>>();
        let first = content_lines.first().copied().unwrap_or_default();
        lines.push(format!("{indent}{marker}{first}"));
        let continuation = "    ";
        for line in content_lines.iter().skip(1) {
            if line.chars().all(|ch| ch == ' ') {
                lines.push(format!("{indent}{continuation}"));
                continue;
            }
            if line.starts_with("    ") {
                lines.push(format!("{indent}{line}"));
            } else {
                lines.push(format!("{indent}{continuation}{line}"));
            }
        }
    }
    let content = lines.join("\n");
    if ends_parent_item(node) {
        // Turndown's `list` rule: a list that ends its item adds one newline, not a blank line.
        let content = content.trim_matches(['\n', ' ', '\t']);
        return format!("\n{content}");
    }
    block(content)
}

fn ends_parent_item(node: &Handle) -> bool {
    let Some(parent) = node.parent.take() else {
        return false;
    };
    node.parent.set(Some(parent.clone()));
    let Some(parent) = parent.upgrade() else {
        return false;
    };
    element_name(&parent) == Some("li")
        && parent
            .children
            .borrow()
            .iter()
            .rev()
            .find(|child| element_name(child).is_some())
            .is_some_and(|last| std::rc::Rc::ptr_eq(last, node))
}

fn descendants_named(node: &Handle, wanted: &str, output: &mut Vec<Handle>) {
    for child in node.children.borrow().iter() {
        if element_name(child) == Some(wanted) {
            output.push(child.clone());
        } else if !matches!(element_name(child), Some("table")) {
            descendants_named(child, wanted, output);
        }
    }
}

fn direct_cells(row: &Handle) -> Vec<Handle> {
    row.children
        .borrow()
        .iter()
        .filter(|child| matches!(element_name(child), Some("th" | "td")))
        .cloned()
        .collect()
}

fn table_cell(content: &str, index: usize) -> String {
    let prefix = if index == 0 { "| " } else { " " };
    let normalized = js_trim(content)
        .replace("\n\r", "<br>")
        .replace('\n', "<br>");
    let mut escaped = String::with_capacity(normalized.len());
    let mut in_pipes = false;
    for ch in normalized.chars() {
        if ch == '|' {
            if !in_pipes {
                escaped.push_str("\\|");
                in_pipes = true;
            }
        } else {
            escaped.push(ch);
            in_pipes = false;
        }
    }
    let padding = " ".repeat(3_usize.saturating_sub(escaped.encode_utf16().count()));
    format!("{prefix}{escaped}{padding} |")
}

fn cell_alignment(cell: &Handle) -> &'static str {
    let value = attr(cell, "align").or_else(|| {
        attr(cell, "style").and_then(|style| {
            style.split(';').find_map(|part| {
                let (property, value) = part.split_once(':')?;
                property
                    .trim()
                    .eq_ignore_ascii_case("text-align")
                    .then(|| value.trim().to_ascii_lowercase())
            })
        })
    });
    match value.as_deref() {
        Some("left") => ":---",
        Some("right") => "---:",
        Some("center") => ":---:",
        _ => "---",
    }
}

fn render_table(node: &Handle, state: &RenderState) -> String {
    let mut rows = Vec::new();
    descendants_named(node, "tr", &mut rows);
    if rows.is_empty() {
        return String::new();
    }
    let mut output = Vec::new();
    let first_cells = direct_cells(&rows[0]);
    let first_is_heading = first_cells
        .iter()
        .all(|cell| element_name(cell) == Some("th"));
    for (row_index, row) in rows.iter().enumerate() {
        let cells = direct_cells(row);
        output.push(
            cells
                .iter()
                .enumerate()
                .map(|(index, cell)| table_cell(&cleanup(render_children(cell, state)), index))
                .collect::<String>(),
        );
        if row_index == 0 && first_is_heading {
            output.push(
                first_cells
                    .iter()
                    .enumerate()
                    .map(|(index, cell)| table_cell(cell_alignment(cell), index))
                    .collect(),
            );
        }
    }
    block(output.join("\n"))
}

fn cleanup(input: String) -> String {
    let mut lines = Vec::new();
    let mut fence_length = None;
    for line in input.split('\n') {
        let trimmed = line.trim_start();
        let ticks = trimmed.bytes().take_while(|byte| *byte == b'`').count();
        let closes_fence = fence_length
            .is_some_and(|open| ticks >= open && trimmed[ticks..].trim_matches(' ').is_empty());
        if fence_length.is_none() && line.is_empty() && lines.last().is_some_and(String::is_empty) {
            continue;
        }
        lines.push(line.to_owned());
        if closes_fence {
            fence_length = None;
        } else if fence_length.is_none() && ticks >= 3 {
            fence_length = Some(ticks);
        }
    }
    let output = lines.join("\n");
    trim_ascii_html_whitespace(&output).to_owned()
}

#[cfg(test)]
mod tests {
    use super::convert;

    #[test]
    fn renders_core_turndown_and_gfm_rules() {
        assert_eq!(
            convert("<h2>Heading</h2><ul><li>one</li><li>two</li></ul>"),
            "## Heading\n\n-   one\n-   two"
        );
        assert_eq!(
            convert("<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>"),
            "| A   | B   |\n| --- | --- |\n| 1   | 2   |"
        );
        assert_eq!(
            convert(
                "<p><strong>bold <em>italic</em></strong></p><blockquote><p>quoted</p></blockquote>"
            ),
            "**bold _italic_**\n\n> quoted"
        );
    }

    #[test]
    fn removes_active_and_hidden_nodes() {
        let html = "<style>.x{}</style><script>bad()</script><noscript>ns</noscript><template>template</template><iframe>frame</iframe><object>object</object><embed src='hidden'><p hidden>hidden</p><p aria-hidden='true'>aria</p><p style='display:none !important'>gone</p><p style='visibility:collapse'>visibility</p><input type='hidden' value='secret'><p>Tom &amp; Jerry &copy; R&eacute;sum&eacute;</p>";
        assert_eq!(convert(html), "Tom & Jerry © Résumé");
    }

    #[test]
    fn supports_nested_lists_code_links_images_and_strike() {
        let html = "<ol start='3'><li>a<ul><li>b</li></ul></li></ol><pre><code class='language-rs'>let x = `y`;</code></pre><p><del>x</del> <a href='https://a.test'>A</a> <img src='x.png' alt='x'></p>";
        let out = convert(html);
        assert!(out.contains("3.  a"));
        assert!(out.contains("-   b"));
        assert!(out.contains("```rs"));
        assert!(out.contains("~~x~~ [A](https://a.test) ![x](x.png)"));
    }

    #[test]
    fn preserves_nested_list_structure_at_three_levels_and_across_list_kinds() {
        assert_eq!(
            convert("<ul><li>a<ul><li>b<ul><li>c</li></ul></li></ul></li></ul>"),
            "-   a\n    -   b\n        -   c"
        );
        assert_eq!(
            convert("<ul><li>a<ol start='2'><li>b</li><li>c</li></ol></li></ul>"),
            "-   a\n    2.  b\n    3.  c"
        );
    }

    #[test]
    fn normalizes_ascii_whitespace_with_dom_sibling_context() {
        assert_eq!(convert("<div>\n<p>a</p>\n<p>b</p>\n</div>"), "a\n\nb");
        assert_eq!(
            convert("<p>a <strong> b </strong> c <em>d </em>e</p>"),
            "a **b** c _d_ e"
        );
        assert_eq!(
            convert(
                "<div>\n<ul>\n<li>a</li>\n</ul>\n<table>\n<tr><th>B</th></tr>\n</table>\n</div>"
            ),
            "-   a\n\n| B   |\n| --- |"
        );
    }

    #[test]
    fn moves_inline_flanking_whitespace_outside_markers_and_links() {
        assert_eq!(convert("<p><b>Note: </b>text</p>"), "**Note:** text");
        assert_eq!(convert("<p><em>one </em>two</p>"), "_one_ two");
        assert_eq!(convert("<p><del>old </del>new</p>"), "~~old~~ new");
        assert_eq!(
            convert("<p><a href='https://a.test'>site </a>next</p>"),
            "[site](https://a.test) next"
        );
    }

    #[test]
    fn matches_default_block_blank_and_highlighted_code_rules() {
        assert_eq!(convert("<p><mark>plain</mark></p>"), "plain");
        assert_eq!(
            convert("<dl><dt>Term</dt><dd>Definition</dd></dl>"),
            "Term\n\nDefinition"
        );
        assert_eq!(
            convert(
                "<div>a</div><form>b</form><fieldset>c</fieldset><center>d</center><hgroup>e</hgroup><menu>f</menu>"
            ),
            "a\n\nb\n\nc\n\nd\n\ne\n\nf"
        );
        assert_eq!(
            convert("<div class='highlight highlight-source-js'><pre>const x = 1;</pre></div>"),
            "```js\nconst x = 1;\n```"
        );
        assert_eq!(
            convert(
                "<canvas>a</canvas><audio>b</audio><output>c</output><summary>d</summary><dir>e</dir><frameset>f</frameset><noframes>g</noframes><span>h</span>"
            ),
            "a\n\nb\n\nc\n\nd\n\ne\n\nf\n\ng\n\nh"
        );
        for empty in [
            "<h2></h2>",
            "<ul><li></li></ul>",
            "<a href='x'></a>",
            "<strong></strong>",
        ] {
            assert_eq!(convert(empty), "", "{empty}");
        }
    }

    #[test]
    fn matches_pre_code_image_link_quote_list_and_unicode_whitespace_edges() {
        assert_eq!(convert("<pre>alpha\nbeta</pre>"), "alpha\nbeta");
        assert_eq!(
            convert("<pre><code>a\n\n\n  \nb</code></pre>"),
            "```\na\n\n\n  \nb\n```"
        );
        assert_eq!(
            convert("<pre><code>a\n```\n\n\n  \nb</code></pre>"),
            "````\na\n```\n\n\n  \nb\n````"
        );
        assert_eq!(convert("<p><code>alpha\nbeta</code></p>"), "`alpha beta`");
        assert_eq!(convert("<img src='x' alt='a]b'>"), "![a]b](x)");
        assert_eq!(convert("<p><a href=''>text</a></p>"), "text");
        assert_eq!(convert("<p>a&nbsp;&nbsp;b</p>"), "a\u{a0}\u{a0}b");
        assert_eq!(
            convert("<blockquote><p>a</p><p>b</p></blockquote>"),
            "> a\n> \n> b"
        );
        assert_eq!(
            convert("<ol start='10'><li>a<br>b</li></ol>"),
            "10.  a  \n    b"
        );
        assert_eq!(convert("<ol><li></li><li>b</li></ol>"), "2.  b");
        assert_eq!(
            convert("<ul><li><p>a</p><p>b</p></li></ul>"),
            "-   a\n    \n    b"
        );
    }

    #[test]
    fn gfm_tables_collapse_pipe_runs_and_only_heading_rows_get_borders() {
        assert_eq!(
            convert("<table><tr><th align='right'>a||b</th></tr></table>"),
            "| a\\|b |\n| ---: |"
        );
        assert_eq!(
            convert("<table><tr><td>A</td></tr><tr><td>B</td></tr></table>"),
            "| A   |\n| B   |"
        );
        assert_eq!(
            convert(
                "<table><thead><tr><th colspan='1000000'>A</th></tr></thead><tbody><tr><td>B</td></tr></tbody></table>"
            ),
            "| A   |\n| --- |\n| B   |"
        );
    }

    #[test]
    fn escapes_markdown_constructs_in_provider_text_and_preserves_hard_breaks() {
        assert_eq!(convert("<p># title</p>"), "\\# title");
        assert_eq!(convert("<p>[label](url)</p>"), "\\[label\\](url)");
        assert_eq!(convert("<p>1. item</p>"), "1\\. item");
        assert_eq!(convert("<p>&gt; quote</p>"), "\\> quote");
        assert_eq!(convert("<p>a<br>b</p>"), "a  \nb");
    }

    #[test]
    fn highlighted_code_follows_the_gfm_rule() {
        assert_eq!(
            convert(
                "<div class=\"highlight highlight-text-html-basic\"><pre>&lt;a&gt;</pre></div>"
            ),
            "```html\n<a>\n```"
        );
        assert_eq!(
            convert("<div class=\"highlight-source-c++\"><pre>x</pre></div>"),
            "```c\nx\n```"
        );
        assert_eq!(
            convert("<div class=\"highlight highlight-source-js\"><pre>x\n\n</pre></div>"),
            "```js\nx\n\n\n```"
        );
    }

    #[test]
    fn highlighted_code_ignores_whitespace_between_tags() {
        assert_eq!(
            convert("<div class=\"highlight-source-js\">\n  <pre>x</pre>\n</div>"),
            "```js\nx\n```"
        );
        assert_eq!(
            convert("<div class=\"highlight-source-js\"><pre>x</pre> \n</div>"),
            "```js\nx\n```"
        );
    }

    #[test]
    fn images_without_src_vanish_and_attributes_lose_line_breaks() {
        assert_eq!(convert("<p><img src=\"\" alt=\"x\"></p>"), "");
        assert_eq!(
            convert("<p><img src=\"a.png\" alt=\"x\n  y\" title=\"t\n\n u\"></p>"),
            "![x\ny](a.png \"t\nu\")"
        );
    }

    #[test]
    fn fenced_code_language_takes_the_first_class_with_a_name() {
        assert_eq!(
            convert("<pre><code class=\"language- language-rs\">x</code></pre>"),
            "```rs\nx\n```"
        );
    }

    #[test]
    fn table_cells_pad_in_utf16_units_and_trim_like_javascript() {
        assert_eq!(
            convert("<table><tr><th>\u{1F600}</th></tr></table>"),
            "| \u{1F600}  |\n| --- |"
        );
        assert_eq!(
            convert("<table><tr><th>\u{feff}x</th></tr></table>"),
            "| x   |\n| --- |"
        );
    }

    #[test]
    fn fenced_code_follows_turndown() {
        assert_eq!(convert("<pre><code>a\n\n</code></pre>"), "```\na\n\n```");
        assert_eq!(convert("<pre> <code>x</code></pre>"), "`x`");
        assert_eq!(
            convert("<pre><code>x ``` y\n```\nz</code></pre>"),
            "````\nx ``` y\n```\nz\n````"
        );
        assert_eq!(
            convert("<pre><code class=\"foo language-rs bar\">x</code></pre>"),
            "```rs\nx\n```"
        );
    }

    #[test]
    fn a_list_ending_its_item_adds_one_newline_not_a_blank_line() {
        assert_eq!(
            convert("<ul><li><p>a</p><ul><li>b</li></ul></li></ul>"),
            "-   a\n    \n    -   b"
        );
        assert_eq!(
            convert("<ul><li>a<ul><li>b</li></ul>tail</li></ul>"),
            "-   a\n    -   btail"
        );
    }

    #[test]
    fn summary_and_title_are_inline_and_image_titles_are_not_escaped() {
        assert_eq!(convert("<span>x<summary>d</summary>y</span>"), "xdy");
        assert_eq!(
            convert("<img src=\"x.png\" alt=\"x\" title='a \"b\"'>"),
            "![x](x.png \"a \"b\"\")"
        );
    }
}
