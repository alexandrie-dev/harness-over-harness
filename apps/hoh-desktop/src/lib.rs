pub mod approval;
pub mod bridge;
pub mod engine;
#[cfg(feature = "desktop")]
pub mod native;
#[cfg(feature = "probe")]
pub mod probe;

pub fn probe_compiled() -> bool {
    cfg!(feature = "probe")
}
