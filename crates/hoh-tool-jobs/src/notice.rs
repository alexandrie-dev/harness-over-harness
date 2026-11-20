//! Completion-notice text: bounded, UTF-8-byte-exact renderings of one settled job.
//!
//! Port of the helper functions of `src/index.ts`.

use hoh_jobs::JobView;
use hoh_llm::{ContentBlock, bound_context_summary};
use hoh_output_retention::{TextRetainer, TextRetentionStrategy};
use hoh_util_values::js_trim_start;

use crate::render::{public_job, snapshot_status_line};

fn retain_tail(text: &str, max_bytes: u64) -> String {
    let mut retainer = TextRetainer::new(TextRetentionStrategy::Tail { max_bytes });
    retainer.push(text);
    retainer.finish().text
}

fn retain_head(text: &str, max_bytes: u64) -> String {
    let mut retainer = TextRetainer::new(TextRetentionStrategy::Head { max_bytes });
    retainer.push(text);
    retainer.finish().text
}

/// `content + suffix`, trimmed from the head of `content` to fit `max_bytes` UTF-8 bytes.
pub fn fit_with_suffix(
    content: &str,
    suffix: &str,
    max_bytes: Option<u64>,
    omitted: &str,
) -> String {
    let complete = format!("{content}{suffix}");
    let Some(max_bytes) = max_bytes else {
        return complete;
    };
    if complete.len() as u64 <= max_bytes {
        return complete;
    }
    let marker = if content.ends_with(js_trim_start(omitted)) {
        ""
    } else {
        omitted
    };
    let fixed = format!("{marker}{suffix}");
    let fixed_bytes = fixed.len() as u64;
    if fixed_bytes >= max_bytes {
        return retain_tail(&fixed, max_bytes);
    }
    format!("{}{fixed}", retain_tail(content, max_bytes - fixed_bytes))
}

/// One-line account of a settled job for the `notice` form's collapsed row.
pub fn completion_summary(job: &JobView) -> String {
    bound_context_summary(&format!(
        "{} {} {}",
        job.kind,
        job.label,
        snapshot_status_line(&public_job(job))
    ))
}

/// The job's label as a notice names it: the first non-empty line, bounded like a notice summary.
/// The model wrote the full command and holds the job id.
fn notice_detail(job: &JobView) -> String {
    let text = js_trim_start(&job.label);
    let label = match text.find('\n') {
        None => bound_context_summary(text),
        Some(newline) => bound_context_summary(&format!("{}…", &text[..newline])),
    };
    format!(
        " ({}: {label}) finished {}",
        job.kind,
        snapshot_status_line(&public_job(job))
    )
}

/// The notice that carries the job's output and final result, when the complete text fits both
/// `max_bytes` and the producer's cap.
pub fn inline_completion_notice(job: &JobView, output: &str, max_bytes: u64) -> Option<String> {
    let head = format!("background job {}{}", job.id, notice_detail(job));
    let text = if output.is_empty() {
        format!("{head}. No new output.")
    } else {
        format!(
            "{head}. Output:\n{}",
            output.strip_suffix('\n').unwrap_or(output)
        )
    };
    let bytes = text.len() as u64;
    (bytes <= max_bytes && bytes <= job.output_limit_bytes.unwrap_or(u64::MAX)).then_some(text)
}

pub fn fit_completion_notice(job: &JobView) -> String {
    let prefix = format!("background job {}", job.id);
    let detail = notice_detail(job);
    let action = "\nDone; job_output.";
    let complete = format!("{prefix}{detail}. Read its output with job_output.");
    let Some(max_bytes) = job.output_limit_bytes else {
        return complete;
    };
    if complete.len() as u64 <= max_bytes {
        return complete;
    }
    let omitted = "\n[notice truncated]";
    let fixed = format!("{prefix}{omitted}{action}");
    let fixed_bytes = fixed.len() as u64;
    if fixed_bytes <= max_bytes {
        return if fixed_bytes == max_bytes {
            fixed
        } else {
            format!(
                "{prefix}{}{omitted}{action}",
                retain_head(&detail, max_bytes - fixed_bytes)
            )
        };
    }
    let compact = format!("{prefix}{action}");
    if compact.len() as u64 <= max_bytes {
        return compact;
    }
    // A partial UUID cannot identify the job. Point the model at the listing instead.
    let recovery = "background job ended; use job_list for id.";
    if recovery.len() as u64 <= max_bytes {
        return recovery.to_owned();
    }
    retain_head("job_list", max_bytes)
}

/// The text of a content list that is exactly one text block.
pub fn raw_single_text(content: &[ContentBlock]) -> Option<String> {
    let [block] = content else {
        return None;
    };
    if block.r#type != "text" {
        return None;
    }
    block.text_value().map(str::to_owned)
}

pub fn bound_single_text(content: &[ContentBlock], max_bytes: u64) -> Option<Vec<ContentBlock>> {
    let text = raw_single_text(content)?;
    Some(vec![ContentBlock::text(fit_with_suffix(
        &text,
        "",
        Some(max_bytes),
        "\n[result truncated]",
    ))])
}
