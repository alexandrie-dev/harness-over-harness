//! Dependency-free parsing for the standalone mock server.

use indexmap::IndexMap;
use serde_json::Value;

use crate::{
    MAX_MOCK_LLM_TIMER_DELAY_MS, MockLlmBehavior, MockLlmServerError, MockLlmServerOptions,
};

pub const CONNECTION_REFUSED_BEHAVIOR: &str = "connection_refused";

pub const MOCK_LLM_CLI_USAGE: &str = "Usage: dsh-llm-mock-server [options]\n\nRequired:\n  --sequence <a,b,...>       Ordered behaviors; connection_refused is allowed first\n\nListener:\n  --host <host>              Default 127.0.0.1\n  --port <port>              Default 8000; required and nonzero for connection_refused\n  --api-key <token>          Validate exact Bearer token when present\n  --listen-delay-ms <ms>     Unavailable interval (default 750 with connection_refused)\n  --repeat-last              Repeat the final request behavior after exhaustion\n  --seed <uint32>            Reproduce random selections\n  --random-weights <a=n,...> Relative weights for concrete behaviors\n\nResponse:\n  --success-text <text>\n  --partial-text <text>\n  --reasoning-text <text>\n  --chunk-size <count>\n  --chunk-delay-ms <ms>\n  --disconnect-delay-ms <ms>\n  --retry-after-ms <ms>\n  --request-id <id>\n  --tool-name <name>\n  --tool-arguments <json>\n\nOther:\n  --help\n";

pub struct MockLlmCliConfig {
    pub server: MockLlmServerOptions,
    pub listen_delay_ms: u64,
    pub starts_unavailable: bool,
}

pub enum MockLlmCliParseResult {
    Help,
    Run(Box<MockLlmCliConfig>),
}

const STRING_OPTIONS: &[&str] = &[
    "sequence",
    "host",
    "port",
    "api-key",
    "listen-delay-ms",
    "seed",
    "random-weights",
    "success-text",
    "partial-text",
    "reasoning-text",
    "chunk-size",
    "chunk-delay-ms",
    "disconnect-delay-ms",
    "retry-after-ms",
    "request-id",
    "tool-name",
    "tool-arguments",
];

fn tokenize(argv: &[String]) -> Result<(IndexMap<String, String>, bool), MockLlmServerError> {
    let mut values = IndexMap::new();
    let mut repeat_last = false;
    let mut index = 0;
    while index < argv.len() {
        let argument = &argv[index];
        if argument == "--repeat-last" {
            repeat_last = true;
            index += 1;
            continue;
        }
        if !argument.starts_with("--") {
            return Err(MockLlmServerError::new(format!(
                "Unexpected argument '{argument}'"
            )));
        }
        let option = &argument[2..];
        let (name, inline) = option
            .split_once('=')
            .map_or((option, None), |(name, value)| (name, Some(value)));
        if !STRING_OPTIONS.contains(&name) {
            return Err(MockLlmServerError::new(format!(
                "Unknown option '--{name}'"
            )));
        }
        let value = if let Some(value) = inline {
            value.to_owned()
        } else {
            let Some(value) = argv.get(index + 1).filter(|value| !value.starts_with('-')) else {
                return Err(MockLlmServerError::new(format!(
                    "Option '--{name} <value>' argument missing"
                )));
            };
            index += 1;
            value.clone()
        };
        values.insert(name.to_owned(), value);
        index += 1;
    }
    Ok((values, repeat_last))
}

fn number_value(option: &str, value: &str) -> Result<f64, MockLlmServerError> {
    let trimmed = hoh_util_values::js_trim(value);
    let parsed = javascript_number(trimmed);
    if !parsed.is_finite() {
        return Err(MockLlmServerError::new(format!(
            "dsh-llm-mock-server: {option} must be a finite number"
        )));
    }
    Ok(parsed)
}

fn javascript_number(value: &str) -> f64 {
    if value.is_empty() {
        return 0.0;
    }
    let (radix, digits) = if let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (16, digits)
    } else if let Some(digits) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        (2, digits)
    } else if let Some(digits) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        (8, digits)
    } else {
        return value.parse::<f64>().unwrap_or(f64::NAN);
    };
    if digits.is_empty() {
        return f64::NAN;
    }
    digits
        .chars()
        .try_fold(0.0, |number, digit| {
            digit
                .to_digit(radix)
                .map(|digit| number * f64::from(radix) + f64::from(digit))
        })
        .unwrap_or(f64::NAN)
}

fn bounded_integer(
    option: &str,
    value: &str,
    min: u64,
    max: u64,
) -> Result<u64, MockLlmServerError> {
    let parsed = number_value(option, value)?;
    if parsed.fract() != 0.0 || parsed < min as f64 || parsed > max as f64 {
        return Err(MockLlmServerError::new(format!(
            "dsh-llm-mock-server: {option} must be an integer between {min} and {max}"
        )));
    }
    Ok(parsed as u64)
}

fn quoted(value: &str) -> String {
    hoh_util_values::js_json_stringify(&Value::String(value.to_owned()))
}

fn parse_sequence(raw: &str) -> Result<(bool, Vec<MockLlmBehavior>), MockLlmServerError> {
    let entries: Vec<&str> = raw.split(',').map(hoh_util_values::js_trim).collect();
    if entries.iter().any(|entry| entry.is_empty()) {
        return Err(MockLlmServerError::new(
            "dsh-llm-mock-server: --sequence must contain non-empty comma-separated behaviors",
        ));
    }
    let unavailable = entries.first() == Some(&CONNECTION_REFUSED_BEHAVIOR);
    if entries
        .iter()
        .skip(1)
        .any(|entry| *entry == CONNECTION_REFUSED_BEHAVIOR)
    {
        return Err(MockLlmServerError::new(
            "dsh-llm-mock-server: connection_refused is allowed only as the first behavior",
        ));
    }
    let request_entries = if unavailable {
        &entries[1..]
    } else {
        &entries[..]
    };
    if request_entries.is_empty() {
        return Err(MockLlmServerError::new(
            "dsh-llm-mock-server: connection_refused must be followed by a request behavior",
        ));
    }
    let mut sequence = Vec::new();
    for entry in request_entries {
        let Some(behavior) = MockLlmBehavior::parse(entry) else {
            return Err(MockLlmServerError::new(format!(
                "dsh-llm-mock-server: unknown behavior {}",
                quoted(entry)
            )));
        };
        sequence.push(behavior);
    }
    Ok((unavailable, sequence))
}

fn parse_weights(raw: &str) -> Result<IndexMap<String, f64>, MockLlmServerError> {
    let mut weights = IndexMap::new();
    for entry in raw.split(',') {
        let parts: Vec<&str> = entry.split('=').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(MockLlmServerError::new(
                "dsh-llm-mock-server: --random-weights expects behavior=weight comma-separated entries",
            ));
        }
        let behavior = parts[0];
        if MockLlmBehavior::parse(behavior).is_none_or(|value| value == MockLlmBehavior::Random) {
            return Err(MockLlmServerError::new(format!(
                "dsh-llm-mock-server: random weight requires a concrete behavior, got {}",
                quoted(behavior)
            )));
        }
        if weights.contains_key(behavior) {
            return Err(MockLlmServerError::new(format!(
                "dsh-llm-mock-server: duplicate random weight for {}",
                quoted(behavior)
            )));
        }
        weights.insert(
            behavior.to_owned(),
            number_value("--random-weights", parts[1])?,
        );
    }
    Ok(weights)
}

/// Parse arguments after the executable name.
pub fn parse_mock_llm_cli_args(
    argv: &[String],
) -> Result<MockLlmCliParseResult, MockLlmServerError> {
    if argv.iter().any(|argument| argument == "--help") {
        return Ok(MockLlmCliParseResult::Help);
    }
    let (mut values, repeat_last) = tokenize(argv)?;
    let sequence_raw = values
        .shift_remove("sequence")
        .ok_or_else(|| MockLlmServerError::new("dsh-llm-mock-server: --sequence is required"))?;
    let (starts_unavailable, sequence) = parse_sequence(&sequence_raw)?;
    let port = values
        .shift_remove("port")
        .map_or(Ok(8_000.0), |value| number_value("--port", &value))?;
    let listen_delay = values
        .shift_remove("listen-delay-ms")
        .map(|value| bounded_integer("--listen-delay-ms", &value, 0, MAX_MOCK_LLM_TIMER_DELAY_MS))
        .transpose()?;
    let random_seed = values
        .shift_remove("seed")
        .map(|value| number_value("--seed", &value))
        .transpose()?;
    let random_weights = values
        .shift_remove("random-weights")
        .map(|value| parse_weights(&value))
        .transpose()?;
    if starts_unavailable && port == 0.0 {
        return Err(MockLlmServerError::new(
            "dsh-llm-mock-server: connection_refused requires an explicit nonzero --port",
        ));
    }
    if !starts_unavailable && listen_delay.is_some() {
        return Err(MockLlmServerError::new(
            "dsh-llm-mock-server: --listen-delay-ms requires connection_refused first in --sequence",
        ));
    }
    if !sequence.contains(&MockLlmBehavior::Random)
        && (random_seed.is_some() || random_weights.is_some())
    {
        return Err(MockLlmServerError::new(
            "dsh-llm-mock-server: --seed and --random-weights require random in --sequence",
        ));
    }
    let mut server = MockLlmServerOptions {
        sequence,
        port: Some(port),
        repeat_last: Some(repeat_last),
        random_seed,
        random_weights,
        ..MockLlmServerOptions::default()
    };
    server.host = values.shift_remove("host");
    server.api_key = values.shift_remove("api-key");
    server.success_text = values.shift_remove("success-text");
    server.partial_text = values.shift_remove("partial-text");
    server.reasoning_text = values.shift_remove("reasoning-text");
    server.chunk_size = values
        .shift_remove("chunk-size")
        .map(|value| number_value("--chunk-size", &value))
        .transpose()?;
    server.chunk_delay_ms = values
        .shift_remove("chunk-delay-ms")
        .map(|value| number_value("--chunk-delay-ms", &value))
        .transpose()?;
    server.disconnect_delay_ms = values
        .shift_remove("disconnect-delay-ms")
        .map(|value| number_value("--disconnect-delay-ms", &value))
        .transpose()?;
    server.retry_after_ms = values
        .shift_remove("retry-after-ms")
        .map(|value| number_value("--retry-after-ms", &value))
        .transpose()?;
    server.request_id = values.shift_remove("request-id");
    server.tool_name = values.shift_remove("tool-name");
    server.tool_arguments = values.shift_remove("tool-arguments");
    Ok(MockLlmCliParseResult::Run(Box::new(MockLlmCliConfig {
        server,
        listen_delay_ms: if starts_unavailable {
            listen_delay.unwrap_or(750)
        } else {
            0
        },
        starts_unavailable,
    })))
}
