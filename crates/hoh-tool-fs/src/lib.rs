//! Port of @deepseek-ai/dsh-tool-fs (packages/fs/tool-fs).
//!
//! Model-facing `read`, `read_image`, `write`, `edit`, and `apply_patch` tools over the mounted
//! `fs` service. This package owns schemas, validation, read windows, formatting, and observation
//! events, never a concrete provider. An optional event policy supplies mutation guards; without
//! one the tools use unconditional provider calls.

pub mod apply_patch;
pub mod apply_patch_output;
pub mod apply_patch_presentation;
pub mod diff;
pub mod edit;
pub mod error;
pub mod image_error;
pub mod mutation_result;
pub mod patch;
pub mod read;
pub mod read_image;
pub mod read_render;
pub mod read_target;
pub mod sandbox;
pub mod session_cwd;
pub mod shim;
pub mod write;

use std::sync::Arc;

use hoh_attachment::Attachments;
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_system_prompt::SystemPromptKey;
use hoh_tools::Tools;
use hoh_working_directory::WorkingDirectoryKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::apply_patch::apply_apply_patch_tool;
use crate::edit::apply_edit_tool;
use crate::read::{READ_LIMIT, ReadToolCaps, STREAM_MIN_SIZE, apply_read_tool};
use crate::read_image::apply_read_image_tool;
use crate::read_render::{READ_MAX_BYTES, READ_MAX_LINE_LENGTH};
use crate::sandbox::FsSandboxController;
use crate::write::apply_write_tool;

/// Plugin config (all optional — [`Config::default`] supplies the defaults).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Register `read` and its guidance. Defaults to true.
    pub read: bool,
    /// Register `read_image` while an attachment store is mounted. Defaults to true.
    pub read_image: bool,
    /// Register `write` and its guidance. Defaults to true.
    pub write: bool,
    /// Register `edit` and its guidance. Defaults to true.
    pub edit: bool,
    /// Register `apply_patch` and its guidance beside `edit`. Defaults to false; shipped
    /// compositions set it on their `tool-fs` rows.
    pub apply_patch: bool,
    /// Default and maximum number of lines returned by one `read` call.
    pub read_limit: f64,
    /// Maximum characters returned for a single line before truncation.
    pub read_max_line_length: f64,
    /// Maximum bytes returned for the selected lines of one `read` call.
    pub read_max_bytes: f64,
    /// Files at or above this size stream instead of loading whole into memory.
    pub read_stream_min_size: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            read: true,
            read_image: true,
            write: true,
            edit: true,
            apply_patch: false,
            read_limit: READ_LIMIT,
            read_max_line_length: READ_MAX_LINE_LENGTH as f64,
            read_max_bytes: READ_MAX_BYTES as f64,
            read_stream_min_size: STREAM_MIN_SIZE,
        }
    }
}

/// Every read cap counts lines/chars/bytes — a positive integer, or windowing arithmetic
/// misbehaves silently.
fn assert_positive_integer(name: &str, value: f64) -> Result<(), BoxError> {
    if value.is_finite() && value.fract() == 0.0 && value >= 1.0 {
        Ok(())
    } else {
        Err(format!("tool-fs: {name} must be a positive integer").into())
    }
}

/// The services every tool of this package reads (`static inject = ['tools', 'fs', 'systemPrompt',
/// 'workingDirectory']`) with the context that owns the registrations.
#[derive(Clone)]
pub struct Env {
    pub ctx: Context,
    pub fs: Service<Fs>,
    pub working_directory: Service<WorkingDirectoryKey>,
    pub tools: Service<Tools>,
    pub system_prompt: Service<SystemPromptKey>,
}

/// Services required by the filesystem tool suite.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    fs: Service<Fs>,
    system_prompt: Service<SystemPromptKey>,
    working_directory: Service<WorkingDirectoryKey>,
}

/// The optional attachment store `read_image` waits for (`ctx.inject(['attachments'], …)`).
#[derive(Inject)]
struct AttachmentsDep {
    #[allow(dead_code)]
    attachments: Service<Attachments>,
}

/// Register the selected filesystem tools; `read_image` additionally requires a mounted attachment
/// store.
pub struct ToolFs;

impl Plugin for ToolFs {
    const NAME: &'static str = "tool-fs";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        assert_positive_integer("readLimit", config.read_limit)?;
        assert_positive_integer("readMaxLineLength", config.read_max_line_length)?;
        assert_positive_integer("readMaxBytes", config.read_max_bytes)?;
        assert_positive_integer("readStreamMinSize", config.read_stream_min_size)?;
        let env = Arc::new(Env {
            ctx: ctx.clone(),
            fs: deps.fs,
            working_directory: deps.working_directory,
            tools: deps.tools,
            system_prompt: deps.system_prompt,
        });
        if config.read {
            apply_read_tool(
                &env,
                ReadToolCaps {
                    limit: config.read_limit,
                    max_line_length: config.read_max_line_length,
                    max_bytes: config.read_max_bytes,
                    stream_min_size: config.read_stream_min_size,
                },
            )?;
        }
        // read_image is composition-conditional: without a mounted attachment store the
        // deployment cannot durably commit image bytes, so the tool never registers; the execute
        // body keeps a defensive re-check for direct callers.
        if config.read_image {
            let image_env = env.clone();
            ctx.inject(move |image_ctx, _deps: AttachmentsDep| {
                let env = Arc::new(Env {
                    ctx: image_ctx,
                    ..(*image_env).clone()
                });
                async move { apply_read_image_tool(&env) }
            })?;
        }
        // One escalation API shared by the mutating tools: advertisement gating, per-call policy
        // resolution, and denial-marker mapping, all keyed off whether the mounted fs confines
        // (fs.sandboxMode).
        if config.write || config.edit || config.apply_patch {
            let sandbox = Arc::new(FsSandboxController::new(&ctx, &*env.fs)?);
            if config.write {
                apply_write_tool(&env, &sandbox)?;
            }
            if config.edit {
                apply_edit_tool(&env, &sandbox)?;
            }
            if config.apply_patch {
                apply_apply_patch_tool(&env, &sandbox)?;
            }
        }
        Ok(())
    }
}
