//! Tool render-intent vocabulary: the provider-neutral types a tool declares via
//! `ToolDefinition::present_call`/`ToolDefinition::present_result` to say how one of its calls
//! renders in a UI (an editor's tool-call card, a CLI log line).

use hoh_llm::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Keep an explicit `null` as `Some(Value::Null)`: `Option` alone reads it as an absent field.
fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// Category of a tool call, used by a UI to pick an icon or treatment; `other` is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Fetch,
    Other,
}

/// A file location a tool reads or modifies, so a capable UI can "follow along".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileLocation {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

/// A single-file change a tool is about to make, for a UI that renders inline diffs.
/// `old_text` is `null` for a new-file create or an overwrite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: String,
}

/// Provider-neutral pending-call presentation. Tools declare one tagged intent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "card", rename_all = "lowercase")]
pub enum ToolCallView {
    Generic(GenericCallView),
    Terminal(TerminalCallView),
    Diff(DiffCallView),
}

/// The default card: a titled tool-call row with an optional category icon, a salient raw input,
/// extra content blocks, and follow-along file locations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericCallView {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolCallKind>,
    /// `None` is an absent field; `Some(Value::Null)` is an explicit `null`.
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub raw_input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<FileLocation>>,
}

/// A call that IS a shell command running in a working directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerminalCallView {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// A call that creates or modifies files, rendered as an inline diff card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiffCallView {
    pub title: String,
    pub diffs: Vec<FileDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<FileLocation>>,
}

/// How a tool wants the COMPLETED call shown, a `card`-tagged union mirroring [`ToolCallView`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "card", rename_all = "lowercase")]
pub enum ToolResultView {
    Generic(GenericResultView),
    Terminal(TerminalResultView),
    Diff(DiffResultView),
    Search(SearchResultView),
    Read(ReadResultView),
    Web(WebResultView),
}

/// The default completed card: an optional replacement title and reformatted content.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GenericResultView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentBlock>>,
}

/// The completed state of a [`TerminalCallView`]: the captured output and exit status.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResultView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
}

/// A completed file mutation rendered as an inline diff card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiffResultView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub diffs: Vec<FileDiff>,
}

/// One matched line inside a [`SearchFileMatches`] group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLineMatch {
    pub line_number: u64,
    pub line: String,
}

/// One file's grouped content matches, in first-seen file order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchFileMatches {
    pub path: String,
    pub matches: Vec<SearchLineMatch>,
}

/// A completed search rendered as a search card, discriminated by `shape`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "lowercase")]
pub enum SearchResultView {
    /// A completed content search (`grep`) grouped by file.
    Matches {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        files: Vec<SearchFileMatches>,
        truncated: bool,
        total: u64,
    },
    /// A completed path search (`glob`) as a flat path list.
    Paths {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        paths: Vec<String>,
        truncated: bool,
        total: u64,
    },
}

/// One numbered line of a file, the unit a [`ReadResultView`] carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadFileLine {
    pub number: u64,
    pub text: String,
}

/// A completed file read rendered as a line-numbered, optionally syntax-highlighted code view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResultView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub path: String,
    pub offset: u64,
    pub lines: Vec<ReadFileLine>,
    pub total_lines: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentBlock>>,
}

/// One citeable source in a completed [`WebResultView::Search`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSource {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

/// A completed web retrieval rendered as a structured card, discriminated by `kind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WebResultView {
    /// The completed state of a `web_search` call.
    Search {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        sources: Vec<WebSource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
        truncated: bool,
    },
    /// The completed state of a `web_fetch` call.
    Fetch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        url: String,
        #[serde(rename = "statusCode")]
        status_code: u16,
        truncated: bool,
    },
}
