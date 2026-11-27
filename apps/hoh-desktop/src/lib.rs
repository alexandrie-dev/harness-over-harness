pub mod approval;
#[cfg(all(feature = "desktop", any(target_os = "linux", target_os = "macos")))]
pub mod auto_update;
pub mod bridge;
pub mod chat_secrets;
pub mod engine;
pub mod errors;
#[cfg(feature = "desktop")]
pub mod external_ui;
pub mod folders;
pub mod git_workspace;
pub mod image_asset;
#[cfg(feature = "desktop")]
pub mod native;
pub mod opener;
pub mod project_files;
#[cfg(feature = "desktop")]
mod platform;
#[cfg(all(feature = "probe", target_os = "linux"))]
pub mod probe;
pub mod questions;
pub mod workspace;
pub mod zoom;

pub fn probe_compiled() -> bool {
    cfg!(all(feature = "probe", target_os = "linux"))
}

#[cfg(feature = "desktop")]
pub mod html_preview;

#[cfg(all(feature = "desktop", target_os = "linux"))]
pub mod clipboard_image;
