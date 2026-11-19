//! Git working-tree snapshots through the subprocess capability.

use crate::{
    numstat::{NumstatEntry, parse_numstat},
    paths::{canonical_path, is_inside, to_posix},
};
use hoh_error::BoxError;
use hoh_kernel::Service;
use hoh_subprocess::{
    Subprocess, SubprocessCollect, SubprocessEnvironment, SubprocessOutputMode,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const TERMINATE_GRACE_MS: f64 = 2_000.0;
const STDERR_TAIL_BYTES: u64 = 16 * 1_024;
const CHECK_IGNORE_BATCH_PATHS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitRunResult {
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

pub struct GitRunOptions {
    pub cwd: PathBuf,
    pub env: SubprocessEnvironment,
    pub stdin: Option<String>,
    pub max_bytes: Option<u64>,
    pub signal: AbortSignal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitLimits {
    pub timeout_ms: u64,
    pub output_max_bytes: u64,
}

#[derive(Clone)]
pub struct GitRunner {
    runtime: Service<Subprocess>,
    executable: String,
    limits: GitLimits,
}

impl GitRunner {
    pub fn new(runtime: Service<Subprocess>, executable: String, limits: GitLimits) -> Self {
        Self {
            runtime,
            executable,
            limits,
        }
    }

    pub async fn run(
        &self,
        args: &[&str],
        options: GitRunOptions,
    ) -> Result<GitRunResult, BoxError> {
        let timeout = AbortSignal::timeout(self.limits.timeout_ms as f64);
        let signal = AbortSignal::any([options.signal, timeout.clone()]);
        let mut env: SubprocessEnvironment = IndexMap::new();
        for (name, value) in [
            ("GIT_CONFIG_COUNT", "0"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_OPTIONAL_LOCKS", "0"),
            ("LC_ALL", "C"),
        ] {
            env.insert(name.into(), Some(value.into()));
        }
        for (name, value) in options.env {
            env.insert(name, value);
        }
        let handle = self.runtime.spawn(SubprocessSpawnSpec {
            argv: std::iter::once(self.executable.clone())
                .chain(args.iter().map(|argument| (*argument).to_owned()))
                .collect(),
            cwd: options.cwd.to_string_lossy().into_owned(),
            stdio: SubprocessStdio {
                stdin: options.stdin.map_or(SubprocessStdinMode::Ignore, |data| {
                    SubprocessStdinMode::Data { data }
                }),
                stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: options.max_bytes.unwrap_or(self.limits.output_max_bytes),
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: STDERR_TAIL_BYTES,
                    spill: None,
                }),
                control: None,
            },
            grace_ms: TERMINATE_GRACE_MS,
            signal: Some(signal.clone()),
            env: Some(env),
        })?;
        let outcome = handle.done.await;
        if signal.aborted() {
            return Err(if timeout.aborted() {
                format!(
                    "git {} timed out after {}ms",
                    args.join(" "),
                    self.limits.timeout_ms
                )
                .into()
            } else {
                format!("git {} was aborted", args.join(" ")).into()
            });
        }
        let outcome = outcome.map_err(|error| -> BoxError { Box::new(error) })?;
        let stdout = handle
            .collected
            .stdout
            .as_ref()
            .map(|reader| reader.read_from(0))
            .unwrap_or_else(|| hoh_subprocess::SubprocessOutputRead {
                text: String::new(),
                next_offset: 0,
                lossy: false,
                spill_path: None,
            });
        let stderr = handle
            .collected
            .stderr
            .as_ref()
            .map(|reader| reader.read_from(0).text)
            .unwrap_or_default();
        Ok(GitRunResult {
            exit_code: outcome.exit_code,
            stdout: stdout.text,
            stderr,
            truncated: stdout.lossy,
        })
    }
}

fn ok<'a>(result: &'a GitRunResult, what: &str) -> Result<&'a GitRunResult, BoxError> {
    if result.exit_code == Some(0) {
        Ok(result)
    } else {
        Err(format!("{what} failed: {}", result.stderr.trim()).into())
    }
}

#[derive(Clone, Debug)]
pub struct GitWorkspace {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub scratch: PathBuf,
    pub env: SubprocessEnvironment,
    pub excludes: Vec<String>,
}

static UNIQUE_DIRECTORY: AtomicU64 = AtomicU64::new(1);

async fn unique_directory(parent: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    for _ in 0..1_000 {
        let number = UNIQUE_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}{}-{number}", std::process::id()));
        let mut builder = tokio::fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&path).await {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique scratch directory",
    ))
}

fn run_options(cwd: &Path, env: SubprocessEnvironment, signal: AbortSignal) -> GitRunOptions {
    GitRunOptions {
        cwd: cwd.to_path_buf(),
        env,
        stdin: None,
        max_bytes: None,
        signal,
    }
}

pub async fn locate_git_workspace(
    git: &GitRunner,
    cwd: &Path,
    scratch: impl AsyncFnOnce() -> Result<PathBuf, BoxError>,
    signal: AbortSignal,
) -> Result<Option<GitWorkspace>, BoxError> {
    let found = git
        .run(
            &[
                "rev-parse",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-path",
                "objects",
            ],
            run_options(cwd, IndexMap::new(), signal),
        )
        .await?;
    if found.exit_code == Some(128)
        && found
            .stderr
            .to_ascii_lowercase()
            .contains("not a git repository")
    {
        return Ok(None);
    }
    let lines: Vec<&str> = ok(&found, "git rev-parse")?.stdout.lines().collect();
    if lines.len() < 3 {
        return Err("git rev-parse returned fewer than three paths".into());
    }
    let resolve = |value: &str| {
        let value = Path::new(value);
        if value.is_absolute() {
            value.to_path_buf()
        } else {
            cwd.join(value)
        }
    };
    let root = resolve(lines[0]);
    let git_dir = resolve(lines[1]);
    let repository_objects = resolve(lines[2]);
    let directory = canonical_path(&scratch().await?).await;
    let objects = directory.join("objects");
    tokio::fs::create_dir_all(&objects).await?;
    let excludes = is_inside(&root, &directory)
        .then(|| to_posix(directory.strip_prefix(&root).expect("inside root")))
        .into_iter()
        .collect();
    let mut env = IndexMap::new();
    env.insert(
        "GIT_OBJECT_DIRECTORY".into(),
        Some(objects.to_string_lossy().into_owned()),
    );
    env.insert(
        "GIT_ALTERNATE_OBJECT_DIRECTORIES".into(),
        Some(repository_objects.to_string_lossy().into_owned()),
    );
    Ok(Some(GitWorkspace {
        root,
        git_dir,
        scratch: directory,
        env,
        excludes,
    }))
}

pub async fn snapshot_tree(
    git: &GitRunner,
    workspace: &GitWorkspace,
    signal: AbortSignal,
) -> Result<String, BoxError> {
    let scratch = unique_directory(&workspace.scratch, "index-").await?;
    let result = async {
        let index = scratch.join("index");
        match tokio::fs::copy(workspace.git_dir.join("index"), &index).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut env = workspace.env.clone();
        env.insert(
            "GIT_INDEX_FILE".into(),
            Some(index.to_string_lossy().into_owned()),
        );
        let mut arguments = vec!["add".to_owned(), "--all".into(), "--ignore-errors".into()];
        if !workspace.excludes.is_empty() {
            arguments.extend(["--".into(), ".".into()]);
            arguments.extend(
                workspace
                    .excludes
                    .iter()
                    .map(|path| format!(":(exclude){path}")),
            );
        }
        let references: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let added = git
            .run(
                &references,
                run_options(&workspace.root, env.clone(), signal.clone()),
            )
            .await?;
        if added.exit_code != Some(1) {
            ok(&added, &format!("git add in {}", workspace.root.display()))?;
        }
        let written = git
            .run(&["write-tree"], run_options(&workspace.root, env, signal))
            .await?;
        Ok::<_, BoxError>(ok(&written, "git write-tree")?.stdout.trim().to_owned())
    }
    .await;
    let cleanup = tokio::fs::remove_dir_all(&scratch).await;
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeBlob {
    pub oid: String,
    pub size: u64,
}

pub async fn tree_blob(
    git: &GitRunner,
    workspace: &GitWorkspace,
    tree: &str,
    path: &str,
    signal: AbortSignal,
) -> Result<Option<TreeBlob>, BoxError> {
    let mut env = workspace.env.clone();
    env.insert("GIT_LITERAL_PATHSPECS".into(), Some("1".into()));
    let result = git
        .run(
            &["ls-tree", "-z", "-l", tree, "--", path],
            run_options(&workspace.root, env, signal),
        )
        .await?;
    let entry = ok(&result, "git ls-tree")?
        .stdout
        .split('\0')
        .next()
        .unwrap_or_default();
    let Some((metadata, _path)) = entry.split_once('\t') else {
        return Ok(None);
    };
    let fields: Vec<&str> = metadata.split_whitespace().collect();
    if fields.len() != 4 || fields[1] != "blob" {
        return Ok(None);
    }
    Ok(Some(TreeBlob {
        oid: fields[2].to_owned(),
        size: fields[3].parse()?,
    }))
}

pub async fn blob_text(
    git: &GitRunner,
    workspace: &GitWorkspace,
    oid: &str,
    max_bytes: u64,
    signal: AbortSignal,
) -> Result<String, BoxError> {
    let mut options = run_options(&workspace.root, workspace.env.clone(), signal);
    options.max_bytes = Some(max_bytes);
    let result = git.run(&["cat-file", "blob", oid], options).await?;
    let result = ok(&result, "git cat-file")?;
    if result.truncated {
        return Err(format!("blob {oid} exceeds {max_bytes} bytes").into());
    }
    Ok(result.stdout.clone())
}

pub async fn diff_trees(
    git: &GitRunner,
    workspace: &GitWorkspace,
    before: &str,
    after: &str,
    signal: AbortSignal,
) -> Result<Vec<NumstatEntry>, BoxError> {
    if before == after {
        return Ok(Vec::new());
    }
    let result = git
        .run(
            &["diff-tree", "-r", "-M", "-z", "--numstat", before, after],
            run_options(&workspace.root, workspace.env.clone(), signal),
        )
        .await?;
    let result = ok(&result, "git diff-tree")?;
    if result.truncated {
        return Err("git diff-tree output exceeded the configured cap".into());
    }
    parse_numstat(&result.stdout)
}

pub async fn gitlink_paths(
    git: &GitRunner,
    workspace: &GitWorkspace,
    signal: AbortSignal,
) -> Result<HashSet<String>, BoxError> {
    let result = git
        .run(
            &["ls-files", "-z", "--stage"],
            run_options(&workspace.root, workspace.env.clone(), signal),
        )
        .await?;
    let mut links = HashSet::new();
    for entry in ok(&result, "git ls-files")?.stdout.split('\0') {
        if entry.starts_with("160000 ")
            && let Some((_, path)) = entry.split_once('\t')
        {
            links.insert(path.to_owned());
        }
    }
    Ok(links)
}

pub async fn ignored_paths(
    git: &GitRunner,
    workspace: &GitWorkspace,
    paths: &[String],
    signal: AbortSignal,
) -> Result<HashSet<String>, BoxError> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut ignored = HashSet::new();
    for paths in paths.chunks(CHECK_IGNORE_BATCH_PATHS) {
        let mut options = run_options(&workspace.root, workspace.env.clone(), signal.clone());
        options.stdin = Some(paths.iter().fold(String::new(), |mut input, path| {
            input.push_str(path);
            input.push('\0');
            input
        }));
        let result = git
            .run(
                &["check-ignore", "-z", "-v", "--non-matching", "--stdin"],
                options,
            )
            .await?;
        if result.truncated {
            return Err("git check-ignore output exceeded the configured cap".into());
        }
        if !matches!(result.exit_code, Some(0 | 1)) {
            ok(&result, "git check-ignore")?;
        }
        let fields: Vec<&str> = result.stdout.split_terminator('\0').collect();
        if fields.len() == paths.len().saturating_mul(4) {
            for record in fields.chunks_exact(4) {
                if !record[2].is_empty() && !record[2].starts_with('!') {
                    ignored.insert(record[3].to_owned());
                }
            }
            continue;
        }

        // The local subprocess provider can publish batch stdin after a very short child has
        // observed EOF. Verbose non-matching output makes that detectable; preserve correctness
        // with literal argv checks only for the affected batch.
        let mut pending = vec![paths];
        while let Some(group) = pending.pop() {
            let mut arguments = if group.len() == 1 {
                vec!["check-ignore", "-q", "--"]
            } else {
                vec!["check-ignore", "--"]
            };
            arguments.extend(group.iter().map(String::as_str));
            let result = git
                .run(
                    &arguments,
                    run_options(&workspace.root, workspace.env.clone(), signal.clone()),
                )
                .await?;
            match result.exit_code {
                Some(0) => {
                    if let [path] = group {
                        ignored.insert(path.clone());
                    } else {
                        let middle = group.len() / 2;
                        pending.push(&group[..middle]);
                        pending.push(&group[middle..]);
                    }
                }
                Some(1) => {}
                _ => {
                    ok(&result, "git check-ignore")?;
                }
            }
        }
    }
    Ok(ignored)
}
