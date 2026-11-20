//! Filesystem-backed skill provider.

mod frontmatter;
mod watch;

use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use feruca::{Collator, Locale, Tailoring};
use futures_util::future::BoxFuture;
use hoh_error::{BoxError, HarnessError};
use hoh_fs::{Fs, FsErrorCode, FsObserved, FsResolveOptions, FsType};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_skill::{
    BUNDLED_SKILL_RANK, NativeError, SkillCandidate, SkillDefinition, SkillError, SkillErrorCode,
    SkillLookupOptions, SkillProvider, SkillProviderControl, SkillProviderObservation,
    SkillProviderOutput, Skills, is_skill_name,
};
use hoh_tools::ToolExecution;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{frontmatter::Frontmatter, watch::WatchManager};

pub const NAME: &str = "skill-filesystem";
pub const INJECT: [&str; 1] = ["skills"];
pub const PROJECT_DSH_RANK: u64 = 100;
pub const PROJECT_AGENTS_RANK: u64 = 200;
pub const CUSTOM_RANK: u64 = 300;
pub const USER_DSH_RANK: u64 = 400;
pub const USER_AGENTS_RANK: u64 = 500;
pub const DEFAULT_WATCH_STABILITY_THRESHOLD_MS: u64 = 200;
pub const DEFAULT_WATCH_POLL_INTERVAL_MS: u64 = 100;
pub const DEFAULT_WATCH_MAX_PROJECTS: usize = 128;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn default_provider_name() -> String {
    "filesystem".into()
}
const fn default_true() -> bool {
    true
}
const fn default_stability() -> f64 {
    DEFAULT_WATCH_STABILITY_THRESHOLD_MS as f64
}
const fn default_poll_interval() -> f64 {
    DEFAULT_WATCH_POLL_INTERVAL_MS as f64
}
const fn default_max_projects() -> f64 {
    DEFAULT_WATCH_MAX_PROJECTS as f64
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_provider_name")]
    #[schemars(length(min = 1))]
    pub provider_name: String,
    #[serde(default = "default_true")]
    pub include_default_roots: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_home: Option<String>,
    #[serde(default)]
    pub custom_skill_dirs: Vec<String>,
    #[serde(default = "default_true")]
    pub watch: bool,
    #[serde(default)]
    pub watch_use_polling: bool,
    #[serde(default = "default_stability")]
    pub watch_stability_threshold_ms: f64,
    #[serde(default = "default_poll_interval")]
    pub watch_poll_interval_ms: f64,
    #[serde(default = "default_max_projects")]
    pub watch_max_projects: f64,
    #[serde(default = "default_true")]
    pub watch_follow_symlinks: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled_skill_dir: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider_name: default_provider_name(),
            include_default_roots: true,
            dsh_home: None,
            agents_home: None,
            custom_skill_dirs: vec![],
            watch: true,
            watch_use_polling: false,
            watch_stability_threshold_ms: default_stability(),
            watch_poll_interval_ms: default_poll_interval(),
            watch_max_projects: default_max_projects(),
            watch_follow_symlinks: true,
            bundled_skill_dir: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SkillRoot {
    path: PathBuf,
    source: String,
    rank: u64,
    skip_system: bool,
    project_root: Option<PathBuf>,
    trusted_host: bool,
}

#[derive(Clone, Debug)]
struct LocalLocator {
    path: PathBuf,
    directory: PathBuf,
}

struct ParsedSkill {
    path: String,
    content: String,
    name: String,
    description: String,
    when_to_use: Option<String>,
    invocation: Value,
    metadata: Option<Value>,
}

#[derive(Clone, Copy)]
pub(crate) struct ResolvedWatchConfig {
    enabled: bool,
    use_polling: bool,
    stability_threshold_ms: u64,
    poll_interval_ms: u64,
    max_projects: usize,
    follow_symlinks: bool,
}

fn positive_integer(field: &str, value: f64) -> Result<u64, BoxError> {
    if !value.is_finite() || value.fract() != 0.0 || value < 1.0 || value > u64::MAX as f64 {
        return Err(NativeError::Type(format!(
            "skill-filesystem: {field} must be a positive integer"
        ))
        .into());
    }
    Ok(value as u64)
}

fn resolve_watch_config(config: &Config) -> Result<ResolvedWatchConfig, BoxError> {
    Ok(ResolvedWatchConfig {
        enabled: config.watch,
        use_polling: config.watch_use_polling,
        stability_threshold_ms: positive_integer(
            "watchStabilityThresholdMs",
            config.watch_stability_threshold_ms,
        )?,
        poll_interval_ms: positive_integer("watchPollIntervalMs", config.watch_poll_interval_ms)?,
        max_projects: usize::try_from(positive_integer(
            "watchMaxProjects",
            config.watch_max_projects,
        )?)
        .map_err(|_| "skill-filesystem: watchMaxProjects must be a positive integer")?,
        follow_symlinks: config.watch_follow_symlinks,
    })
}

struct Inner {
    ctx: Context,
    name: String,
    include_default_roots: bool,
    dsh_home: PathBuf,
    agents_home: PathBuf,
    custom_skill_dirs: Vec<PathBuf>,
    bundled_skill_dir: Option<PathBuf>,
    watch: WatchManager,
}

#[derive(Clone)]
pub struct FileSystemSkillProvider(Arc<Inner>);

impl FileSystemSkillProvider {
    pub fn new(
        ctx: Context,
        control: SkillProviderControl,
        config: Config,
    ) -> Result<Self, BoxError> {
        if config.provider_name.is_empty() {
            return Err("skill-filesystem: providerName must be a non-empty string".into());
        }
        let dsh_home = hoh_home_paths::resolve_dsh_home(config.dsh_home.as_deref(), None)?;
        let agents_home = hoh_home_paths::resolve_agents_home(config.agents_home.as_deref(), None)?;
        let custom_skill_dirs = config
            .custom_skill_dirs
            .iter()
            .map(|path| absolute(path))
            .collect::<Result<Vec<_>, _>>()?;
        let bundled = config
            .bundled_skill_dir
            .clone()
            .or_else(|| {
                config
                    .include_default_roots
                    .then(|| std::env::var("DSH_BUNDLED_SKILL_DIR").ok())
                    .flatten()
            })
            .map(|path| absolute(&path))
            .transpose()?;
        let watch_config = resolve_watch_config(&config)?;
        Ok(Self(Arc::new(Inner {
            watch: WatchManager::new(ctx.clone(), control, watch_config),
            ctx,
            name: config.provider_name,
            include_default_roots: config.include_default_roots,
            dsh_home,
            agents_home,
            custom_skill_dirs,
            bundled_skill_dir: bundled,
        })))
    }

    pub fn observe_host_mutation(&self, path: &str) {
        if let Ok(path) = absolute(path) {
            self.0.watch.observe_host_mutation(&path);
        }
    }

    pub fn dispose(&self) {
        self.0.watch.dispose();
    }

    async fn roots(&self, cwd: Option<&str>) -> Result<Vec<SkillRoot>, BoxError> {
        let mut roots = vec![];
        if self.0.include_default_roots
            && let Some(cwd) = cwd
        {
            let cwd = absolute(cwd)?;
            let project = find_project_root(&cwd, optional_fs(&self.0.ctx)).await;
            roots.push(SkillRoot {
                path: project.join(".dsh/skills"),
                source: "project-dsh".into(),
                rank: PROJECT_DSH_RANK,
                skip_system: false,
                project_root: Some(project.clone()),
                trusted_host: false,
            });
            roots.push(SkillRoot {
                path: project.join(".agents/skills"),
                source: "project-agents".into(),
                rank: PROJECT_AGENTS_RANK,
                skip_system: false,
                project_root: Some(project),
                trusted_host: false,
            });
        }
        roots.extend(
            self.0
                .custom_skill_dirs
                .iter()
                .cloned()
                .map(|path| SkillRoot {
                    path,
                    source: "custom".into(),
                    rank: CUSTOM_RANK,
                    skip_system: false,
                    project_root: None,
                    trusted_host: false,
                }),
        );
        if self.0.include_default_roots {
            roots.push(SkillRoot {
                path: self.0.dsh_home.join("skills"),
                source: "user-dsh".into(),
                rank: USER_DSH_RANK,
                skip_system: true,
                project_root: None,
                trusted_host: false,
            });
            roots.push(SkillRoot {
                path: self.0.agents_home.join("skills"),
                source: "user-agents".into(),
                rank: USER_AGENTS_RANK,
                skip_system: false,
                project_root: None,
                trusted_host: false,
            });
        }
        if let Some(path) = &self.0.bundled_skill_dir {
            roots.push(SkillRoot {
                path: path.clone(),
                source: "bundled".into(),
                rank: BUNDLED_SKILL_RANK,
                skip_system: false,
                project_root: None,
                trusted_host: true,
            });
        }
        Ok(roots)
    }

    async fn list_inner(
        &self,
        options: Arc<SkillLookupOptions>,
    ) -> Result<SkillProviderOutput, BoxError> {
        let roots = self.roots(options.cwd.as_deref()).await?;
        let mut complete = self.0.watch.observe_roots(&roots).await.is_ok();
        let mut candidates = vec![];
        for root in roots {
            if let Some(signal) = &options.signal {
                signal.throw_if_aborted()?;
            }
            match discover_root(&root, &self.0.ctx, &self.0.name, options.signal.as_ref()).await {
                Ok(mut found) => {
                    if let Some(signal) = &options.signal {
                        signal.throw_if_aborted()?;
                    }
                    candidates.append(&mut found);
                }
                Err(error) => {
                    if let Some(signal) = &options.signal {
                        signal.throw_if_aborted()?;
                    }
                    complete = false;
                    self.0.ctx.logger().warn(format_args!(
                        "skill root \"{}\" skipped: {}. Other roots remain available; check this directory's permissions and symlinks. Discovery retries on the next lookup.",
                        root.path.display(), error
                    ));
                }
            }
        }
        Ok(if complete {
            SkillProviderOutput::Candidates(candidates)
        } else {
            SkillProviderOutput::Observation(SkillProviderObservation {
                candidates,
                complete: false,
            })
        })
    }

    async fn get_inner(
        &self,
        candidate: SkillCandidate,
        options: Arc<SkillLookupOptions>,
    ) -> Result<Option<SkillDefinition>, BoxError> {
        let locator = candidate
            .locator::<LocalLocator>()
            .ok_or_else(|| "skill-filesystem: candidate has no local locator".to_owned())?;
        let trusted = candidate.text("source") == Some("bundled");
        let Some(parsed) =
            parse_skill_file(&locator.path, &self.0.ctx, options.signal.as_ref(), trusted).await?
        else {
            return Ok(None);
        };
        let mut object = Map::new();
        object.insert("name".into(), Value::String(parsed.name));
        object.insert("description".into(), Value::String(parsed.description));
        if let Some(when_to_use) = parsed.when_to_use {
            object.insert("whenToUse".into(), Value::String(when_to_use));
        }
        object.insert("invocation".into(), parsed.invocation);
        object.insert(
            "source".into(),
            Value::String(candidate.text("source").unwrap_or_default().into()),
        );
        object.insert("provider".into(), Value::String(self.0.name.clone()));
        object.insert(
            "resourceBase".into(),
            json!({"kind":"directory", "path": path_string(&locator.directory)}),
        );
        object.insert("path".into(), Value::String(parsed.path));
        if let Some(metadata) = parsed.metadata {
            object.insert("metadata".into(), metadata);
        }
        object.insert("content".into(), Value::String(parsed.content));
        Ok(Some(SkillDefinition::from_value(Value::Object(object))))
    }
}

impl SkillProvider for FileSystemSkillProvider {
    fn name(&self) -> &str {
        &self.0.name
    }

    fn list(
        &self,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>> {
        let this = self.clone();
        Box::pin(async move { this.list_inner(options).await })
    }

    fn get(
        &self,
        candidate: SkillCandidate,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>> {
        let this = self.clone();
        Box::pin(async move { this.get_inner(candidate, options).await })
    }
}

#[derive(Inject)]
pub struct Deps {
    skills: Service<Skills>,
}

pub struct SkillFileSystemPlugin;

impl Plugin for SkillFileSystemPlugin {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let holder = Arc::new(Mutex::new(None::<Arc<FileSystemSkillProvider>>));
        let output = holder.clone();
        let provider_ctx = ctx.clone();
        let resolved = config.get().as_ref().clone();
        deps.skills.register_provider(&ctx, move |control| {
            let provider = Arc::new(FileSystemSkillProvider::new(
                provider_ctx,
                control,
                resolved,
            )?);
            *lock(&output) = Some(provider.clone());
            Ok(provider)
        })?;
        let provider = lock(&holder)
            .clone()
            .ok_or("skill-filesystem: provider registration did not initialize")?;
        let cleanup = provider.clone();
        ctx.effect("skill-filesystem watcher", move || {
            Undo::sync(move || cleanup.dispose())
        })?;
        ctx.on::<FsObserved>(move |event| {
            let tool = event
                .actor
                .as_ref()
                .and_then(|actor| actor.downcast_ref::<ToolExecution>());
            if tool.is_some_and(|tool| matches!(tool.name.as_str(), "edit" | "write")) {
                provider.observe_host_mutation(&event.target.display_path);
            }
        })?;
        Ok(())
    }
}

async fn discover_root(
    root: &SkillRoot,
    ctx: &Context,
    provider: &str,
    signal: Option<&hoh_timeout::AbortSignal>,
) -> Result<Vec<SkillCandidate>, BoxError> {
    let mut entries = list_root_entries(root, ctx, signal).await?;
    let mut collator = Collator::new(Tailoring::Cldr(Locale::Root), false, false);
    entries.sort_by(|left, right| collator.collate(&left.0, &right.0));
    let mut skills = vec![];
    for (name, kind, path) in entries {
        if root.skip_system && name == ".system" {
            continue;
        }
        let locator = match kind {
            FsType::Directory => LocalLocator {
                path: path.join("SKILL.md"),
                directory: path,
            },
            FsType::File if name.ends_with(".md") => LocalLocator {
                path,
                directory: root.path.clone(),
            },
            _ => continue,
        };
        let Some(parsed) = parse_skill_file(&locator.path, ctx, signal, root.trusted_host).await?
        else {
            continue;
        };
        let mut object = Map::new();
        object.insert("name".into(), Value::String(parsed.name));
        object.insert("description".into(), Value::String(parsed.description));
        if let Some(when_to_use) = parsed.when_to_use {
            object.insert("whenToUse".into(), Value::String(when_to_use));
        }
        object.insert("invocation".into(), parsed.invocation);
        object.insert("provider".into(), Value::String(provider.into()));
        object.insert("source".into(), Value::String(root.source.clone()));
        object.insert("rank".into(), json!(root.rank));
        object.insert(
            "locator".into(),
            json!({"path":path_string(&locator.path), "directory":path_string(&locator.directory)}),
        );
        object.insert(
            "resourceBase".into(),
            json!({"kind":"directory", "path":path_string(&locator.directory)}),
        );
        object.insert("path".into(), Value::String(parsed.path));
        if let Some(metadata) = parsed.metadata {
            object.insert("metadata".into(), metadata);
        }
        skills.push(
            SkillCandidate::from_value(Value::Object(object)).with_locator(Arc::new(locator)),
        );
    }
    Ok(skills)
}

async fn list_root_entries(
    root: &SkillRoot,
    ctx: &Context,
    signal: Option<&hoh_timeout::AbortSignal>,
) -> Result<Vec<(String, FsType, PathBuf)>, BoxError> {
    if !root.trusted_host
        && let Some(fs) = optional_fs(ctx)
    {
        let target = match fs
            .resolve(&path_string(&root.path), FsResolveOptions::default())
            .await
        {
            Ok(target) => target,
            Err(error) if is_absent_error(error.as_ref()) => return Ok(vec![]),
            Err(error) => {
                if let Some(signal) = signal {
                    signal.throw_if_aborted()?;
                }
                return Err(error);
            }
        };
        let entries = match fs.list_dir(&target, signal.cloned()).await {
            Ok(entries) => entries,
            Err(error) if is_absent_error(error.as_ref()) => return Ok(vec![]),
            Err(error) => {
                if let Some(signal) = signal {
                    signal.throw_if_aborted()?;
                }
                return Err(error);
            }
        };
        return Ok(entries
            .into_iter()
            .map(|entry| {
                (
                    entry.name,
                    entry.r#type,
                    PathBuf::from(entry.target.display_path),
                )
            })
            .collect());
    }
    let mut directory = match tokio::fs::read_dir(&root.path).await {
        Ok(directory) => directory,
        Err(error) if is_absent_io(&error) => return Ok(vec![]),
        Err(error) => return Err(Box::new(error)),
    };
    let mut entries = vec![];
    while let Some(entry) = directory.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let file_type = entry.file_type().await?;
        let kind = if file_type.is_dir() {
            FsType::Directory
        } else if file_type.is_file() {
            FsType::File
        } else if file_type.is_symlink() {
            match tokio::fs::metadata(&path).await {
                Ok(metadata) if metadata.is_dir() => FsType::Directory,
                Ok(metadata) if metadata.is_file() => FsType::File,
                Ok(_) => FsType::Other,
                Err(error) => {
                    ctx.logger().warn(format_args!(
                        "skill entry {} ignored: failed to follow symbolic link: {error}",
                        path.display()
                    ));
                    FsType::Other
                }
            }
        } else {
            FsType::Other
        };
        entries.push((name, kind, path));
    }
    Ok(entries)
}

async fn parse_skill_file(
    path: &Path,
    ctx: &Context,
    signal: Option<&hoh_timeout::AbortSignal>,
    trusted_host: bool,
) -> Result<Option<ParsedSkill>, BoxError> {
    let read = finish_optional_read(
        read_skill_text(path, ctx, signal, trusted_host).await?,
        signal,
    )?;
    let Some((resolved_path, raw)) = read else {
        return Ok(None);
    };
    let Frontmatter { data, body } = match frontmatter::parse(&raw) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            ctx.logger().warn(format_args!(
                "skill file {} ignored: missing YAML frontmatter",
                path.display()
            ));
            return Ok(None);
        }
        Err(error) => {
            ctx.logger().warn(format_args!(
                "skill file {} ignored: invalid YAML frontmatter: {error}",
                path.display()
            ));
            return Ok(None);
        }
    };
    let Some(name) = string_field(&data, "name") else {
        warn_required(ctx, path);
        return Ok(None);
    };
    let Some(description) = string_field(&data, "description") else {
        warn_required(ctx, path);
        return Ok(None);
    };
    if !is_skill_name(&name) {
        ctx.logger().warn(format_args!(
            "skill file {} ignored: invalid skill name \"{name}\"",
            path.display()
        ));
        return Ok(None);
    }
    let invocation = match invocation_policy(&data) {
        Ok(invocation) => invocation,
        Err(error) => {
            ctx.logger().warn(format_args!(
                "skill file {} ignored: invalid invocation frontmatter: {error}",
                path.display()
            ));
            return Ok(None);
        }
    };
    Ok(Some(ParsedSkill {
        path: resolved_path,
        content: body.trim().to_owned(),
        name,
        description,
        when_to_use: string_field(&data, "whenToUse"),
        invocation,
        metadata: data
            .get("metadata")
            .and_then(Value::as_object)
            .map(|object| Value::Object(object.clone())),
    }))
}

fn finish_optional_read<T>(
    value: Option<T>,
    signal: Option<&hoh_timeout::AbortSignal>,
) -> Result<Option<T>, BoxError> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    Ok(value)
}

async fn read_skill_text(
    path: &Path,
    ctx: &Context,
    signal: Option<&hoh_timeout::AbortSignal>,
    trusted_host: bool,
) -> Result<Option<(String, String)>, BoxError> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    if !trusted_host && let Some(fs) = optional_fs(ctx) {
        let target = match fs
            .resolve(&path_string(path), FsResolveOptions::default())
            .await
        {
            Ok(target) => target,
            Err(error) if is_absent_error(error.as_ref()) => return Ok(None),
            Err(error) => {
                if let Some(signal) = signal {
                    signal.throw_if_aborted()?;
                }
                return Err(error);
            }
        };
        if let Some(signal) = signal {
            signal.throw_if_aborted()?;
        }
        match fs.stat(&target, signal.cloned()).await {
            Ok(Some(info)) if info.r#type == FsType::File => {}
            Ok(_) => return Ok(None),
            Err(error) if is_absent_error(error.as_ref()) => return Ok(None),
            Err(error) => {
                if let Some(signal) = signal {
                    signal.throw_if_aborted()?;
                }
                return Err(error);
            }
        }
        match fs.read_text(&target, signal.cloned()).await {
            Ok(content) => return Ok(Some((fs.process_path(&target), content))),
            Err(error) if is_absent_error(error.as_ref()) => return Ok(None),
            Err(error)
                if HarnessError::of(error.as_ref())
                    .is_some_and(|error| error.code() == FsErrorCode::NotText.as_str()) =>
            {
                ctx.logger().warn(format_args!(
                    "skill file {} ignored: failed to read text file at {}: {error}",
                    path.display(),
                    target.display_path
                ));
                return Ok(None);
            }
            Err(error) => {
                if let Some(signal) = signal {
                    signal.throw_if_aborted()?;
                }
                return Err(error);
            }
        }
    }
    let canonical = match tokio::fs::canonicalize(path).await {
        Ok(path) => path,
        Err(error) if is_absent_io(&error) => return Ok(None),
        Err(error) => {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            return Err(skill_read_error(path, error));
        }
    };
    let content = match tokio::fs::read(&canonical).await {
        Ok(content) => String::from_utf8_lossy(&content).into_owned(),
        Err(error) if is_absent_io(&error) => return Ok(None),
        Err(error) => {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            return Err(skill_read_error(path, error));
        }
    };
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    Ok(Some((path_string(&canonical), content)))
}

fn skill_read_error(path: &Path, cause: io::Error) -> BoxError {
    SkillError::new(
        format!("failed to read skill file {}: {cause}", path.display()),
        SkillErrorCode::SkillReadFailed,
    )
    .with_cause(cause)
    .into()
}

fn invocation_policy(data: &Map<String, Value>) -> Result<Value, String> {
    for (legacy, canonical) in [
        ("disableModelInvocation", "disable-model-invocation"),
        ("modelInvocable", "disable-model-invocation"),
        ("userInvocable", "user-invocable"),
    ] {
        if data.contains_key(legacy) {
            return Err(format!(
                "frontmatter field \"{legacy}\" is unsupported; use \"{canonical}\""
            ));
        }
    }
    let disabled = frontmatter_bool(data, "disable-model-invocation")?;
    let user = frontmatter_bool(data, "user-invocable")?;
    Ok(json!({
        "modelInvocable": disabled != Some(true),
        "userInvocable": user != Some(false),
    }))
}

fn frontmatter_bool(data: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    let Some(value) = data.get(key) else {
        return Ok(None);
    };
    if let Some(value) = value.as_bool() {
        return Ok(Some(value));
    }
    if value.as_i64() == Some(1) {
        return Ok(Some(true));
    }
    if value.as_i64() == Some(0) {
        return Ok(Some(false));
    }
    if value.as_f64() == Some(1.0) {
        return Ok(Some(true));
    }
    if value.as_f64() == Some(0.0) {
        return Ok(Some(false));
    }
    if let Some(value) = value.as_str() {
        return match value.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(Some(true)),
            "false" | "no" | "off" | "0" => Ok(Some(false)),
            _ => Err(format!("frontmatter field \"{key}\" must be a boolean")),
        };
    }
    Err(format!("frontmatter field \"{key}\" must be a boolean"))
}

fn string_field(data: &Map<String, Value>, key: &str) -> Option<String> {
    data.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn warn_required(ctx: &Context, path: &Path) {
    ctx.logger().warn(format_args!(
        "skill file {} ignored: frontmatter requires name and description",
        path.display()
    ));
}

fn optional_fs(ctx: &Context) -> Option<Service<Fs>> {
    ctx.get::<Fs>()
}

async fn find_project_root(cwd: &Path, fs: Option<Service<Fs>>) -> PathBuf {
    let mut current = cwd.to_path_buf();
    loop {
        if path_exists(&current.join(".git"), fs.as_ref()).await {
            return current;
        }
        let Some(parent) = current.parent() else {
            return cwd.to_path_buf();
        };
        if parent == current {
            return cwd.to_path_buf();
        }
        current = parent.to_path_buf();
    }
}

async fn path_exists(path: &Path, fs: Option<&Service<Fs>>) -> bool {
    if let Some(fs) = fs {
        let Ok(target) = fs
            .resolve(&path_string(path), FsResolveOptions::default())
            .await
        else {
            return false;
        };
        return fs
            .stat(&target, None)
            .await
            .is_ok_and(|info| info.is_some());
    }
    tokio::fs::metadata(path).await.is_ok()
}

pub(crate) fn is_potential_skill_path(root: &SkillRoot, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(&root.path) else {
        return false;
    };
    let segments = relative.components().collect::<Vec<_>>();
    if segments.is_empty() || segments.len() > 2 {
        return false;
    }
    let first = segments[0].as_os_str().to_string_lossy();
    if root.skip_system && first == ".system" {
        return false;
    }
    if segments.len() == 1 {
        first.ends_with(".md")
    } else {
        segments[1].as_os_str() == "SKILL.md"
    }
}

fn is_absent_error(error: &(dyn std::error::Error + 'static)) -> bool {
    HarnessError::of(error)
        .is_some_and(|error| matches!(error.code(), "FS_NOT_FOUND" | "FS_NOT_DIRECTORY"))
        || error.downcast_ref::<io::Error>().is_some_and(is_absent_io)
}

fn is_absent_io(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn absolute(path: &str) -> io::Result<PathBuf> {
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    Ok(PathBuf::from(hoh_util_workspace_path::node_path::resolve(
        &cwd,
        &[if path.is_empty() { "." } else { path }],
    )))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use hoh_timeout::AbortController;

    use super::finish_optional_read;

    #[test]
    fn an_absent_read_does_not_swallow_a_concurrent_abort() {
        let controller = AbortController::new();
        controller.abort();

        assert!(finish_optional_read::<()>(None, Some(&controller.signal())).is_err());
    }
}
