use hoh_conformance::{Result, read, records};
use serde_json::Value;
use std::{
    io::{BufRead, Write},
    path::Path,
};

pub fn execute() -> Result<u8> {
    let config = std::env::var("HOH_CONFORMANCE_FAKE").map_err(|e| e.to_string())?;
    let config =
        hoh_util_values::js_json_parse(&read(Path::new(&config))?).map_err(|e| e.to_string())?;
    if config.get("plans").is_some() {
        return super::peer::execute(&config);
    }
    let refs = config["refs"].as_str().ok_or("fake refs missing")?;
    let frames = records(&read(
        &Path::new(refs).join("snapshots/acp/handshake/stdout.expected.jsonl"),
    )?)?;
    let mut next = 0;
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let request = hoh_util_values::js_json_parse(&line).map_err(|e| e.to_string())?;
        let mut frame = frames
            .get(next)
            .ok_or("unexpected handshake request")?
            .clone();
        next += 1;
        frame["id"] = request["id"].clone();
        if next == 2 {
            frame["result"]["sessionId"] = Value::from("00000001-0000-4000-8000-000000000001");
        }
        writeln!(
            std::io::stdout(),
            "{}",
            hoh_util_values::js_json_stringify(&frame)
        )
        .map_err(|e| e.to_string())?;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    if next != 2 {
        return Err("stdin EOF before handshake".into());
    }
    Ok(config["exitCode"].as_u64().unwrap_or(0) as u8)
}
