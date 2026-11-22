pub mod approval;
pub mod bridge;
pub mod engine;
pub mod errors;
#[cfg(feature = "desktop")]
pub mod native;
pub mod opener;
#[cfg(feature = "probe")]
pub mod probe;
pub mod questions;
pub mod workspace;

pub fn probe_compiled() -> bool {
    cfg!(feature = "probe")
}
