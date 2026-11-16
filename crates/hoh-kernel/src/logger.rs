use crate::*;
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::Weak,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}
#[derive(Clone, Debug)]
pub struct Record {
    pub sn: u64,
    pub ts_ms: i64,
    pub name: Arc<str>,
    pub level: Level,
    pub message: String,
    pub fiber: u32,
}
#[derive(Clone, Debug, Default)]
pub struct Levels {
    pub default: Option<Level>,
    pub by_name: HashMap<String, Level>,
}
struct Export {
    effect: Weak<effects::Effect>,
    levels: Levels,
    callback: Arc<dyn Fn(&Record) + Send + Sync>,
}
pub(crate) struct Logs {
    sn: u64,
    records: VecDeque<Record>,
    exporters: Vec<Arc<Export>>,
    start: tokio::time::Instant,
    epoch_ms: i64,
}
impl Default for Logs {
    fn default() -> Self {
        Self {
            sn: 0,
            records: VecDeque::new(),
            exporters: vec![],
            start: tokio::time::Instant::now(),
            epoch_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or(0),
        }
    }
}
#[derive(Clone)]
pub struct Logger {
    pub(crate) logs: Arc<Mutex<Logs>>,
    pub(crate) inner: Weak<fiber::Shared>,
    pub(crate) activation: Weak<effects::Activation>,
    pub(crate) data: Weak<fiber::FiberData>,
    pub(crate) name: Arc<str>,
    pub(crate) owner_name: Arc<str>,
    pub(crate) uid: u32,
    pub(crate) level: Option<Level>,
}
impl Logger {
    fn now_ms(&self) -> i64 {
        let logs = lock(&self.logs);
        logs.epoch_ms
            .saturating_add(logs.start.elapsed().as_millis().min(i64::MAX as u128) as i64)
    }
    pub(crate) fn fiber_name(&self) -> &str {
        &self.owner_name
    }
    pub fn named(&self, name: &str) -> Self {
        let mut logger = self.clone();
        logger.name = name.into();
        logger
    }
    pub fn error(&self, message: impl fmt::Display) {
        self.record(Level::Error, message);
    }
    pub fn warn(&self, message: impl fmt::Display) {
        self.record(Level::Warn, message);
    }
    pub fn info(&self, message: impl fmt::Display) {
        self.record(Level::Info, message);
    }
    pub fn debug(&self, message: impl fmt::Display) {
        self.record(Level::Debug, message);
    }
    pub(crate) fn kernel_error(&self, error: &KernelError) {
        let mut pending: Vec<&(dyn std::error::Error + Send + Sync + 'static)> = vec![error];
        while let Some(error) = pending.pop() {
            match error.downcast_ref::<KernelError>() {
                Some(KernelError::Listeners { errors, .. }) if !errors.is_empty() => {
                    pending.extend(errors.iter().rev().map(|e| e.as_ref().as_ref()))
                }
                Some(KernelError::Failed(error)) => pending.push(error.as_ref().as_ref()),
                _ => self.error(error),
            }
        }
    }
    fn record(&self, level: Level, message: impl fmt::Display) {
        let message = message.to_string();
        let (record, exporters) = {
            let mut logs = lock(&self.logs);
            logs.sn += 1;
            let record = Record {
                sn: logs.sn,
                ts_ms: logs
                    .epoch_ms
                    .saturating_add(logs.start.elapsed().as_millis().min(i64::MAX as u128) as i64),
                name: self.name.clone(),
                level,
                message,
                fiber: self.uid,
            };
            if self.level.unwrap_or(Level::Info) >= level {
                logs.records.push_back(record.clone());
                if logs.records.len() > 1000 {
                    logs.records.pop_front();
                }
            }
            (record, logs.exporters.clone())
        };
        for export in exporters {
            if export.effect.upgrade().is_none_or(|e| e.is_running()) {
                continue;
            }
            if export
                .levels
                .by_name
                .get(self.name.as_ref())
                .copied()
                .or(export.levels.default)
                .or(self.level)
                .unwrap_or(Level::Info)
                >= level
            {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (export.callback)(&record)
                }));
            }
        }
    }
    pub fn exporter(
        &self,
        levels: Levels,
        export: impl Fn(&Record) + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError> {
        let inner = self.inner.upgrade().ok_or(KernelError::Closed)?;
        let activation = self.activation.upgrade().ok_or(KernelError::Inactive)?;
        let data = self.data.upgrade().ok_or(KernelError::Inactive)?;
        let ctx = Context::base(inner, data, activation);
        let disposer = ctx.reserve_effect("ctx.logger.exporter()")?;
        let export = Arc::new(Export {
            effect: Arc::downgrade(&disposer.effect),
            levels,
            callback: Arc::new(export),
        });
        lock(&self.logs).exporters.push(export.clone());
        let logs = self.logs.clone();
        disposer.effect.add(Undo::sync(move || {
            lock(&logs).exporters.retain(|e| !Arc::ptr_eq(e, &export))
        }));
        disposer.effect.finish_setup();
        Ok(disposer)
    }
    pub fn buffer(&self) -> Vec<Record> {
        lock(&self.logs).records.iter().cloned().collect()
    }
}
pub struct ConsoleExporter;
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleConfig {
    #[serde(default)]
    pub levels: HashMap<String, u8>,
    #[serde(default = "default_time")]
    pub show_time: String,
    #[serde(default)]
    pub show_diff: bool,
}
fn default_time() -> String {
    "yyyy-MM-dd hh:mm:ss ".into()
}
impl Plugin for ConsoleExporter {
    const NAME: &'static str = "logger-console";
    type Config = ConsoleConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<Self::Config>) -> Result<(), Error> {
        let config = config.get();
        let mut levels = Levels::default();
        for (name, level) in &config.levels {
            let level = match level {
                0 => Level::Error,
                1 => Level::Warn,
                2 => Level::Info,
                _ => Level::Debug,
            };
            if name == "default" {
                levels.default = Some(level);
            } else {
                levels.by_name.insert(name.clone(), level);
            }
        }
        let previous = Mutex::new(ctx.logger().now_ms());
        ctx.logger().exporter(levels, move |record| {
            use std::io::Write;
            let prefix = match record.level {
                Level::Error => "E",
                Level::Warn => "W",
                Level::Info => "I",
                Level::Debug => "D",
            };
            let time = if config.show_time.is_empty() {
                String::new()
            } else {
                time_template(
                    &config.show_time,
                    record.ts_ms,
                    &jiff::tz::TimeZone::system(),
                )
            };
            let mut previous = lock(&previous);
            let diff = if config.show_diff {
                format!(" +{}", time_diff(record.ts_ms - *previous))
            } else {
                String::new()
            };
            *previous = record.ts_ms;
            let indent = " ".repeat(4 + config.show_time.encode_utf16().count());
            let message = record
                .message
                .replace("\r\n", "\n")
                .split('\n')
                .map(|line| {
                    let mut width = 0;
                    let truncated = line
                        .chars()
                        .take_while(|c| {
                            width += c.len_utf16();
                            width <= 10240
                        })
                        .collect::<String>();
                    if width > 10240 {
                        format!("{truncated}...")
                    } else {
                        truncated
                    }
                })
                .collect::<Vec<_>>()
                .join(&format!("\n{indent}"));
            let _ = writeln!(
                std::io::stderr().lock(),
                "{time}[{prefix}] {} {}{diff}",
                record.name,
                message
            );
        })?;
        Ok(())
    }
}
fn time_diff(ms: i64) -> String {
    for (threshold, unit, label) in [
        (84_600_000, 86_400_000, "d"),
        (3_570_000, 3_600_000, "h"),
        (59_500, 60_000, "m"),
        (1_000, 1_000, "s"),
    ] {
        if ms.unsigned_abs() >= threshold {
            return format!(
                "{}{label}",
                ((ms as f64 / unit as f64) + 0.5).floor() as i64
            );
        }
    }
    format!("{ms}ms")
}
fn time_template(template: &str, ts: i64, zone: &jiff::tz::TimeZone) -> String {
    let Ok(instant) = jiff::Timestamp::from_millisecond(ts) else {
        return template.to_owned();
    };
    let time = instant.to_zoned_datetime(zone.clone());
    let mut output = template.to_owned();
    for (token, value) in [
        ("yyyy", format!("{:04}", time.year())),
        ("yy", format!("{:02}", time.year().rem_euclid(100))),
        ("MM", format!("{:02}", time.month())),
        ("dd", format!("{:02}", time.day())),
        ("hh", format!("{:02}", time.hour())),
        ("mm", format!("{:02}", time.minute())),
        ("ss", format!("{:02}", time.second())),
        ("SSS", format!("{:03}", time.millisecond())),
    ] {
        output = output.replacen(token, &value, 1);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn a7_fixed_timezone_not_utc() {
        assert_eq!(
            time_template(
                "yyyy-MM-dd hh:mm:ss.SSS",
                0,
                &jiff::tz::TimeZone::fixed(jiff::tz::offset(3))
            ),
            "1970-01-01 03:00:00.000"
        );
    }
    #[tokio::test(start_paused = true)]
    async fn console_time_formats() {
        assert_eq!(
            time_template("yyyy-MM-dd hh:mm:ss.SSS", 0, &jiff::tz::TimeZone::UTC),
            "1970-01-01 00:00:00.000"
        );
        assert_eq!(
            time_template(
                "yyyy-MM-dd hh:mm:ss.SSS",
                951782400001,
                &jiff::tz::TimeZone::UTC
            ),
            "2000-02-29 00:00:00.001"
        );
        assert_eq!(time_diff(1000), "1s");
        assert_eq!(time_diff(59500), "1m");
    }
}
