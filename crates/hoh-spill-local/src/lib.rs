//! Local-filesystem implementation of the spill storage seam.

pub mod cleanup;
pub mod store;

pub use cleanup::{SweepOptions, SweepRoot, WarnFn, discover_default_roots, sweep_spill_roots};
pub use store::{
    DEFAULT_ROOT_PREFIX, SaveTextOptions, SavedText, encode_segment, is_errno, private_root,
    save_text_file, session_dir,
};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Plugin};
use hoh_spill::{SaveTextSpill, SpillRef, SpillStore, SpillStoreApi, spill_locator};
use hoh_util_workspace_path::node_path;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;
const RETRIEVAL_HINT: &str = "Use read with offset/limit, or grep this path to search within it.";

fn default_cleanup_period_days() -> u64 {
    30
}

fn deserialize_cleanup_period_days<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(default_cleanup_period_days());
    };
    let number = value
        .as_number()
        .ok_or_else(|| serde::de::Error::custom("cleanupPeriodDays must be an integer"))?;
    if let Some(value) = number.as_u64() {
        return Ok(value);
    }
    let value = number
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0 && value.fract() == 0.0)
        .ok_or_else(|| serde::de::Error::custom("cleanupPeriodDays must be an integer"))?;
    const U64_EXCLUSIVE_UPPER_BOUND: f64 = 18_446_744_073_709_551_616.0;
    if value >= U64_EXCLUSIVE_UPPER_BOUND {
        return Err(serde::de::Error::custom(
            "cleanupPeriodDays is out of range",
        ));
    }
    Ok(value as u64)
}

fn deserialize_root<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_root"
    )]
    #[schemars(with = "String")]
    pub root: Option<String>,
    #[serde(
        default = "default_cleanup_period_days",
        deserialize_with = "deserialize_cleanup_period_days"
    )]
    #[schemars(with = "Option<u64>")]
    pub cleanup_period_days: u64,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: None,
            cleanup_period_days: default_cleanup_period_days(),
            extra: IndexMap::new(),
        }
    }
}

pub struct LocalSpillStore {
    pub root: String,
    pub config: Config,
}

#[cfg(test)]
struct CleanupTestHook {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    roots: Vec<SweepRoot>,
}

#[cfg(test)]
static CLEANUP_TEST_HOOK: OnceLock<Mutex<Option<Arc<CleanupTestHook>>>> = OnceLock::new();

impl LocalSpillStore {
    pub fn new(config: Config) -> Result<Self, BoxError> {
        let root = match &config.root {
            Some(root) => {
                let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
                node_path::resolve(&cwd, &[root])
            }
            None => private_root()?,
        };
        Ok(Self { root, config })
    }
}

impl SpillStoreApi for LocalSpillStore {
    fn save_text(
        &self,
        input: SaveTextSpill,
    ) -> Pin<Box<dyn Future<Output = Result<SpillRef, BoxError>> + Send + '_>> {
        Box::pin(async move {
            let saved = save_text_file(SaveTextOptions {
                root: self.root.clone(),
                session_id: input.owner.session_id.to_string(),
                suggested_name: input.suggested_name,
                content: input.content,
            })
            .await?;
            Ok(SpillRef {
                locator: spill_locator(saved.path),
                bytes: saved.bytes,
                retrieval_hint: RETRIEVAL_HINT.into(),
            })
        })
    }
}

impl Plugin for LocalSpillStore {
    const NAME: &'static str = "LocalSpillStore";
    type Config = Config;
    type Deps = ();

    async fn apply(ctx: Context, _: Self::Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let store = Arc::new(LocalSpillStore::new((*config.get()).clone())?);
        ctx.provide::<SpillStore>(store.clone()).await?;
        if store.config.cleanup_period_days > 0 {
            let root = store.root.clone();
            let cleanup_days = store.config.cleanup_period_days;
            let logger = ctx.logger();
            ctx.spawn("spill-local cleanup sweep", move |_| async move {
                let warn: WarnFn = Arc::new(move |message| logger.warn(message));
                let cutoff_ms = epoch_ms().saturating_sub(
                    i64::try_from(cleanup_days)
                        .unwrap_or(i64::MAX)
                        .saturating_mul(MS_PER_DAY),
                );
                #[cfg(test)]
                let hook = CLEANUP_TEST_HOOK
                    .get_or_init(|| Mutex::new(None))
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                #[cfg(test)]
                if let Some(hook) = &hook {
                    hook.started.notify_one();
                    hook.release.notified().await;
                }
                #[cfg(test)]
                let roots = match hook {
                    Some(hook) => hook.roots.clone(),
                    None => cleanup::gather_sweep_roots(&root, warn.clone(), None).await,
                };
                #[cfg(not(test))]
                let roots = cleanup::gather_sweep_roots(&root, warn.clone(), None).await;
                sweep_spill_roots(SweepOptions {
                    roots,
                    cutoff_ms,
                    warn,
                })
                .await;
                Ok(())
            })?;
        }
        Ok(())
    }
}

fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use hoh_kernel::{Kernel, Level};
    use hoh_llm::ToolCallId;
    use hoh_session::SessionId;
    use hoh_spill::{SpillOwner, SpillSource};
    use std::time::{Duration, SystemTime};

    static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    fn install_hook(roots: Vec<SweepRoot>) -> Arc<CleanupTestHook> {
        let hook = Arc::new(CleanupTestHook {
            started: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
            roots,
        });
        *CLEANUP_TEST_HOOK
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(hook.clone());
        hook
    }

    fn clear_hook() {
        *CLEANUP_TEST_HOOK
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = None;
    }

    #[tokio::test]
    async fn activation_does_not_wait_for_cleanup_but_disposal_does() {
        let _guard = TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let directory = session_dir(root.path(), "old-session");
        std::fs::create_dir_all(&directory).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let old = directory.join("old.txt");
        std::fs::write(&old, "old").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::now() - Duration::from_secs(40 * 24 * 60 * 60)),
            )
            .unwrap();
        let hook = install_hook(vec![SweepRoot {
            path: root.path().to_string_lossy().into_owned(),
            prune_when_empty: false,
        }]);
        let kernel = Kernel::start();
        let context = kernel.root();
        let fiber = context
            .plugin::<LocalSpillStore>(serde_json::json!({
                "root": root.path(),
                "cleanupPeriodDays": 30
            }))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), fiber.settled())
            .await
            .expect("activation must not await cleanup")
            .unwrap();
        hook.started.notified().await;
        assert!(old.exists());
        let saved = context
            .get::<SpillStore>()
            .expect("service is available while cleanup is parked")
            .save_text(SaveTextSpill {
                owner: SpillOwner {
                    session_id: SessionId::from("live-session"),
                },
                source: SpillSource::Tool {
                    tool_name: "test".into(),
                    call_id: ToolCallId::from("call-1"),
                    label: "result".into(),
                },
                suggested_name: "live.txt".into(),
                content: "live".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(saved.locator.as_str()).unwrap(),
            "live"
        );
        let disposing = tokio::spawn(fiber.dispose());
        tokio::task::yield_now().await;
        assert!(!disposing.is_finished());
        hook.release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), disposing)
            .await
            .expect("disposal completes after cleanup is released")
            .unwrap();
        assert!(
            !old.exists(),
            "cleanup warnings: {:?}",
            context.logger().buffer()
        );
        clear_hook();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_warning_is_forwarded_to_the_context_logger() {
        let _guard = TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("not-a-directory");
        std::fs::write(&file, "x").unwrap();
        let hook = install_hook(vec![SweepRoot {
            path: file.to_string_lossy().into_owned(),
            prune_when_empty: false,
        }]);
        let kernel = Kernel::start();
        let context = kernel.root();
        let fiber = context
            .plugin::<LocalSpillStore>(serde_json::json!({
                "root": root.path(),
                "cleanupPeriodDays": 30
            }))
            .unwrap();
        fiber.settled().await.unwrap();
        hook.started.notified().await;
        hook.release.notify_one();
        fiber.dispose().await;
        assert!(context.logger().buffer().iter().any(|record| {
            record.level == Level::Warn && record.message.contains("skipped unsafe root")
        }));
        clear_hook();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn explicit_null_cleanup_period_reaches_the_schemastery_default() {
        let _guard = TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = tempfile::tempdir().unwrap();
        let hook = install_hook(vec![]);
        let kernel = Kernel::start();
        let fiber = kernel
            .root()
            .plugin::<LocalSpillStore>(serde_json::json!({
                "root": root.path(),
                "cleanupPeriodDays": null
            }))
            .unwrap();
        fiber.settled().await.unwrap();
        hook.started.notified().await;
        hook.release.notify_one();
        fiber.dispose().await;
        clear_hook();
        kernel.shutdown().await;
    }
}
