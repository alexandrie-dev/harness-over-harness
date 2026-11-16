use crate::{
    file_applications_linux::linux_file_applications,
    file_applications_windows::windows_file_applications, path_opener::*, runner::*, types::*,
};
use indexmap::IndexMap;
use serde_json::Value;

struct MacApplication {
    base: NativeFileApplication,
    bundle: Option<String>,
    version: Option<String>,
}
fn mac_field(value: Option<&Value>) -> Result<Option<String>, NativeCommandError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok((!s.is_empty()).then(|| s.clone())),
        _ => Err(NativeCommandError::message(
            "Invalid native application entry",
        )),
    }
}
fn compare_versions(left: Option<&str>, right: Option<&str>) -> std::cmp::Ordering {
    match (left, right) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => {
            let a: Vec<_> = a.split('.').collect();
            let b: Vec<_> = b.split('.').collect();
            for i in 0..a.len().max(b.len()) {
                let number = |s: Option<&&str>| s.map_or(0.0, |s| version_number(s));
                let order = number(a.get(i))
                    .partial_cmp(&number(b.get(i)))
                    .unwrap_or(std::cmp::Ordering::Equal);
                if !order.is_eq() {
                    return order;
                }
            }
            std::cmp::Ordering::Equal
        }
    }
}
fn version_number(text: &str) -> f64 {
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (prefixes, radix) in [(["0x", "0X"], 16), (["0b", "0B"], 2), (["0o", "0O"], 8)] {
        if let Some(digits) = prefixes.iter().find_map(|prefix| text.strip_prefix(prefix)) {
            if digits.is_empty() {
                return 0.0;
            }
            return digits
                .chars()
                .try_fold(0.0, |value, c| {
                    c.to_digit(radix)
                        .map(|digit| value * f64::from(radix) + f64::from(digit))
                })
                .unwrap_or(0.0);
        }
    }
    if text
        .chars()
        .any(|c| !c.is_ascii_digit() && !"eE+-.".contains(c))
    {
        return 0.0;
    }
    text.parse::<f64>()
        .ok()
        .filter(|n| !n.is_nan())
        .unwrap_or(0.0)
}
async fn desktop_target(
    path: &str,
    signal: &dyn CommandSignal,
    internals: &PathOpenerInternals<'_>,
) -> Result<(String, String), NativeCommandError> {
    signal.throw_if_aborted()?;
    if internals.platform() == "linux"
        && native_file_manager(internals) == Some(NativeFileManager::Explorer)
    {
        return Ok((
            "win32".into(),
            translated_path(path, signal, internals.runner()).await?,
        ));
    }
    Ok((internals.platform().into(), path.into()))
}
async fn query_file_applications(
    path: &str,
    signal: &dyn CommandSignal,
    internals: &PathOpenerInternals<'_>,
    display: bool,
) -> Result<Vec<NativeFileApplication>, NativeCommandError> {
    signal.throw_if_aborted()?;
    let (platform, target) = desktop_target(path, signal, internals).await?;
    let run = internals.runner();
    if platform == "linux" {
        return linux_file_applications(path, signal, run, &internals.environment()).await;
    }
    if platform == "darwin" {
        let output = run
            .run(
                "osascript",
                &[
                    "-l".into(),
                    "JavaScript".into(),
                    "-e".into(),
                    include_str!("mac_applications.js")
                        .trim_end_matches('\n')
                        .into(),
                    target,
                    if display { "icons" } else { "handlers" }.into(),
                ],
                signal,
                Window::Hidden,
            )
            .await?;
        let value: Value = serde_json::from_str(&output.stdout)
            .map_err(|e| NativeCommandError::message(e.to_string()))?;
        let base = parse_native_file_applications(&value)?;
        // The parser above established an array, one entry for each base application.
        let entries = value.as_array().expect("validated array");
        let mut applications = Vec::new();
        for (entry, base) in entries.iter().zip(base) {
            applications.push(MacApplication {
                base,
                bundle: mac_field(entry.get("bundle"))?,
                version: mac_field(entry.get("version"))?,
            });
        }
        if !display {
            return Ok(applications.into_iter().map(|a| a.base).collect());
        }
        let mut order: Vec<MacApplication> = Vec::new();
        let mut groups: IndexMap<String, usize> = IndexMap::new();
        for app in applications {
            if let Some(bundle) = &app.bundle {
                let key = format!("{bundle}\0{}", app.base.name);
                if let Some(index) = groups.get(&key) {
                    let held = &order[*index];
                    if !held.base.default
                        && (app.base.default
                            || compare_versions(app.version.as_deref(), held.version.as_deref())
                                .is_gt())
                    {
                        order[*index] = app;
                    }
                    continue;
                }
                groups.insert(key, order.len());
            }
            order.push(app);
        }
        return Ok(order.into_iter().map(|a| a.base).collect());
    }
    if platform != "win32" {
        return Ok(Vec::new());
    }
    let output = windows_file_applications(&target, None, signal, run).await?;
    let value =
        serde_json::from_str(&output).map_err(|e| NativeCommandError::message(e.to_string()))?;
    parse_native_file_applications(&value)
}
pub async fn native_file_applications(
    path: &str,
    signal: &dyn CommandSignal,
    internals: &PathOpenerInternals<'_>,
) -> Result<Vec<NativeFileApplication>, NativeCommandError> {
    query_file_applications(path, signal, internals, true).await
}
pub async fn open_native_file_application(
    path: &str,
    application: &str,
    signal: &dyn CommandSignal,
    internals: &PathOpenerInternals<'_>,
) -> Result<(), NativeCommandError> {
    let (platform, target) = desktop_target(path, signal, internals).await?;
    let run = internals.runner();
    if platform == "win32" {
        windows_file_applications(&target, Some(application), signal, run).await?;
        return Ok(());
    }
    let apps = query_file_applications(path, signal, internals, false).await?;
    if !apps.iter().any(|a| a.id == application) {
        return Err(NativeCommandError::message(
            "Application is not registered for this file",
        ));
    }
    if platform == "linux" {
        run.run(
            "gio",
            &["launch".into(), application.into(), path.into()],
            signal,
            Window::Hidden,
        )
        .await?;
    } else {
        run.run(
            "open",
            &["-a".into(), application.into(), path.into()],
            signal,
            Window::Hidden,
        )
        .await?;
    }
    Ok(())
}
