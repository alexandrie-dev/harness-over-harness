use hoh_conformance::{
    Result, corpus,
    runner::{self, Options},
};
use std::{io::Write, process::ExitCode, sync::Arc};

async fn execute() -> Result<bool> {
    let options = Arc::new(Options::parse(std::env::args().skip(1))?);
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
                "hoh-conformance: {e}\nusage: hoh-conformance --hoh PATH --refs CLONE --runs DIR [--suite session|sdk|acp] [--scenario NAME] [--jobs N] [--results TSV] [--timeout-ms N] [--request-timeout-ms N] [--wait-timeout-ms N]"
            );
            ExitCode::FAILURE
        }
    }
}
