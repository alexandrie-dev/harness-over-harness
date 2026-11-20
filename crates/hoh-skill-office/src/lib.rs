//! Bundled Office workflows and filesystem resources for document authoring and operations.

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use futures_util::future::{BoxFuture, join_all};
use hoh_error::{BoxError, HarnessError, SharedError, node_error_code, node_fs_error_message};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_skill::{
    BUNDLED_SKILL_RANK, SkillCandidate, SkillDefinition, SkillLookupOptions, SkillProvider,
    SkillProviderControl, SkillProviderObservation, SkillProviderOutput, Skills,
};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;
use hoh_util_workspace_path::node_path;
use saphyr_parser::{Event, Parser, ScalarStyle};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const NAME: &str = "skill-office";
pub const INJECT: [&str; 1] = ["skills"];
pub const PROVIDER_NAME: &str = "dsh-office";
pub const SKILL_NAMES: [&str; 4] = ["office", "office-docx", "office-pptx", "office-xlsx"];
const PYTHON_SKILLS: [&str; 3] = ["office-docx", "office-pptx", "office-xlsx"];

pub const WORKSPACE_DEPENDENCIES_SECTION: &str = "Follow an explicit user or applicable AGENTS.md requirement for a Python environment. Otherwise call `load_workspace_dependencies` and run scripts with its Python executable and bundled libraries. Do not install packages or search for a system Python for the default workflow. If the tool is unavailable, use an already configured environment and report a missing dependency only when it prevents the requested operation.";
pub const USER_PYTHON_SECTION: &str = "Follow an explicit user or applicable AGENTS.md requirement for a Python environment. Otherwise use the project's or user's existing Python, such as `python3` or `python` on PATH. Before the first script, check that this Python can import the libraries the workflow needs. If Python or a library is missing, tell the user which packages to install; do not install packages into a global environment without the user's agreement.";

/// Python source named in the Office authoring and verification skills.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PythonEnvironment {
    #[default]
    WorkspaceDependencies,
    User,
}

impl PythonEnvironment {
    const fn section(self) -> &'static str {
        match self {
            Self::WorkspaceDependencies => WORKSPACE_DEPENDENCIES_SECTION,
            Self::User => USER_PYTHON_SECTION,
        }
    }
}

/// Office skill resources and Python environment.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// Absolute assets directory containing one folder per skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub asset_root: Option<String>,
    /// Python environment named in authoring and verification instructions.
    #[serde(default)]
    pub python_environment: PythonEnvironment,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug)]
struct ReadError {
    message: String,
    source: io::Error,
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
struct MetadataError(String);

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for MetadataError {}

enum SkillRead {
    Loaded {
        description: String,
        content: String,
    },
    Failed {
        error: FailureError,
        persistent: bool,
    },
}

enum FailureError {
    Harness(HarnessError),
    Shared(SharedError),
}

impl FailureError {
    fn text(&self) -> String {
        match self {
            Self::Harness(error) => error.to_string(),
            Self::Shared(error) => error.to_string(),
        }
    }

    fn into_box(self) -> BoxError {
        match self {
            Self::Harness(error) => Box::new(error),
            Self::Shared(error) => Box::new(error),
        }
    }
}

#[derive(Clone)]
struct OfficeSkillProvider(Arc<ProviderInner>);

struct ProviderInner {
    ctx: Context,
    control: SkillProviderControl,
    asset_root: String,
    reader: Arc<dyn SkillFileReader>,
    python_environment: PythonEnvironment,
    reported_failures: Mutex<HashMap<String, String>>,
}

impl OfficeSkillProvider {
    fn new(
        ctx: Context,
        control: SkillProviderControl,
        asset_root: String,
        python_environment: PythonEnvironment,
    ) -> Self {
        Self::new_with_reader(
            ctx,
            control,
            asset_root,
            python_environment,
            Arc::new(TokioSkillFileReader),
        )
    }

    fn new_with_reader(
        ctx: Context,
        control: SkillProviderControl,
        asset_root: String,
        python_environment: PythonEnvironment,
        reader: Arc<dyn SkillFileReader>,
    ) -> Self {
        Self(Arc::new(ProviderInner {
            ctx,
            control,
            asset_root,
            reader,
            python_environment,
            reported_failures: Mutex::new(HashMap::new()),
        }))
    }

    async fn read_skill(
        &self,
        skill_name: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<SkillRead, BoxError> {
        let path = node_path::join(&[&self.0.asset_root, skill_name, "SKILL.md"]);
        let bytes = match read_bytes(self.0.reader.as_ref(), Path::new(&path), signal).await {
            Ok(bytes) => bytes,
            Err(ReadOutcome::Aborted(error)) => return Err(Box::new(error)),
            Err(ReadOutcome::Io { error, persistent }) => {
                return Ok(self.failed(skill_name, error, persistent));
            }
        };
        // Node's readFile(path, "utf8") replaces malformed sequences rather than
        // treating them as an I/O failure.
        let raw = String::from_utf8_lossy(&bytes);
        match parse_skill(&raw, Path::new(&path)) {
            Ok((description, content)) => {
                lock(&self.0.reported_failures).remove(skill_name);
                Ok(SkillRead::Loaded {
                    description,
                    content,
                })
            }
            Err(error) => Ok(self.failed(
                skill_name,
                FailureError::Shared(SharedError::new(Box::new(error))),
                true,
            )),
        }
    }

    fn failed(&self, skill_name: &str, error: FailureError, persistent: bool) -> SkillRead {
        let text = error.text();
        let failure = if text.starts_with("YAMLParseError:") {
            text
        } else {
            format!("Error: {text}")
        };
        let mut reported = lock(&self.0.reported_failures);
        if reported.get(skill_name) != Some(&failure) {
            reported.insert(skill_name.into(), failure.clone());
            self.0.ctx.logger().warn(format_args!(
                "skill-office: skill \"{skill_name}\" is unavailable: {failure}"
            ));
        }
        SkillRead::Failed { error, persistent }
    }

    async fn list_inner(
        &self,
        options: Arc<SkillLookupOptions>,
    ) -> Result<SkillProviderOutput, BoxError> {
        let reads = join_all(SKILL_NAMES.into_iter().map(|skill_name| {
            let provider = self.clone();
            let signal = options.signal.clone();
            async move {
                provider
                    .read_skill(skill_name, signal.as_ref())
                    .await
                    .map(|read| (skill_name, read))
            }
        }))
        .await;
        let mut candidates = Vec::with_capacity(SKILL_NAMES.len());
        let mut complete = true;
        for result in reads {
            let (skill_name, read) = result?;
            match read {
                SkillRead::Loaded { description, .. } => {
                    let directory = node_path::join(&[&self.0.asset_root, skill_name]);
                    let locator = node_path::join(&[&directory, "SKILL.md"]);
                    candidates.push(SkillCandidate::from_value(json!({
                        "name": skill_name,
                        "description": description,
                        "invocation": { "modelInvocable": true, "userInvocable": true },
                        "provider": PROVIDER_NAME,
                        "source": "bundled",
                        "rank": BUNDLED_SKILL_RANK,
                        "resourceBase": { "kind": "directory", "path": directory },
                        "locator": locator,
                    })));
                }
                SkillRead::Failed { persistent, .. } => complete &= persistent,
            }
        }
        Ok(if complete {
            SkillProviderOutput::Candidates(candidates)
        } else {
            SkillProviderOutput::Observation(SkillProviderObservation {
                candidates,
                complete: false,
            })
        })
    }

    async fn get_inner(
        &self,
        candidate: SkillCandidate,
        options: Arc<SkillLookupOptions>,
    ) -> Result<Option<SkillDefinition>, BoxError> {
        let name = candidate.text("name").unwrap_or_default();
        let read = self.read_skill(name, options.signal.as_ref()).await?;
        let SkillRead::Loaded {
            description: _,
            mut content,
        } = read
        else {
            let SkillRead::Failed { error, persistent } = read else {
                unreachable!("skill read has exactly two variants")
            };
            if !persistent {
                return Err(error.into_box());
            }
            self.0.control.invalidate();
            return Ok(None);
        };
        if PYTHON_SKILLS.contains(&name) {
            content.push_str("\n\n## Python environment\n\n");
            content.push_str(self.0.python_environment.section());
        }
        let Value::Object(mut summary) = candidate.to_value() else {
            return Err(MetadataError("skill-office: candidate must be an object".into()).into());
        };
        summary.remove("rank");
        summary.remove("locator");
        summary.insert("content".into(), Value::String(content));
        Ok(Some(SkillDefinition::from_value(Value::Object(summary))))
    }
}

impl SkillProvider for OfficeSkillProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn list(
        &self,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>> {
        let provider = self.clone();
        Box::pin(async move { provider.list_inner(options).await })
    }

    fn get(
        &self,
        candidate: SkillCandidate,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>> {
        let provider = self.clone();
        Box::pin(async move { provider.get_inner(candidate, options).await })
    }
}

enum ReadOutcome {
    Aborted(SharedError),
    Io {
        error: FailureError,
        persistent: bool,
    },
}

trait SkillFileReader: Send + Sync {
    fn read(&self, path: PathBuf) -> BoxFuture<'static, io::Result<Vec<u8>>>;
}

struct TokioSkillFileReader;

impl SkillFileReader for TokioSkillFileReader {
    fn read(&self, path: PathBuf) -> BoxFuture<'static, io::Result<Vec<u8>>> {
        Box::pin(async move { tokio::fs::read(path).await })
    }
}

async fn read_bytes(
    reader: &dyn SkillFileReader,
    path: &Path,
    signal: Option<&AbortSignal>,
) -> Result<Vec<u8>, ReadOutcome> {
    if let Some(signal) = signal
        && let Err(error) = signal.throw_if_aborted()
    {
        return Err(ReadOutcome::Aborted(error));
    }
    let read = reader.read(path.to_owned());
    let result = if let Some(signal) = signal {
        tokio::select! {
            biased;
            error = signal.cancelled() => return Err(ReadOutcome::Aborted(error)),
            result = read => result,
        }
    } else {
        read.await
    };
    result.map_err(|error| {
        let persistent = matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
        );
        let code = node_error_code(&error);
        let syscall = if code == Some("EISDIR") {
            "read"
        } else {
            "open"
        };
        let path = path.to_string_lossy();
        let message = node_fs_error_message(&error, syscall, Some(&path), None);
        let error = match code {
            Some(code) => FailureError::Harness(
                HarnessError::named("Error", message, code).with_cause(Box::new(error) as BoxError),
            ),
            None => FailureError::Shared(SharedError::new(Box::new(ReadError {
                message,
                source: error,
            }))),
        };
        ReadOutcome::Io { error, persistent }
    })
}

fn parse_skill(raw: &str, path: &Path) -> Result<(String, String), MetadataError> {
    let Some((frontmatter, body_start)) = frontmatter(raw) else {
        return Err(MetadataError(format!(
            "skill-office: {} has no YAML frontmatter",
            path.display()
        )));
    };
    let description = parse_description(frontmatter).map_err(|error| {
        MetadataError(format!(
            "YAMLParseError: skill-office: {}: {error}",
            path.display()
        ))
    })?;
    let Some(description) = description.filter(|description| !description.is_empty()) else {
        return Err(MetadataError(format!(
            "skill-office: {} has no description",
            path.display()
        )));
    };
    Ok((description, js_trim(&raw[body_start..]).into()))
}

fn frontmatter(raw: &str) -> Option<(&str, usize)> {
    let header_start = if raw.starts_with("---\r\n") {
        5
    } else if raw.starts_with("---\n") {
        4
    } else {
        return None;
    };
    for (relative, _) in raw[header_start..].match_indices("\n---") {
        let newline = header_start + relative;
        let delimiter_end = newline + 4;
        let body_start = if raw[delimiter_end..].starts_with("\r\n") {
            delimiter_end + 2
        } else if raw[delimiter_end..].starts_with('\n') {
            delimiter_end + 1
        } else if delimiter_end == raw.len() {
            delimiter_end
        } else {
            continue;
        };
        let header_end = newline - usize::from(raw.as_bytes()[newline - 1] == b'\r');
        return Some((&raw[header_start..header_end], body_start));
    }
    None
}

fn parse_description(yaml: &str) -> Result<Option<String>, String> {
    let metadata = parse_yaml(yaml)?;
    Ok(metadata
        .as_object()
        .and_then(|metadata| metadata.get("description"))
        .and_then(Value::as_str)
        .map(str::to_owned))
}

fn parse_yaml(text: &str) -> Result<Value, String> {
    let mut events = VecDeque::new();
    for event in Parser::new_from_str(text) {
        events.push_back(event.map_err(|error| error.to_string())?.0);
    }
    if !matches!(events.pop_front(), Some(Event::StreamStart)) {
        return Err("missing YAML stream".into());
    }
    if matches!(events.front(), Some(Event::StreamEnd)) {
        return Ok(Value::Null);
    }
    events.pop_front(); // DocumentStart is guaranteed by saphyr-parser.
    let value = yaml_node(&mut events, &mut HashMap::new(), &mut 0, 0)?;
    if !matches!(events.pop_front(), Some(Event::DocumentEnd))
        || !matches!(events.pop_front(), Some(Event::StreamEnd))
    {
        return Err("expected a single YAML document".into());
    }
    Ok(value)
}

fn yaml_node<'a>(
    events: &mut VecDeque<Event<'a>>,
    anchors: &mut HashMap<usize, Value>,
    expanded: &mut usize,
    depth: usize,
) -> Result<Value, String> {
    if depth > 128 {
        return Err("YAML nesting limit exceeded".into());
    }
    *expanded = expanded.saturating_add(1);
    if *expanded > 100_000 {
        return Err("expanded YAML node limit exceeded (100000)".into());
    }
    let (value, anchor) = match events.pop_front().ok_or("missing YAML node")? {
        Event::Scalar(text, style, anchor, tag) => {
            let suffix = match tag.as_deref() {
                Some(tag)
                    if tag.is_yaml_core_schema()
                        && matches!(
                            tag.suffix.as_ref(),
                            "str" | "null" | "bool" | "int" | "float" | "map" | "seq"
                        ) =>
                {
                    Some(tag.suffix.as_ref())
                }
                // `yaml.parse` keeps unresolved scalar tags as their textual value
                // (and reports a parser warning). The provider has no warning channel,
                // so preserve the accepted value as a string.
                Some(_) => Some("str"),
                None => None,
            };
            (yaml_scalar(&text, style, suffix)?, anchor)
        }
        Event::Alias(id) => {
            let value = anchors
                .get(&id)
                .cloned()
                .ok_or_else(|| "unknown or cyclic YAML alias".to_owned())?;
            *expanded = expanded.saturating_add(yaml_node_count(&value));
            if *expanded > 100_000 {
                return Err("expanded YAML node limit exceeded (100000)".into());
            }
            return Ok(value);
        }
        Event::SequenceStart(anchor, tag) => {
            if !valid_collection_tag(tag.as_deref(), "seq") {
                return Err("unsupported tagged YAML sequence".into());
            }
            let mut items = vec![];
            while !matches!(events.front(), Some(Event::SequenceEnd)) {
                items.push(yaml_node(events, anchors, expanded, depth + 1)?);
            }
            events.pop_front();
            (Value::Array(items), anchor)
        }
        Event::MappingStart(anchor, tag) => {
            if !valid_collection_tag(tag.as_deref(), "map") {
                return Err("unsupported tagged YAML mapping".into());
            }
            let mut map = Map::new();
            let mut seen: Vec<Value> = vec![];
            while !matches!(events.front(), Some(Event::MappingEnd)) {
                let raw_key = yaml_node(events, anchors, expanded, depth + 1)?;
                // `yaml` uniqueKeys compares key values (1 and "1" differ); only the
                // JS object key is stringified afterwards.
                if seen.contains(&raw_key) {
                    return Err(format!("duplicate YAML key {raw_key}"));
                }
                let key = yaml_mapping_key(&raw_key);
                seen.push(raw_key);
                map.insert(key, yaml_node(events, anchors, expanded, depth + 1)?);
            }
            events.pop_front();
            (Value::Object(map), anchor)
        }
        _ => return Err("expected YAML value".into()),
    };
    if anchor != 0 {
        anchors.insert(anchor, value.clone());
    }
    Ok(value)
}

fn valid_collection_tag(tag: Option<&saphyr_parser::Tag>, expected: &str) -> bool {
    tag.is_none_or(|tag| {
        !tag.is_yaml_core_schema()
            || tag.suffix == expected
            || !matches!(
                tag.suffix.as_ref(),
                "str" | "null" | "bool" | "int" | "float" | "map" | "seq"
            )
    })
}

fn yaml_mapping_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        // `yaml` stringifies collection keys (with a warning) instead of failing.
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn yaml_node_count(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1usize.saturating_add(items.iter().map(yaml_node_count).sum()),
        Value::Object(items) => 1usize.saturating_add(items.values().map(yaml_node_count).sum()),
        _ => 1,
    }
}

fn yaml_scalar(text: &str, style: ScalarStyle, tag: Option<&str>) -> Result<Value, String> {
    if tag == Some("str") || tag.is_none() && style != ScalarStyle::Plain {
        return Ok(Value::String(text.into()));
    }
    let automatic = tag.is_none();
    if tag == Some("null") || automatic && matches!(text, "" | "~" | "null" | "Null" | "NULL") {
        return Ok(Value::Null);
    }
    if tag == Some("bool")
        || automatic && matches!(text, "true" | "True" | "TRUE" | "false" | "False" | "FALSE")
    {
        return match text {
            "true" | "True" | "TRUE" => Ok(Value::Bool(true)),
            "false" | "False" | "FALSE" => Ok(Value::Bool(false)),
            _ => Err(format!("invalid explicit boolean {text:?}")),
        };
    }
    if tag == Some("int") || automatic {
        if let Some(integer) = yaml_integer(text) {
            return Ok(integer);
        }
        if tag == Some("int") {
            return Err(format!("invalid explicit integer {text:?}"));
        }
    }
    if tag == Some("float") || automatic {
        if matches!(
            text.trim_start_matches(['+', '-']),
            ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN"
        ) {
            // JSON has no Infinity/NaN representation. Metadata fields other than
            // `description` are ignored, while a non-finite description must remain
            // non-string and fail validation, so null is a lossless type sentinel here.
            return Ok(Value::Null);
        }
        let normalized = if !yaml_core_float(text) {
            String::new()
        } else if let Some(rest) = text.strip_prefix("+.") {
            format!("0.{rest}")
        } else if let Some(rest) = text.strip_prefix("-.") {
            format!("-0.{rest}")
        } else if let Some(rest) = text.strip_prefix('.') {
            format!("0.{rest}")
        } else {
            text.to_owned()
        };
        if let Ok(number) = normalized.parse::<f64>()
            && let Some(number) = serde_json::Number::from_f64(number)
        {
            return Ok(Value::Number(number));
        }
        if tag == Some("float") {
            return Err(format!("invalid explicit float {text:?}"));
        }
    }
    if let Some(tag) = tag {
        return Err(format!("unsupported YAML core tag !!{tag}"));
    }
    Ok(Value::String(text.into()))
}

/// YAML 1.2 core int: `[-+]?[0-9]+`, `0o[0-7]+`, `0x[0-9a-fA-F]+` (no sign, no `_`, no `0b`).
fn yaml_integer(text: &str) -> Option<Value> {
    let (digits, radix) = if let Some(value) = text.strip_prefix("0x") {
        (value, 16)
    } else if let Some(value) = text.strip_prefix("0o") {
        (value, 8)
    } else {
        let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
        if !unsigned.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        (unsigned, 10)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = u64::from_str_radix(digits, radix).ok()?;
    if text.starts_with('-') {
        let magnitude = i128::from(magnitude);
        i64::try_from(-magnitude).ok().map(Value::from)
    } else {
        Some(Value::from(magnitude))
    }
}

/// YAML 1.2 core float: `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`.
fn yaml_core_float(text: &str) -> bool {
    let rest = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        Some(("", fraction)) => !fraction.is_empty() && digits(fraction),
        Some((int, fraction)) => !int.is_empty() && digits(int) && digits(fraction),
        None => !mantissa.is_empty() && digits(mantissa),
    };
    let exponent_ok = exponent.is_none_or(|part| {
        let part = part.strip_prefix(['-', '+']).unwrap_or(part);
        !part.is_empty() && digits(part)
    });
    mantissa_ok && exponent_ok
}

fn packaged_asset_root() -> String {
    node_path::join(&[env!("CARGO_MANIFEST_DIR"), "assets"])
}

#[derive(Inject)]
pub struct Deps {
    skills: Service<Skills>,
}

pub struct SkillOfficePlugin;

impl Plugin for SkillOfficePlugin {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        let asset_root = config
            .asset_root
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(packaged_asset_root);
        if !node_path::is_absolute(&asset_root) {
            return Err(MetadataError(
                "skill-office: assetRoot must be an absolute directory".into(),
            )
            .into());
        }
        let asset_root = node_path::normalize(&asset_root);
        let provider_ctx = ctx.clone();
        let python_environment = config.python_environment;
        deps.skills.register_provider(&ctx, move |control| {
            Ok(Arc::new(OfficeSkillProvider::new(
                provider_ctx,
                control,
                asset_root,
                python_environment,
            )))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;
    use hoh_skill::{SkillRegistry, SkillViewOptions};
    use hoh_timeout::{AbortController, is_abort_error};

    struct ScriptedReader {
        target: PathBuf,
        failures: Mutex<VecDeque<bool>>,
    }

    impl SkillFileReader for ScriptedReader {
        fn read(&self, path: PathBuf) -> BoxFuture<'static, io::Result<Vec<u8>>> {
            let fail = path == self.target && lock(&self.failures).pop_front().unwrap_or(false);
            Box::pin(async move {
                if fail {
                    Err(io::Error::other("temporary read failure"))
                } else {
                    tokio::fs::read(path).await
                }
            })
        }
    }

    #[tokio::test]
    async fn direct_provider_list_honors_an_already_aborted_signal_without_warning() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SkillRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let holder = Arc::new(Mutex::new(None));
        let output = holder.clone();
        let provider_ctx = ctx.clone();
        ctx.get::<Skills>()
            .unwrap()
            .register_provider(&ctx, move |control| {
                let provider = OfficeSkillProvider::new(
                    provider_ctx,
                    control,
                    "/does/not/matter".into(),
                    PythonEnvironment::WorkspaceDependencies,
                );
                *lock(&output) = Some(provider.clone());
                Ok(Arc::new(provider))
            })
            .unwrap();
        let provider = lock(&holder).clone().unwrap();
        let abort = AbortController::new();
        abort.abort();
        let error = provider
            .list(Arc::new(SkillViewOptions {
                signal: Some(abort.signal()),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert!(is_abort_error(error.as_ref()));
        assert!(
            ctx.logger()
                .buffer()
                .iter()
                .all(|record| !record.message.contains("skill-office: skill"))
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn direct_provider_normalizes_resource_and_locator_paths() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SkillRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let holder = Arc::new(Mutex::new(None));
        let output = holder.clone();
        let provider_ctx = ctx.clone();
        let spelled_root = format!("{}/assets/../assets/.", env!("CARGO_MANIFEST_DIR"));
        let normalized_root = node_path::normalize(&spelled_root);
        ctx.get::<Skills>()
            .unwrap()
            .register_provider(&ctx, move |control| {
                let provider = OfficeSkillProvider::new(
                    provider_ctx,
                    control,
                    spelled_root,
                    PythonEnvironment::WorkspaceDependencies,
                );
                *lock(&output) = Some(provider.clone());
                Ok(Arc::new(provider))
            })
            .unwrap();
        let provider = lock(&holder).clone().unwrap();
        let SkillProviderOutput::Candidates(candidates) = provider
            .list(Arc::new(SkillViewOptions::default()))
            .await
            .unwrap()
        else {
            panic!("packaged provider must produce a complete candidate list")
        };
        let candidate = candidates
            .into_iter()
            .find(|candidate| candidate.text("name") == Some("office-docx"))
            .unwrap();
        assert_eq!(
            candidate.get("resourceBase"),
            Some(&json!({
                "kind": "directory",
                "path": node_path::join(&[&normalized_root, "office-docx"]),
            }))
        );
        let expected_locator = node_path::join(&[&normalized_root, "office-docx", "SKILL.md"]);
        assert_eq!(candidate.text("locator"), Some(expected_locator.as_str()));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn no_code_read_error_is_incomplete_recovers_and_is_rethrown_by_get() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SkillRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let asset_root = packaged_asset_root();
        let reader = Arc::new(ScriptedReader {
            target: PathBuf::from(node_path::join(&[&asset_root, "office-docx", "SKILL.md"])),
            failures: Mutex::new([true, false, true, false].into()),
        });
        let provider_ctx = ctx.clone();
        ctx.get::<Skills>()
            .unwrap()
            .register_provider(&ctx, move |control| {
                Ok(Arc::new(OfficeSkillProvider::new_with_reader(
                    provider_ctx,
                    control,
                    asset_root,
                    PythonEnvironment::WorkspaceDependencies,
                    reader,
                )))
            })
            .unwrap();

        let incomplete = ctx
            .get::<Skills>()
            .unwrap()
            .snapshot(Arc::new(SkillViewOptions::default()))
            .await
            .unwrap();
        assert!(!incomplete.complete);
        assert_eq!(incomplete.skills.len(), 3);
        let recovered = ctx
            .get::<Skills>()
            .unwrap()
            .snapshot(Arc::new(SkillViewOptions::default()))
            .await
            .unwrap();
        assert!(recovered.complete);
        assert_eq!(recovered.skills.len(), 4);

        let error = ctx
            .get::<Skills>()
            .unwrap()
            .get("office-docx", Arc::new(SkillViewOptions::default()))
            .await
            .unwrap_err();
        let expected = format!(
            "temporary read failure, open '{}'",
            node_path::join(&[&packaged_asset_root(), "office-docx", "SKILL.md"])
        );
        assert_eq!(error.to_string(), expected);
        assert!(
            ctx.get::<Skills>()
                .unwrap()
                .get("office-docx", Arc::new(SkillViewOptions::default()))
                .await
                .unwrap()
                .is_some()
        );
        kernel.shutdown().await;
    }
}
