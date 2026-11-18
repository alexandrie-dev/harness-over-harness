use hoh_protocol::transport::FrameReader;
use hoh_window_stub::{Engine, recording::Recording};
use std::{io, path::PathBuf, sync::mpsc, thread, time::Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("hoh-window-stub: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = std::env::var_os("HOH_DSH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"));
    let mut scenario = String::from("text-turn");
    let mut recording = None;
    let mut fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/fixtures");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--help" {
            println!(
                "hoh-window-stub [--scenario text-turn|approval|plan-review] [--source-root PATH] [--recording PATH] [--fixtures PATH]"
            );
            return Ok(());
        }
        let value = args.next().ok_or("option requires a value")?;
        match arg.as_str() {
            "--source-root" => source = value.into(),
            "--scenario" => scenario = value,
            "--recording" => recording = Some(PathBuf::from(value)),
            "--fixtures" => fixtures = value.into(),
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }
    if !scenario
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || scenario.is_empty()
    {
        return Err("invalid scenario name".into());
    }
    let path = recording.unwrap_or_else(|| {
        source
            .join("snapshots/sdk")
            .join(scenario)
            .join("session.v4.jsonl")
    });
    let recording = Recording::read(&path)?;
    let (tx, rx) = mpsc::sync_channel(8);
    thread::spawn(move || {
        let input = io::stdin();
        let mut reader = FrameReader::new(input.lock());
        loop {
            let frame = reader.read();
            let finished = !matches!(frame, Ok(Some(_)));
            if tx.send(frame).is_err() || finished {
                break;
            }
        }
    });
    let output = io::stdout();
    let mut engine = Engine::new(output.lock(), recording, fixtures);
    loop {
        let incoming = if let Some(deadline) = engine.deadline() {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(frame) => frame?,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    engine.expire()?;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("JSON-RPC input closed".into());
                }
            }
        } else {
            rx.recv()?.map_err(io::Error::other)?
        };
        match incoming {
            Some(frame) => engine.accept(frame)?,
            None => {
                if engine.has_pending() {
                    engine.expire()?;
                    return Err("JSON-RPC input closed during host request".into());
                }
                break;
            }
        }
        if engine.stopped() {
            break;
        }
    }
    Ok(())
}
