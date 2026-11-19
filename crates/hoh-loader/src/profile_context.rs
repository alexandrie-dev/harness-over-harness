//! Active profile facts supplied by a launcher before its plugin tree mounts.
use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileContext {
    pub name: String,
    pub dir: PathBuf,
    pub home: PathBuf,
}
pub struct ProfileContextKey;
impl hoh_kernel::ServiceKey for ProfileContextKey {
    type Value = ProfileContext;
    const NAME: &'static str = "profileContext";
    fn view(value: &ProfileContext) -> Option<serde_json::Value> {
        Some(
            serde_json::json!({"name":value.name,"dir":value.dir.to_string_lossy(),"home":value.home.to_string_lossy()}),
        )
    }
}
