use crate::{EvalEnv, Layer, LayerSource, LoadError, Patch, compose, parse_patches};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Bundle {
    pub name: String,
    pub dir: Option<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_platforms")]
    pub platforms: Option<Vec<String>>,
    pub files: Vec<(String, String)>,
}
fn deserialize_platforms<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Vec<String>>, D::Error> {
    use serde::Deserialize;
    let value = Value::deserialize(d)?;
    let error = || {
        serde::de::Error::custom(
            "dsh.bundle.platforms must be a nonempty list of Node platform names or wsl",
        )
    };
    let rows = value.as_array().ok_or_else(error)?;
    let strings = rows
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or_else(error))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(strings))
}
pub trait BundleSource: Send + Sync {
    fn bundle(&self, name: &str) -> Result<Bundle, LoadError>;
}
#[derive(Debug)]
pub struct SkippedBundle {
    pub name: String,
    pub reason: String,
}
#[derive(Debug)]
pub struct Profile {
    pub name: String,
    pub dir: PathBuf,
    pub layers: Vec<Layer>,
    pub skipped: Vec<SkippedBundle>,
}
fn shape(file: &Path, message: impl Into<String>) -> LoadError {
    LoadError::Shape {
        file: file.into(),
        line: 1,
        column: 1,
        message: message.into(),
    }
}
fn read(file: &Path) -> Result<String, LoadError> {
    std::fs::read_to_string(file).map_err(|source| LoadError::Io {
        file: file.into(),
        source,
    })
}
pub fn load_optional_patches(file: &Path) -> Result<Option<Vec<Patch>>, LoadError> {
    match std::fs::read_to_string(file) {
        Ok(text) => parse_patches(&text, file).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LoadError::Io {
            file: file.into(),
            source,
        }),
    }
}

fn node_read_error(source: &std::io::Error, file: &Path) -> String {
    if source.kind() == std::io::ErrorKind::IsADirectory {
        hoh_error::node_fs_error_message(source, "read", None, None)
    } else {
        let file = file.to_string_lossy();
        hoh_error::node_fs_error_message(source, "open", Some(file.as_ref()), None)
    }
}

fn parse_named_patches(
    bin_name: &str,
    label: &str,
    text: &str,
    file: &Path,
) -> Result<Vec<Patch>, LoadError> {
    let parse_error = |source: LoadError| {
        LoadError::Runtime(format!(
            "{bin_name}: failed to parse {label} {}: {source}",
            file.display()
        ))
    };
    let value = crate::yaml::parse_value(text, file).map_err(&parse_error)?;
    let Some(entries) = value.as_array() else {
        return Err(LoadError::Runtime(format!(
            "{bin_name}: {label} {} must be a top-level YAML array of loader patch entries",
            file.display()
        )));
    };
    if let Some(index) = entries.iter().position(|entry| !entry.is_object()) {
        return Err(LoadError::Runtime(format!(
            "{bin_name}: {label} entry {} in {} must be a mapping (a loader patch entry)",
            index + 1,
            file.display()
        )));
    }
    parse_patches(text, file).map_err(parse_error)
}

/// Source-compatible optional profile/home patch diagnostics for launcher surfaces.
pub fn load_named_optional_patches(
    bin_name: &str,
    file: &Path,
) -> Result<Option<Vec<Patch>>, LoadError> {
    match std::fs::read_to_string(file) {
        Ok(text) => parse_named_patches(bin_name, "patches", &text, file).map(Some),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LoadError::Runtime(format!(
            "{bin_name}: failed to read patches {}: Error: {}",
            file.display(),
            node_read_error(&source, file)
        ))),
    }
}

/// Source-compatible required command-line overlay diagnostics for launcher surfaces.
pub fn load_named_overlay_patches(bin_name: &str, file: &Path) -> Result<Vec<Patch>, LoadError> {
    let text = std::fs::read_to_string(file).map_err(|source| {
        LoadError::Runtime(format!(
            "{bin_name}: failed to read overlay {}: Error: {}",
            file.display(),
            node_read_error(&source, file)
        ))
    })?;
    parse_named_patches(bin_name, "overlay", &text, file)
}
impl Profile {
    pub fn load(
        dir: &Path,
        bundles: &dyn BundleSource,
        home: &Path,
        overlays: &[PathBuf],
        env: &EvalEnv,
    ) -> Result<Profile, LoadError> {
        Self::load_with_user_layer(dir, bundles, home, overlays, env, true)
    }
    /// Recovery dumps deliberately do not read profile, home or argv patches.
    pub fn load_with_user_layer(
        dir: &Path,
        bundles: &dyn BundleSource,
        home: &Path,
        overlays: &[PathBuf],
        env: &EvalEnv,
        user_layer: bool,
    ) -> Result<Profile, LoadError> {
        let dir = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            env.cwd.join(dir)
        };
        let manifest = dir.join("package.json");
        let mut value: Value =
            hoh_util_values::js_json_parse(&read(&manifest)?).map_err(|e| LoadError::Shape {
                file: manifest.clone(),
                line: 1,
                column: 1,
                message: e.to_string(),
            })?;
        if !value.is_object() {
            return Err(shape(&manifest, "profile manifest must hold a JSON object"));
        }
        crate::profile_files::drop_retired(&dir, &mut value)?;
        let names = match value.pointer("/dsh/profile/bundles") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.clone(),
            Some(_) => {
                return Err(shape(&manifest, "dsh.profile.bundles must be a list"));
            }
        };
        let mut layers = Vec::new();
        let mut skipped = Vec::new();
        for raw_name in names {
            let loaded = (|| {
                let name = raw_name
                    .as_str()
                    .ok_or_else(|| invalid_bundle_name(&raw_name))?;
                let bundle = bundles.bundle(name).map_err(|e| e.to_string())?;
                platform(&bundle.platforms, env, &manifest).map_err(|e| e.to_string())?;
                let mut patches = Vec::new();
                let mut files = Vec::new();
                for (file, text) in bundle.files {
                    let file = match &bundle.dir {
                        Some(bundle_dir) => {
                            let bundle_dir = if bundle_dir.is_absolute() {
                                bundle_dir.clone()
                            } else {
                                env.cwd.join(bundle_dir)
                            };
                            node_join(&bundle_dir, &file)
                        }
                        None => node_join(&node_join(&dir.join(".hoh-bundles"), name), &file),
                    };
                    patches.extend(parse_patches(&text, &file).map_err(|e| e.to_string())?);
                    files.push(file);
                }
                Ok::<_, String>(Layer {
                    source: LayerSource::Bundle(name.into()),
                    file: if files.len() == 1 { files.pop() } else { None },
                    patches,
                })
            })();
            match loaded {
                Ok(loaded) => layers.push(loaded),
                Err(reason) => skipped.push(SkippedBundle {
                    name: hoh_util_values::js_string(&raw_name),
                    reason,
                }),
            }
        }
        if user_layer {
            let file = dir.join("cordis.patch.yml");
            if let Some(patches) = load_optional_patches(&file)? {
                layers.push(Layer {
                    source: LayerSource::Profile,
                    file: Some(file),
                    patches,
                });
            }
        }
        let mut profile = Profile {
            name: dir.file_name().unwrap_or_default().to_string_lossy().into(),
            dir,
            layers,
            skipped,
        };
        if user_layer {
            profile.layers = read_profile_patches(&profile, home, overlays, env)?;
        }
        Ok(profile)
    }
}
/// Bundle/profile layers followed by current home, argv and launch-time telemetry policy.
pub fn read_profile_patches(
    profile: &Profile,
    home: &Path,
    overlays: &[PathBuf],
    env: &EvalEnv,
) -> Result<Vec<Layer>, LoadError> {
    read_profile_patches_impl(None, profile, home, overlays, env)
}

/// Launcher variant with source-compatible diagnostics and overlay-first reads.
pub fn read_named_profile_patches(
    bin_name: &str,
    profile: &Profile,
    home: &Path,
    overlays: &[PathBuf],
    env: &EvalEnv,
) -> Result<Vec<Layer>, LoadError> {
    read_profile_patches_impl(Some(bin_name), profile, home, overlays, env)
}

fn read_profile_patches_impl(
    bin_name: Option<&str>,
    profile: &Profile,
    home: &Path,
    overlays: &[PathBuf],
    env: &EvalEnv,
) -> Result<Vec<Layer>, LoadError> {
    let mut layers: Vec<_> = profile
        .layers
        .iter()
        .filter(|l| matches!(l.source, LayerSource::Bundle(_) | LayerSource::Profile))
        .cloned()
        .collect();
    if let Some(bin_name) = bin_name
        && !layers
            .iter()
            .any(|layer| matches!(layer.source, LayerSource::Profile))
    {
        let file = profile.dir.join("cordis.patch.yml");
        if let Some(patches) = load_named_optional_patches(bin_name, &file)? {
            layers.push(Layer {
                source: LayerSource::Profile,
                file: Some(file),
                patches,
            });
        }
    }
    // The source freezes every explicit overlay in composeProfile before boot reads
    // the optional home patch, while the final application order remains home then argv.
    let overlay_layers = overlays
        .iter()
        .map(|overlay| {
            let file = crate::resolve_config_path(
                &overlay.to_string_lossy(),
                None,
                &env.cwd.to_string_lossy(),
            );
            let patches = match bin_name {
                Some(bin_name) => load_named_overlay_patches(bin_name, &file)?,
                None => parse_patches(&read(&file)?, &file)?,
            };
            Ok(Layer {
                source: LayerSource::CommandLine(file.clone()),
                file: Some(file),
                patches,
            })
        })
        .collect::<Result<Vec<_>, LoadError>>()?;
    let home = if home.is_absolute() {
        home.to_path_buf()
    } else {
        env.cwd.join(home)
    };
    let file = home.join("cordis.patch.yml");
    let home_patches = match bin_name {
        Some(bin_name) => load_named_optional_patches(bin_name, &file)?,
        None => load_optional_patches(&file)?,
    };
    if let Some(patches) = home_patches {
        layers.push(Layer {
            source: LayerSource::Home,
            file: Some(file),
            patches,
        });
    }
    layers.extend(overlay_layers);
    append_telemetry_patch(&mut layers, env);
    Ok(layers)
}
pub fn resolve_telemetry_patch(disabled_env: Option<&str>, has_row: bool) -> Option<Patch> {
    (disabled_env.is_some_and(|s| !s.is_empty()) && has_row).then(|| Patch {
        id: Some("session-telemetry-otel".into()),
        name: None,
        set: [("disabled".into(), Value::Bool(true))].into(),
        insert: None,
    })
}
pub(crate) fn append_telemetry_patch(layers: &mut Vec<Layer>, env: &EvalEnv) {
    if let Some(patch) = resolve_telemetry_patch(
        (env.var)("DSH_TELEMETRY_DISABLED").as_deref(),
        compose(&[], layers, &mut |_| {})
            .entries
            .iter()
            .any(|row| row.id == "session-telemetry-otel"),
    ) {
        layers.push(Layer {
            source: LayerSource::Launcher("DSH_TELEMETRY_DISABLED"),
            file: None,
            patches: vec![patch],
        });
    }
}
fn invalid_bundle_name(value: &Value) -> String {
    let received = match value {
        Value::Null => "null".into(),
        Value::Bool(_) => format!("type boolean ({})", hoh_util_values::js_string(value)),
        Value::Number(number) => {
            let text = if number
                .as_f64()
                .is_some_and(|n| n == 0.0 && n.is_sign_negative())
            {
                "-0".into()
            } else {
                hoh_util_values::js_string(value)
            };
            format!("type number ({text})")
        }
        Value::Array(_) => "an instance of Array".into(),
        Value::Object(_) => "an instance of Object".into(),
        Value::String(_) => "type string".into(), // The caller only rejects non-string manifest elements.
    };
    format!(
        "TypeError [ERR_INVALID_ARG_TYPE]: The \"request\" argument must be of type string. Received {received}"
    )
}
fn node_join(base: &Path, part: &str) -> PathBuf {
    PathBuf::from(crate::expr::platform_join(
        &[base.to_string_lossy().into_owned(), part.into()],
        if cfg!(windows) { "win32" } else { "linux" },
    ))
}
fn platform(platforms: &Option<Vec<String>>, env: &EvalEnv, file: &Path) -> Result<(), LoadError> {
    let Some(platforms) = platforms else {
        return Ok(());
    };
    const VALID: &[&str] = &[
        "aix", "android", "darwin", "freebsd", "haiku", "linux", "openbsd", "sunos", "win32",
        "cygwin", "netbsd", "wsl",
    ];
    if platforms.is_empty() || platforms.iter().any(|p| !VALID.contains(&p.as_str())) {
        return Err(shape(
            file,
            "dsh.bundle.platforms must be a nonempty list of Node platform names or wsl",
        ));
    }
    let wsl = env.platform == "linux"
        && ["WSL_DISTRO_NAME", "WSL_INTEROP"]
            .iter()
            .any(|name| (env.var)(name).is_some_and(|v| !v.is_empty()));
    if platforms
        .iter()
        .any(|p| p == env.platform || (p == "wsl" && wsl))
    {
        Ok(())
    } else {
        Err(shape(
            file,
            format!(
                "Bundle supports Host platforms {}; the DSH Host is running {}",
                platforms.join(", "),
                env.platform
            ),
        ))
    }
}
