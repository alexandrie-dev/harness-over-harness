//! Boot-free literal composition; no expression evaluation or plugin application.
use hoh_error::BoxError;
use hoh_loader::{Composition, Layer, LayerSource, Profile};
use std::path::{Path, PathBuf};
pub fn collect_config_dump_layers(
    profile: &Profile,
    default_only: bool,
    patches: &[PathBuf],
    home: &Path,
    cwd: &Path,
) -> Result<Vec<Layer>, BoxError> {
    let mut layers: Vec<_> = profile
        .layers
        .iter()
        .filter(|l| {
            matches!(l.source, LayerSource::Bundle(_))
                || !default_only && matches!(l.source, LayerSource::Profile)
        })
        .cloned()
        .collect();
    if !default_only {
        let path = super::profile_boot::home_patch_path(home);
        if let Some(patches) = hoh_loader::load_named_optional_patches("dsh", &path)? {
            layers.push(Layer {
                source: LayerSource::Home,
                file: Some(path),
                patches,
            });
        }
        for patch in patches {
            let path = hoh_loader::resolve_config_path(
                &patch.to_string_lossy(),
                None,
                &cwd.to_string_lossy(),
            );
            layers.push(Layer {
                source: LayerSource::CommandLine(path.clone()),
                patches: hoh_loader::load_named_overlay_patches("dsh", &path)?,
                file: Some(path),
            });
        }
    }
    Ok(layers)
}
pub fn run_dump_config(
    profile: &Profile,
    default_only: bool,
    patches: &[PathBuf],
    home: &Path,
    cwd: &Path,
    warn: &mut dyn FnMut(String),
) -> Result<String, BoxError> {
    let layers = collect_config_dump_layers(profile, default_only, patches, home, cwd)?;
    let composition: Composition = hoh_loader::compose(&[], &layers, &mut |message| {
        warn(format!("dsh: {message}\n"))
    });
    Ok(hoh_loader::render_dump(&composition))
}
