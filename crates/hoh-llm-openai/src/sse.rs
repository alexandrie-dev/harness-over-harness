//! Incremental Server-Sent Events framing.
//!
//! Robust to lines split across network chunks (also inside a UTF-8 sequence and between `\r`
//! and `\n`), to `\n`, `\r\n` and `\r` terminators, comments (keep-alives) and a BOM. An
//! unterminated tail is not an event, with one exception: a bare `data: [DONE]`.

/// Longest line accepted; a stream that never ends a line is broken, not slow.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SseOverflow;

#[derive(Default)]
pub struct SseParser {
    line: Vec<u8>,
    skip_lf: bool,
    started: bool,
    data: Option<String>,
    event: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, SseOverflow> {
        let mut events = Vec::new();
        for &byte in bytes {
            if std::mem::take(&mut self.skip_lf) && byte == b'\n' {
                continue;
            }
            match byte {
                b'\n' | b'\r' => {
                    self.skip_lf = byte == b'\r';
                    let line = std::mem::take(&mut self.line);
                    self.process_line(&line, &mut events);
                }
                _ => {
                    if self.line.len() >= MAX_LINE_BYTES {
                        return Err(SseOverflow);
                    }
                    self.line.push(byte);
                }
            }
        }
        Ok(events)
    }

    /// End of input. Returns the tolerated final `[DONE]` if the stream ended mid-event.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let mut events = Vec::new();
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.process_line(&line, &mut events);
        }
        let pending = self.data.take()?;
        let data = pending.strip_suffix('\n').unwrap_or(&pending);
        (data.trim() == "[DONE]").then(|| SseEvent {
            event: self.event.take(),
            data: data.trim().to_owned(),
        })
    }

    fn process_line(&mut self, raw: &[u8], events: &mut Vec<SseEvent>) {
        let mut line = String::from_utf8_lossy(raw).into_owned();
        if !self.started {
            self.started = true;
            if let Some(rest) = line.strip_prefix('\u{feff}') {
                line = rest.to_owned();
            }
        }
        if line.is_empty() {
            let event = self.event.take();
            if let Some(mut data) = self.data.take() {
                data.pop(); // the '\n' appended after every data line
                events.push(SseEvent { event, data });
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_str(), ""),
        };
        match field {
            "data" => {
                let data = self.data.get_or_insert_with(String::new);
                data.push_str(value);
                data.push('\n');
            }
            "event" => self.event = Some(value.to_owned()),
            _ => {}
        }
    }
}
