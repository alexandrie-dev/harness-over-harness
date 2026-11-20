use hoh_conformance::{
    Result, corpus,
    runner::{self, Options},
    v1,
};
use std::{io::Write, process::ExitCode, sync::Arc};

/// `--v1 LIST [--group in|extra|all] --out DIR`: run the list's selection and write
/// `DIR/summary.tsv`, `DIR/diffs/`, `DIR/runs/` instead of the plain results table.
struct V1Run {
    list: std::path::PathBuf,
    group: String,
    out: std::path::PathBuf,
}

fn take_v1(args: Vec<String>) -> Result<(Vec<String>, Option<V1Run>)> {
    let (mut rest, mut list, mut group, mut out) = (Vec::new(), None, "all".to_owned(), None);
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--v1" | "--group" | "--out" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--v1" => list = Some(std::path::PathBuf::from(value)),
                    "--group" => group = value,
                    _ => out = Some(std::path::PathBuf::from(value)),
                }
            }
            _ => rest.push(flag),
        }
    }
    let Some(list) = list else {
        return Ok((rest, None));
    };
    let out = out.ok_or("--v1 requires --out")?;
    let out = if out.is_absolute() {
        out
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(out)
    };
    for (flag, value) in [
        ("--runs", out.join("runs")),
        ("--results", out.join("results.tsv")),
    ] {
        if !rest.iter().any(|a| a == flag) {
            rest.extend([flag.to_owned(), value.to_string_lossy().into_owned()]);
        }
    }
    Ok((rest, Some(V1Run { list, group, out })))
}

async fn execute() -> Result<bool> {
    let (args, v1_run) = take_v1(std::env::args().skip(1).collect())?;
    let mut options = Options::parse(args)?;
    let mut rows = match &v1_run {
        Some(run) => v1::rows(&hoh_conformance::read(&run.list)?, &run.group)?,
        None => Vec::new(),
    };
    // `--suite` narrows the list's selection; it never adds scenarios.
    rows.retain(|r| options.suites.is_empty() || options.suites.contains(&r.suite));
    if v1_run.is_some() {
        // `--scenario` narrows the list, as `--suite` does; it never adds scenarios.
        let wanted = std::mem::take(&mut options.scenarios);
        rows.retain(|r| {
            wanted.is_empty()
                || wanted.iter().any(|name| {
                    name == &r.scenario || name == &format!("{}/{}", r.suite, r.scenario)
                })
        });
        if rows.is_empty() {
            // An empty scenario list means "everything" below; an empty selection must not.
            return Err("no scenarios selected".into());
        }
        options.scenarios = rows
            .iter()
            .map(|r| format!("{}/{}", r.suite, r.scenario))
            .collect();
    }
    let options = Arc::new(options);
    let all = Arc::new(corpus::discover(&options.refs)?);
    let selected: Vec<_> =
        all.iter()
            .filter(|s| {
                (options.suites.is_empty() || options.suites.contains(&s.suite))
                    && (options.scenarios.is_empty()
                        || options.scenarios.iter().any(|name| {
                            name == &s.name || name == &format!("{}/{}", s.suite, s.name)
                        }))
            })
            .cloned()
            .collect();
    if selected.is_empty() {
        return Err("no scenarios selected".into());
    }
    for selector in &options.scenarios {
        if !selected
            .iter()
            .any(|s| selector == &s.name || selector == &format!("{}/{}", s.suite, s.name))
        {
            return Err(format!("scenario selector matched nothing: {selector}"));
        }
    }
    let mut pending = selected.into_iter().enumerate();
    let mut tasks = tokio::task::JoinSet::new();
    let mut outcomes = Vec::new();
    loop {
        while tasks.len() < options.jobs {
            let Some((i, s)) = pending.next() else { break };
            let options = options.clone();
            let all = all.clone();
            tasks.spawn(async move { (i, runner::run(&options, &all, &s).await) });
        }
        let Some(result) = tasks.join_next().await else {
            break;
        };
        outcomes.push(result.map_err(|e| format!("runner task failed: {e}"))?);
    }
    outcomes.sort_by_key(|(i, _)| *i);
    let outcomes: Vec<_> = outcomes.into_iter().map(|(_, o)| o).collect();
    for outcome in &outcomes {
        if let Some(error) = &outcome.runner_error {
            writeln!(
                std::io::stderr(),
                "runner error {}/{}: {error}",
                outcome.suite,
                outcome.scenario
            )
            .map_err(|e| e.to_string())?;
        }
    }
    writeln!(
        std::io::stdout(),
        "{}",
        runner::report(&options, &outcomes)?
    )
    .map_err(|e| e.to_string())?;
    if let Some(run) = &v1_run {
        let entries: Vec<_> = rows
            .into_iter()
            .map(|row| {
                let outcome = outcomes
                    .iter()
                    .find(|o| o.suite == row.suite && o.scenario == row.scenario);
                let verdict = match outcome {
                    Some(o) => v1::classify(o, &v1::stderr_of(&options.runs, &row)),
                    None => v1::Verdict {
                        status: v1::Status::NotStarted,
                        phase: "corpus".into(),
                        body_check: "-".into(),
                        detail: "scenario is absent from the corpus".into(),
                    },
                };
                v1::Entry { row, verdict }
            })
            .collect();
        for entry in &entries {
            v1::write_evidence(&run.out, &options.runs, entry)?;
        }
        std::fs::write(run.out.join("summary.tsv"), v1::summary_tsv(&entries))
            .map_err(|e| e.to_string())?;
        writeln!(std::io::stdout(), "{}", v1::totals(&entries)).map_err(|e| e.to_string())?;
    }
    Ok(outcomes.iter().all(|o| o.runner_error.is_none()))
}

#[tokio::main(worker_threads = 4)]
async fn main() -> ExitCode {
    match execute().await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            let _ = writeln!(
                std::io::stderr(),
                "hoh-conformance: {e}\nusage: hoh-conformance --hoh PATH --refs CLONE --runs DIR [--suite session|sdk|acp] [--scenario NAME] [--jobs N] [--results TSV] [--timeout-ms N] [--request-timeout-ms N] [--wait-timeout-ms N] [--disable-plugin ID]..."
            );
            ExitCode::FAILURE
        }
    }
}
