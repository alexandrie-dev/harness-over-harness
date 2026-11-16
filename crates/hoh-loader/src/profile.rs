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
fn optional(file: &Path) -> Result<Option<Vec<Patch>>, LoadError> {
    match std::fs::read_to_string(file) {
        Ok(text) => parse_patches(&text, file).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LoadError::Io {
            file: file.into(),
            source,
        }),
    }
}
impl Profile {
    pub fn load(
        dir: &Path,
        bundles: &dyn BundleSource,
        home: &Path,
        overlays: &[PathBuf],
        env: &EvalEnv,
    ) -> Result<Profile, LoadError> {
        let dir = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            env.cwd.join(dir)
        };
        let manifest = dir.join("package.json");
        let value: Value =
            hoh_util_values::js_json_parse(&read(&manifest)?).map_err(|e| LoadError::Shape {
                file: manifest.clone(),
                line: 1,
                column: 1,
                message: e.to_string(),
            })?;
        if !value.is_object() {
            return Err(shape(&manifest, "profile manifest must hold a JSON object"));
        }
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
        let home = if home.is_absolute() {
            home.to_path_buf()
        } else {
            env.cwd.join(home)
        };
        for (source, file) in [
            (LayerSource::Profile, dir.join("cordis.patch.yml")),
            (LayerSource::Home, home.join("cordis.patch.yml")),
        ] {
            if let Some(patches) = optional(&file)? {
                layers.push(Layer {
                    source,
                    file: Some(file),
                    patches,
                });
            }
        }
        for overlay in overlays {
            let file = if overlay.is_absolute() {
                overlay.clone()
            } else {
                env.cwd.join(overlay)
            };
            let patches = parse_patches(&read(&file)?, &file)?;
            layers.push(Layer {
                source: LayerSource::CommandLine(file.clone()),
                file: Some(file),
                patches,
            });
        }
        if (env.var)("DSH_TELEMETRY_DISABLED").is_some_and(|v| !v.is_empty()) {
            let entries = compose(&[], &layers, &mut |_| {}).entries;
            if entries.iter().any(|row| row.id == "session-telemetry-otel") {
                layers.push(Layer {
                    source: LayerSource::Launcher("DSH_TELEMETRY_DISABLED"),
                    file: None,
                    patches: vec![Patch {
                        id: Some("session-telemetry-otel".into()),
                        name: None,
                        set: [("disabled".into(), Value::Bool(true))].into(),
                        insert: None,
                    }],
                });
            }
        }
        Ok(Profile {
            name: dir.file_name().unwrap_or_default().to_string_lossy().into(),
            dir,
            layers,
            skipped,
        })
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
