pub mod approval;
pub mod bridge;
pub mod engine;
pub mod errors;
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
