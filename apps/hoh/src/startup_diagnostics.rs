//! Private full startup reports and awaited terminal writes.
use hoh_error::BoxError;
use hoh_loader::StartupError;
use std::{future::Future, path::PathBuf};
use tokio::io::AsyncWriteExt;
pub const WARNING: &str = "Внимание: отчёт может содержать настройки и учётные данные из ошибок плагинов. Проверьте его перед отправкой.\n\n";
pub struct StartupDiagnosticContext {
    pub home: PathBuf,
    pub version: String,
    pub profile: String,
}
pub struct ReportFacts {
    pub timestamp: String,
    pub uuid: String,
    pub platform: String,
    pub arch: String,
}
fn uuid() -> Result<String, BoxError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let mut out = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}
pub async fn report_startup_failure<F, Fut>(
    error: &StartupError,
    context: &StartupDiagnosticContext,
    write: F,
) -> Result<(), BoxError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<(), BoxError>>,
{
    let timestamp =
        hoh_util_time::format_four_digit_year_instant(jiff::Timestamp::now().as_millisecond())
            .ok_or("startup timestamp is outside the four-digit-year range")?;
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    report_startup_failure_with_facts(
        error,
        context,
        &ReportFacts {
            timestamp,
            uuid: uuid()?,
            platform: platform.into(),
            arch: arch.into(),
        },
        write,
    )
    .await
}
/// Native host facts are injectable so tests never depend on the real clock.
pub async fn report_startup_failure_with_facts<F, Fut>(
    error: &StartupError,
    context: &StartupDiagnosticContext,
    facts: &ReportFacts,
    mut write: F,
) -> Result<(), BoxError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<(), BoxError>>,
{
    let stamp = facts.timestamp.replace(':', "-");
    if stamp.is_empty()
        || facts.uuid.is_empty()
        || !stamp
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        || !facts
            .uuid
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return Err("invalid startup diagnostic filename facts".into());
    }
    // Rust errors expose Debug/source instead of JavaScript's property descriptors and nodeVersion.
    let report = format!(
        "{WARNING}{{\n  timestamp: {:?},\n  dshVersion: {:?},\n  runtime: {:?},\n  platform: {:?},\n  arch: {:?},\n  profile: {:?},\n  configurationPath: {:?},\n  error: {:#?},\n  causes: {:?},\n  entries: {:#?},\n  messages: {:#?},\n  composition: {:#?},\n}}\n",
        facts.timestamp,
        context.version,
        concat!("hoh/", env!("CARGO_PKG_VERSION")),
        facts.platform,
        facts.arch,
        context.profile,
        error.configuration_path,
        error,
        hoh_loader::startup_diagnostics::format_activation_error(error),
        error.entries,
        error.messages,
        error.composition
    );
    write(format!("{}\n", error.message)).await?;
    let log_dir = context.home.join("logs");
    let log_path = log_dir.join(format!("startup-{stamp}-{}.log", facts.uuid));
    let save = async {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(&log_dir)
            .await
            .map_err(|e| super::profile_initialization::fs_error("mkdir", &log_dir, None, e))?;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&log_path)
            .await
            .map_err(|e| super::profile_initialization::fs_error("open", &log_path, None, e))?;
        file.write_all(report.as_bytes())
            .await
            .map_err(|e| super::profile_initialization::fs_error("write", &log_path, None, e))?;
        file.flush()
            .await
            .map_err(|e| super::profile_initialization::fs_error("write", &log_path, None, e))?;
        Ok::<(), BoxError>(())
    }
    .await;
    if let Err(error) = save {
        write(format!("\nHoH: не удалось сохранить отчёт о запуске: {error}\nДиагностика:\n{report}")).await?;
        return Ok(());
    }
    write(format!("\nДиагностика: {}\n", log_path.display())).await?;
    Ok(())
}
