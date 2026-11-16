use indexmap::IndexMap;

pub type Environment = IndexMap<String, String>;

/// Convert injectable OS environment entries as Node decodes process.env.
pub fn environment_from_os(
    entries: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Environment {
    entries
        .into_iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

pub(crate) fn ambient_environment() -> Environment {
    environment_from_os(std::env::vars_os())
}

#[derive(Debug, Clone, Default)]
pub struct HostEnvironmentFacts {
    pub platform: Option<String>,
    pub env: Option<Environment>,
    pub os_release: Option<String>,
}

pub fn platform() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "win32"
    }
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::consts::OS
    }
}

pub fn os_release() -> String {
    #[cfg(unix)]
    {
        rustix::system::uname()
            .release()
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(not(unix))]
    {
        String::new()
    }
}

pub fn is_wsl(facts: &HostEnvironmentFacts) -> bool {
    if facts.platform.as_deref().unwrap_or_else(|| platform()) != "linux" {
        return false;
    }
    let ambient;
    let env = match &facts.env {
        Some(env) => env,
        None => {
            ambient = ambient_environment();
            &ambient
        }
    };
    ["WSL_DISTRO_NAME", "WSL_INTEROP"]
        .iter()
        .any(|key| env.get(*key).is_some_and(|s| !s.is_empty()))
        || facts
            .os_release
            .clone()
            .unwrap_or_else(os_release)
            .to_lowercase()
            .contains("microsoft")
}
