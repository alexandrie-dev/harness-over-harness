pub mod approval;
pub mod bridge;
pub mod chat_secrets;
pub mod engine;
pub mod errors;
#[cfg(feature = "desktop")]
pub mod external_ui;
pub mod folders;
pub mod image_asset;
#[cfg(feature = "desktop")]
pub mod native;
pub mod opener;
#[cfg(feature = "probe")]
pub mod probe;
pub mod questions;
pub mod workspace;
pub mod zoom;

pub fn probe_compiled() -> bool {
    cfg!(feature = "probe")
}

#[cfg(feature = "desktop")]
pub mod html_preview;

#[cfg(all(feature = "desktop", target_os = "linux"))]
pub mod clipboard_image;
