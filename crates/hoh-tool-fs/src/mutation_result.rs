//! Shared observation and result fields for successful file mutations.

use std::sync::Arc;

use hoh_fs::{FsActor, FsObservation, FsObserved, FsTarget, FsVersion};
use hoh_tools::ToolExecution;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::Env;

/// Record the new file version and expose the mutation result with its canonical path.
///
/// `target` is the resolved file that was mutated; `outcome` is the provider's successful result
/// and `version` the version it produced, which stays in the observation event. Returns the
/// provider's result fields without the version, plus the canonical absolute path first.
pub fn mutation_result<T: Serialize>(
    env: &Env,
    target: &FsTarget,
    outcome: &T,
    version: &FsVersion,
    exec: &Arc<ToolExecution>,
) -> Value {
    env.ctx.emit(&FsObserved {
        target: target.clone(),
        observation: FsObservation::Present {
            version: version.clone(),
        },
        actor: Some(exec.clone() as FsActor),
    });
    let mut result = Map::new();
    result.insert("path".into(), Value::String(env.fs.process_path(target)));
    // A provider outcome is a plain struct of strings, so it always serializes to an object.
    if let Ok(Value::Object(fields)) = serde_json::to_value(outcome) {
        for (key, value) in fields {
            if key != "version" {
                result.insert(key, value);
            }
        }
    }
    Value::Object(result)
}
