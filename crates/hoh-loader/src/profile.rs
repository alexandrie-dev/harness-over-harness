use crate::{EvalEnv, Layer, LayerSource, LoadError, Patch, compose, parse_patches};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Bundle {
    pub name: String,
    pub dir: Option<PathBuf>,
    pub platforms: Option<Vec<String>>,
    pub files: Vec<(String, String)>,
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
            serde_json::from_str(&read(&manifest)?).map_err(|e| LoadError::Shape {
                file: manifest.clone(),
                line: e.line(),
                column: e.column(),
                message: e.to_string(),
            })?;
        if !value.is_object() {
            return Err(shape(&manifest, "profile manifest must hold a JSON object"));
        }
        for path in ["/dsh", "/dsh/profile"] {
            if let Some(value) = value.pointer(path)
                && !value.is_object()
            {
                return Err(shape(&manifest, format!("{path} must be an object")));
            }
        }
        let names = match value.pointer("/dsh/profile/bundles") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_owned).ok_or_else(|| {
                        shape(&manifest, "dsh.profile.bundles must be a list of strings")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(shape(
                    &manifest,
                    "dsh.profile.bundles must be a list of strings",
                ));
            }
        };
        let mut layers = Vec::new();
        let mut skipped = Vec::new();
        for name in names {
            let loaded = (|| {
                let bundle = bundles.bundle(&name)?;
                platform(&bundle.platforms, env, &manifest)?;
                let mut loaded = Vec::new();
                for (file, text) in bundle.files {
                    let file = match &bundle.dir {
                        Some(bundle_dir) => {
                            let bundle_dir = if bundle_dir.is_absolute() {
                                bundle_dir.clone()
                            } else {
                                env.cwd.join(bundle_dir)
                            };
                            bundle_dir.join(file)
                        }
                        None => dir.join(".hoh-bundles").join(&name).join(file),
                    };
                    let patches = parse_patches(&text, &file)?;
                    loaded.push(Layer {
                        source: LayerSource::Bundle(name.clone()),
                        file: Some(file),
                        patches,
                    });
                }
                Ok::<_, LoadError>(loaded)
            })();
            match loaded {
                Ok(loaded) => layers.extend(loaded),
                Err(error) => skipped.push(SkippedBundle {
                    name,
                    reason: error.to_string(),
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
            if contains(&entries, "session-telemetry-otel") {
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
fn contains(rows: &[crate::Entry], id: &str) -> bool {
    rows.iter().any(|r| {
        r.id == id
            || (r.group
                && serde_json::from_value::<Vec<crate::Entry>>(r.config.clone())
                    .is_ok_and(|rows| contains(&rows, id)))
    })
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
