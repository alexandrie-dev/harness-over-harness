use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    hash::{DefaultHasher, Hash, Hasher},
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_CHANGED_FILES: usize = 500;
const MAX_COMMITS: usize = 100;

pub(super) fn query(directory: File, root: &Path, query: GitQuery) -> Result<Value, String> {
    let cwd = root.to_str().ok_or("The project path is not UTF-8.")?;
    let Some(repo) = Repository::discover(directory, root)? else {
        return no_repository(query, cwd);
    };
    match query {
        GitQuery::Status => repo.status(),
        GitQuery::ListRefs {
            query,
            cursor,
            limit,
            ref_kind,
        } => repo.refs(
            query.as_deref(),
            cursor.unwrap_or(0),
            limit.unwrap_or(100),
            ref_kind,
        ),
        GitQuery::BranchCommits { base_ref } => repo.commits(base_ref.as_deref()),
        GitQuery::DiffPreview {
            base_ref,
            commit_sha,
            ignore_whitespace,
            source_kind,
            tracked_only,
            metadata_only,
            file,
        } => repo.preview(Preview {
            base_ref: base_ref.as_deref(),
            commit_sha: commit_sha.as_deref(),
            ignore_whitespace: ignore_whitespace.unwrap_or(false),
            source_kind,
            tracked_only: tracked_only.unwrap_or(false),
            metadata_only: metadata_only.unwrap_or(false),
            file: file.as_ref(),
        }),
        GitQuery::DiffFileContents {
            source_kind,
            change_type,
            base_ref,
            head_ref,
            old_path,
            new_path,
        } => repo.file_contents(
            source_kind,
            change_type,
            base_ref.as_deref(),
            head_ref.as_deref(),
            &old_path,
            &new_path,
        ),
    }
}

struct Repository {
    directory: File,
    root: PathBuf,
    dir: PathBuf,
    git: PathBuf,
    deadline: Instant,
    disabled_filters: Vec<String>,
}

#[derive(Default)]
struct Worktree {
    branch: Option<String>,
    upstream: bool,
    ahead: u64,
    behind: u64,
    dirty: bool,
    untracked: Vec<String>,
}

#[derive(Clone, Debug, Default, Hash)]
struct ChangedFile {
    path: String,
    previous_path: Option<String>,
    change_type: &'static str,
    additions: u64,
    deletions: u64,
}

impl ChangedFile {
    fn wire(&self) -> Value {
        json!({
            "path": self.path, "previousPath": self.previous_path, "changeType": self.change_type,
            "additions": self.additions, "deletions": self.deletions
        })
    }
}

struct Preview<'a> {
    base_ref: Option<&'a str>,
    commit_sha: Option<&'a str>,
    ignore_whitespace: bool,
    source_kind: Option<DiffKind>,
    tracked_only: bool,
    metadata_only: bool,
    file: Option<&'a DiffFile>,
}

impl Repository {
    fn discover(directory: File, root: &Path) -> Result<Option<Self>, String> {
        let dir = PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            directory.as_raw_fd()
        ));
        let dot_git = dir.join(".git");
        // Requiring a private metadata directory also prevents nested projects, symlinked
        // .git directories and Git's external gitdir pointer from widening the selected root.
        match fs::symlink_metadata(&dot_git) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("The Git metadata directory cannot be inspected.".into()),
        }
        for path in [
            "objects",
            "refs",
            "objects/info",
            "objects/pack",
            "commondir",
            "objects/info/alternates",
            "objects/info/http-alternates",
        ] {
            match fs::symlink_metadata(dot_git.join(path)) {
                Ok(metadata)
                    if metadata.is_dir()
                        && matches!(path, "objects" | "refs" | "objects/info" | "objects/pack") => {
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("The checkout uses Git metadata outside the project.".into()),
            }
        }
        let git = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .filter(|path| path.is_absolute())
            .filter_map(|path| path.join("git").canonicalize().ok())
            .find(|git| git.is_file() && !git.starts_with(root))
            .ok_or("Git is not available on this computer.")?;
        let mut repo = Self {
            directory,
            root: root.to_owned(),
            dir,
            git,
            deadline: Instant::now() + Duration::from_secs(8),
            disabled_filters: Vec::new(),
        };
        let actual = repo.required(["rev-parse", "--show-toplevel"])?;
        if without_line_ending(&actual) != repo.root.to_str().unwrap_or_default() {
            return Err("The Git checkout does not match the project folder.".into());
        }

        // Diff and status can consult worktree conversion filters even with external diff
        // drivers disabled. Drop repository-provided filters before inspecting worktree files.
        let (found, keys) = repo.run(
            [
                "config",
                "--null",
                "--name-only",
                "--get-regexp",
                r"^filter\..*\.(clean|process|required)$",
            ],
            None,
        )?;
        if found {
            let names: BTreeSet<_> = keys
                .split_terminator('\0')
                .filter_map(|key| {
                    key.strip_prefix("filter.")?
                        .rsplit_once('.')
                        .map(|(name, _)| name)
                })
                .collect();
            if names.len() > 64
                || names
                    .iter()
                    .any(|name| name.len() > 128 || name.contains('='))
            {
                return Err("The checkout has too many or unsupported Git filters.".into());
            }
            for name in names {
                repo.disabled_filters.extend([
                    format!("filter.{name}.clean="),
                    format!("filter.{name}.process="),
                    format!("filter.{name}.required=false"),
                ]);
            }
        }
        Ok(Some(repo))
    }

    fn run<I, S>(&self, args: I, stdin: Option<&[u8]>) -> Result<(bool, String), String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        if Instant::now() >= self.deadline {
            return Err("Git did not finish in time.".into());
        }
        let mut command = Command::new(&self.git);
        command
            .current_dir(&self.dir)
            .args(["--no-pager", "--no-optional-locks", "--literal-pathspecs"])
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.quotePath=false",
                "-c",
                "diff.color=false",
            ])
            .args(
                self.disabled_filters
                    .iter()
                    .flat_map(|config| ["-c", config]),
            )
            .args(args)
            .env_clear()
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_PROTOCOL_FROM_USER", "0")
            .env("GIT_ALLOW_PROTOCOL", "")
            .env(
                "GIT_CEILING_DIRECTORIES",
                self.root.parent().unwrap_or(&self.root),
            )
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|_| "Git could not be started.".to_owned())?;
        if let Some(input) = stdin
            && let Some(mut stream) = child.stdin.take()
            && stream.write_all(input).is_err()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Git could not read its input.".into());
        }
        let mut stdout = child.stdout.take().expect("Git stdout is piped");
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .by_ref()
                .take((MAX_OUTPUT + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>(bytes)
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < self.deadline => {
                    thread::sleep(Duration::from_millis(5))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err("Git did not finish in time.".to_owned());
                }
            }
        };
        let bytes = reader
            .join()
            .map_err(|_| "Git output was interrupted.")?
            .map_err(|_| "Git output could not be read.")?;
        if bytes.len() > MAX_OUTPUT {
            return Err("This Git result exceeds the size limit.".into());
        }
        let status = status?;
        // Git uses exit 1 for no-match queries and for --no-index when two files differ.
        if !status.success() && status.code() != Some(1) {
            return Err("Git could not inspect this checkout.".into());
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Ok((status.success(), text))
    }

    fn required<I, S>(&self, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let (ok, output) = self.run(args, None)?;
        ok.then_some(output)
            .ok_or_else(|| "Git could not inspect this checkout.".into())
    }

    fn revision(&self, name: &str) -> Result<Option<String>, String> {
        if name.is_empty() || name.len() > 512 || name.starts_with('-') || name.contains('\0') {
            return Err("Invalid Git reference.".into());
        }
        let (found, output) = self.run(
            [
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{name}^{{commit}}"),
            ],
            None,
        )?;
        Ok(found.then(|| output.trim().to_owned()))
    }

    fn default_ref(&self, status: &Worktree) -> Result<Option<String>, String> {
        let name = self.required([
            "for-each-ref",
            "--format=%(symref:short)",
            "refs/remotes/origin/HEAD",
        ])?;
        if !name.trim().is_empty() {
            return Ok(Some(name.trim().to_owned()));
        }
        for name in ["main", "master", "trunk"] {
            if status.branch.as_deref() == Some(name)
                || self.revision(&format!("refs/heads/{name}"))?.is_some()
            {
                return Ok(Some(name.into()));
            }
        }
        Ok(status.branch.clone())
    }

    fn merge_base(&self, head: &str, reference: &str) -> Result<Option<String>, String> {
        let Some(base) = self.revision(reference)? else {
            return Err("The comparison branch no longer exists.".into());
        };
        let (found, sha) = self.run(["merge-base", head, &base], None)?;
        if !found {
            return Err("The comparison branch has no common ancestor with this checkout.".into());
        }
        Ok(Some(sha.trim().to_owned()))
    }

    fn worktree(&self) -> Result<Worktree, String> {
        let output = self.required([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ])?;
        let mut fields = output.split_terminator('\0');
        let mut status = Worktree::default();
        while let Some(field) = fields.next() {
            if let Some(branch) = field.strip_prefix("# branch.head ") {
                if branch != "(detached)" {
                    status.branch = Some(branch.to_owned());
                }
            } else if field.starts_with("# branch.upstream ") {
                status.upstream = true;
            } else if let Some(divergence) = field.strip_prefix("# branch.ab ") {
                if let Some((ahead, behind)) = divergence.split_once(' ') {
                    status.ahead = ahead.trim_start_matches('+').parse().unwrap_or(0);
                    status.behind = behind.trim_start_matches('-').parse().unwrap_or(0);
                }
            } else if let Some(path) = field.strip_prefix("? ") {
                status.dirty = true;
                if check_path(path).is_ok() {
                    status.untracked.push(path.to_owned());
                }
            } else if field.starts_with("2 ") {
                status.dirty = true;
                fields.next(); // original path of a rename, which can itself begin with "# " or "? "
            } else if field.starts_with("1 ") || field.starts_with("u ") {
                status.dirty = true;
            }
        }
        Ok(status)
    }

    fn has_origin(&self) -> Result<bool, String> {
        Ok(self
            .required(["remote"])?
            .lines()
            .any(|name| name == "origin"))
    }

    fn status(&self) -> Result<Value, String> {
        let status = self.worktree()?;
        let head = self.revision("HEAD")?;
        let untracked = self.untracked(&status, None);
        let (files, _) = self.changed_files(head.as_deref(), None, false, &[], untracked)?;
        let default = self.default_ref(&status)?;
        let is_default = default
            .as_deref()
            .zip(status.branch.as_deref())
            .is_some_and(|(default, current)| {
                default == current || default.strip_prefix("origin/") == Some(current)
            });
        Ok(json!({
            "isRepo": true, "hasPrimaryRemote": self.has_origin()?, "isDefaultRef": is_default,
            "refName": status.branch, "hasWorkingTreeChanges": status.dirty,
            "workingTree": {
                "rootPath": self.root, "files": files.iter().map(|file|
                    json!({ "path": file.path, "insertions": file.additions, "deletions": file.deletions })
                ).collect::<Vec<_>>(),
                "insertions": files.iter().map(|file| file.additions).sum::<u64>(),
                "deletions": files.iter().map(|file| file.deletions).sum::<u64>()
            },
            "hasUpstream": status.upstream, "aheadCount": status.ahead, "behindCount": status.behind, "pr": null
        }))
    }

    fn refs(
        &self,
        query: Option<&str>,
        cursor: usize,
        limit: usize,
        kind: Option<RefKind>,
    ) -> Result<Value, String> {
        let status = self.worktree()?;
        let default = self.default_ref(&status)?;
        let refs = self.required([
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(symref)",
            "refs/heads",
            "refs/remotes",
        ])?;
        let pattern = query.unwrap_or_default().to_lowercase();
        let mut parsed = Vec::new();
        for line in refs.lines() {
            let fields: Vec<_> = line.split('\0').collect();
            let [name, upstream, symbolic] = fields.as_slice() else {
                continue;
            };
            if !symbolic.is_empty() {
                continue;
            }
            let (name, remote) = if let Some(local) = name.strip_prefix("refs/heads/") {
                (local, false)
            } else if let Some(remote) = name.strip_prefix("refs/remotes/") {
                (remote, true)
            } else {
                continue;
            };
            if name.trim().is_empty()
                || !name.to_lowercase().contains(&pattern)
                || (kind == Some(RefKind::Local) && remote)
                || (kind == Some(RefKind::Remote) && !remote)
            {
                continue;
            }
            let current = !remote && status.branch.as_deref() == Some(name);
            let mut item = json!({
                "name": name, "isRemote": remote, "current": current, "isDefault": default.as_deref() == Some(name),
                "worktreePath": if current { self.root.to_str() } else { None }
            });
            if remote && let Some((remote_name, _)) = name.split_once('/') {
                item["remoteName"] = json!(remote_name);
            }
            if !upstream.is_empty() {
                item["upstreamRef"] = json!(upstream);
            }
            parsed.push(item);
        }
        let limit = limit.clamp(1, 200);
        let next = cursor.saturating_add(limit);
        Ok(json!({
            "isRepo": true, "isAtRepositoryRoot": true, "hasPrimaryRemote": self.has_origin()?,
            "nextCursor": (next < parsed.len()).then_some(next), "totalCount": parsed.len(),
            "refs": parsed.iter().skip(cursor).take(limit).collect::<Vec<_>>()
        }))
    }

    fn commits(&self, base_ref: Option<&str>) -> Result<Value, String> {
        let status = self.worktree()?;
        let Some(head) = self.revision("HEAD")? else {
            return Ok(json!({ "commits": [], "truncated": false }));
        };
        let default = self.default_ref(&status)?;
        let comparison = base_ref.or(default.as_deref());
        let base = comparison
            .map(|name| self.merge_base(&head, name))
            .transpose()?
            .flatten();
        let range = match base {
            Some(base) => format!("{base}..{head}"),
            None => head,
        };
        let limit = format!("--max-count={}", MAX_COMMITS + 1);
        let output = self.required(["log", &limit, "--format=%H%x00%s%x00%cI", &range, "--"])?;
        let commits: Vec<_> = output.lines().filter_map(|line| {
            let mut fields = line.splitn(3, '\0');
            Some(json!({ "sha": fields.next()?, "subject": fields.next()?, "committedAt": fields.next()? }))
        }).collect();
        Ok(
            json!({ "commits": commits.iter().take(MAX_COMMITS).collect::<Vec<_>>(), "truncated": commits.len() > MAX_COMMITS }),
        )
    }

    fn untracked<'a>(&self, status: &'a Worktree, file: Option<&DiffFile>) -> Vec<&'a str> {
        status
            .untracked
            .iter()
            .map(String::as_str)
            .filter(|path| file.is_none_or(|file| file.path == *path))
            .take(MAX_CHANGED_FILES + 1)
            .collect()
    }

    fn empty_tree(&self) -> Result<String, String> {
        let (ok, sha) = self.run(["hash-object", "-t", "tree", "--stdin"], Some(b""))?;
        ok.then(|| sha.trim().to_owned())
            .ok_or_else(|| "Git could not read the empty tree.".into())
    }

    fn diff_args<'a>(
        &self,
        base: Option<&'a str>,
        head: Option<&'a str>,
        ignore_space: bool,
    ) -> Result<Vec<String>, String> {
        let mut args = vec!["diff".to_owned()];
        args.extend(
            [
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=all",
                "--find-renames",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        if ignore_space {
            args.push("--ignore-all-space".into());
        }
        match (base, head) {
            (Some(base), Some(head)) => args.extend([base.to_owned(), head.to_owned()]),
            (Some(base), None) => args.push(base.to_owned()),
            (None, Some(head)) => args.extend([self.empty_tree()?, head.to_owned()]),
            (None, None) => args.push(self.empty_tree()?),
        }
        Ok(args)
    }

    fn changed_files(
        &self,
        base: Option<&str>,
        head: Option<&str>,
        ignore_space: bool,
        paths: &[&str],
        untracked: Vec<&str>,
    ) -> Result<(Vec<ChangedFile>, bool), String> {
        let args = self.diff_args(base, head, ignore_space)?;
        let read = |flags: &[&str]| {
            self.required(
                args.iter()
                    .map(String::as_str)
                    .chain(flags.iter().copied())
                    .chain(["--"])
                    .chain(paths.iter().copied()),
            )
        };
        let mut by_path = parse_numstat(&read(&["--numstat", "-z"])?);
        let kinds = read(&["--name-status", "-z"])?;
        let mut fields = kinds.split_terminator('\0');
        while let Some(kind) = fields.next() {
            let Some(path) = fields.next() else {
                break;
            };
            let path = if kind.starts_with('R') || kind.starts_with('C') {
                let Some(path) = fields.next() else {
                    break;
                };
                path
            } else {
                path
            };
            if let Some(file) = by_path.get_mut(path) {
                file.change_type = match kind.as_bytes().first() {
                    Some(b'A') => "new",
                    Some(b'D') => "deleted",
                    Some(b'R') if kind == "R100" => "rename-pure",
                    Some(b'R') => "rename-changed",
                    _ => "change",
                };
            }
        }
        let truncated = by_path.len() + untracked.len() > MAX_CHANGED_FILES;
        for path in untracked.into_iter().take(MAX_CHANGED_FILES) {
            if by_path.len() >= MAX_CHANGED_FILES {
                break;
            }
            let additions = self
                .text_file(path)
                .ok()
                .map_or(0, |text| text.lines().count() as u64);
            by_path.entry(path.into()).or_insert_with(|| ChangedFile {
                path: path.into(),
                change_type: "new",
                additions,
                ..Default::default()
            });
        }
        Ok((
            by_path.into_values().take(MAX_CHANGED_FILES).collect(),
            truncated,
        ))
    }

    fn patch(
        &self,
        base: Option<&str>,
        head: Option<&str>,
        ignore_space: bool,
        paths: &[&str],
        untracked: &[&str],
    ) -> Result<(String, bool), String> {
        let mut patch = self.required(
            self.diff_args(base, head, ignore_space)?
                .iter()
                .map(String::as_str)
                .chain(["--patch", "--"])
                .chain(paths.iter().copied()),
        )?;
        for path in untracked {
            if patch.len() > MAX_OUTPUT / 2 {
                return Ok((patch, true));
            }
            // --no-index shows an untracked text file with /dev/null as the left side.
            // Supply bytes only from a scoped regular file, generating the standard new-file hunk.
            if let Ok(text) = self.text_file(path) {
                patch.push_str(&new_file_patch(path, &text));
            }
        }
        Ok((patch, false))
    }

    fn preview(&self, options: Preview<'_>) -> Result<Value, String> {
        let status = self.worktree()?;
        let head = self.revision("HEAD")?;
        let default = self.default_ref(&status)?;
        let mut sources = Vec::new();
        let mut incomplete = false;
        let kinds = if let Some(file) = options.file {
            vec![file.source_kind]
        } else if options.commit_sha.is_some() {
            vec![DiffKind::Commit]
        } else if let Some(kind) = options.source_kind {
            vec![kind]
        } else {
            vec![DiffKind::WorkingTree, DiffKind::BranchRange]
        };
        for kind in kinds {
            let mut subject = None;
            let (base, commit, label_base, label_head, title, id) = match kind {
                DiffKind::WorkingTree => (
                    head.clone(),
                    None,
                    head.as_ref().map(|_| "HEAD".to_owned()),
                    None,
                    "Uncommitted".to_owned(),
                    "working-tree".to_owned(),
                ),
                DiffKind::BranchRange => {
                    let label_base = options
                        .base_ref
                        .map(str::to_owned)
                        .or_else(|| default.clone());
                    let base = label_base
                        .as_deref()
                        .zip(head.as_deref())
                        .map(|(reference, head)| self.merge_base(head, reference))
                        .transpose()?
                        .flatten();
                    let title = label_base
                        .as_ref()
                        .map_or_else(|| "Changes".into(), |base| format!("Changes vs {base}"));
                    (
                        base,
                        None,
                        label_base,
                        status.branch.clone().or_else(|| head.clone()),
                        title,
                        "branch-range".into(),
                    )
                }
                DiffKind::Commit => {
                    let commit = self
                        .revision(options.commit_sha.ok_or("A commit is required.")?)?
                        .ok_or("This commit no longer exists.")?;
                    let parent = self.revision(&format!("{commit}^"))?;
                    let summary =
                        self.required(["show", "--no-patch", "--format=%s", &commit, "--"])?;
                    subject = (!summary.trim().is_empty()).then(|| summary.trim().to_owned());
                    (
                        parent.clone(),
                        Some(commit.clone()),
                        parent,
                        Some(commit.clone()),
                        format!("Commit {}", &commit[..7]),
                        commit,
                    )
                }
            };
            let mut paths = Vec::new();
            if let Some(file) = options.file {
                check_path(&file.path)?;
                paths.push(file.path.as_str());
                if let Some(previous) = &file.previous_path {
                    check_path(previous)?;
                    paths.push(previous);
                }
            }
            let untracked = if kind != DiffKind::Commit && !options.tracked_only {
                self.untracked(&status, options.file)
            } else {
                Vec::new()
            };
            let deferred = options.metadata_only && options.file.is_none();
            let (files, truncated) = self.changed_files(
                base.as_deref(),
                commit.as_deref(),
                options.ignore_whitespace,
                &paths,
                untracked.clone(),
            )?;
            let (patch, patch_truncated) = if deferred {
                (String::new(), false)
            } else {
                self.patch(
                    base.as_deref(),
                    commit.as_deref(),
                    options.ignore_whitespace,
                    &paths,
                    &untracked,
                )?
            };
            let mut hasher = DefaultHasher::new();
            base.hash(&mut hasher);
            commit.hash(&mut hasher);
            options.ignore_whitespace.hash(&mut hasher);
            files.hash(&mut hasher);
            patch.hash(&mut hasher);
            for file in &files {
                if let Ok(metadata) = fs::symlink_metadata(self.dir.join(&file.path)) {
                    metadata.modified().ok().hash(&mut hasher);
                    metadata.len().hash(&mut hasher);
                }
            }
            incomplete |= truncated;
            let mut source = json!({
                "kind": match kind { DiffKind::WorkingTree => "working-tree", DiffKind::BranchRange => "branch-range", DiffKind::Commit => "commit" },
                "id": id, "title": title, "baseRef": label_base, "headRef": label_head,
                "diff": patch, "diffHash": format!("{:016x}", hasher.finish()), "truncated": truncated || patch_truncated,
                "patchesDeferred": deferred, "files": files.iter().map(ChangedFile::wire).collect::<Vec<_>>()
            });
            if let Some(subject) = subject {
                source["commitSubject"] = json!(subject);
            }
            sources.push(source);
        }
        Ok(json!({ "cwd": self.root, "sources": sources, "incomplete": incomplete }))
    }

    fn object_text(&self, commit: &str, path: &str) -> Result<String, String> {
        let selector = format!("{commit}:{path}");
        let size = self.required(["cat-file", "-s", &selector])?;
        if size.trim().parse::<usize>().unwrap_or(usize::MAX) > MAX_FILE_BYTES {
            return Err("This file is too large to expand.".into());
        }
        let text = self.required(["cat-file", "blob", &selector])?;
        if text.contains('\0') || text.contains(char::REPLACEMENT_CHARACTER) {
            return Err("This file cannot be expanded as text.".into());
        }
        Ok(text)
    }

    fn text_file(&self, path: &str) -> Result<String, String> {
        let relative = check_path(path)?;
        let mut file = crate::project_files::open_project_file(&self.directory, relative)
            .map_err(|_| "This file cannot be opened safely.")?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "This file could not be read.")?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err("This file is too large to expand.".into());
        }
        if bytes.contains(&0) {
            return Err("This file cannot be expanded as text.".into());
        }
        String::from_utf8(bytes).map_err(|_| "This file cannot be expanded as text.".into())
    }

    fn file_contents(
        &self,
        kind: DiffKind,
        change: ChangeType,
        base_ref: Option<&str>,
        head_ref: Option<&str>,
        old_path: &str,
        new_path: &str,
    ) -> Result<Value, String> {
        check_path(old_path)?;
        check_path(new_path)?;
        let head = self.revision(head_ref.unwrap_or("HEAD"))?;
        let base = match (kind, base_ref, head.as_deref()) {
            (DiffKind::BranchRange, Some(base), Some(head)) => self.merge_base(head, base)?,
            (_, Some(base), _) => self.revision(base)?,
            _ => None,
        };
        let old = match (change, base) {
            (ChangeType::New, _) | (_, None) => String::new(),
            (_, Some(base)) => self.object_text(&base, old_path)?,
        };
        let new = match (kind, change, head) {
            (_, ChangeType::Deleted, _) => String::new(),
            (DiffKind::Commit, _, Some(head)) => self.object_text(&head, new_path)?,
            (DiffKind::Commit, _, None) => return Err("This commit no longer exists.".into()),
            _ => self.text_file(new_path)?,
        };
        Ok(json!({ "oldContents": old, "newContents": new }))
    }
}

fn without_line_ending(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

fn check_path(path: &str) -> Result<&Path, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\\', '\0', '\u{fffd}'])
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(name) if name != ".git"))
    {
        return Err("The file path is outside the Git project.".into());
    }
    Ok(relative)
}

fn parse_numstat(output: &str) -> BTreeMap<String, ChangedFile> {
    let mut files = BTreeMap::new();
    let mut fields = output.split_terminator('\0');
    while let Some(field) = fields.next() {
        let mut columns = field.splitn(3, '\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        let (path, previous_path) = if path.is_empty() {
            let (Some(old), Some(new)) = (fields.next(), fields.next()) else {
                break;
            };
            (new, Some(old.to_owned()))
        } else {
            (path, None)
        };
        if check_path(path).is_err()
            || previous_path
                .as_deref()
                .is_some_and(|path| check_path(path).is_err())
        {
            continue;
        }
        files.insert(
            path.to_owned(),
            ChangedFile {
                path: path.to_owned(),
                previous_path,
                change_type: "change",
                additions: additions.parse().unwrap_or(0),
                deletions: deletions.parse().unwrap_or(0),
            },
        );
    }
    files
}

fn new_file_patch(path: &str, text: &str) -> String {
    fn patch_path(prefix: char, path: &str) -> String {
        let full = format!("{prefix}/{path}");
        if full.contains(['\n', '\r', '\t', '"']) {
            format!("{full:?}") // Git's quoted-path escapes for line separators and double quotes.
        } else {
            full
        }
    }
    let a = patch_path('a', path);
    let b = patch_path('b', path);
    let mut patch = format!("diff --git {a} {b}\nnew file mode 100644\n");
    if text.is_empty() {
        return patch;
    }
    let count = text.split_terminator('\n').count();
    patch.push_str(&format!("--- /dev/null\n+++ {b}\n@@ -0,0 +1,{count} @@\n"));
    for line in text.split_terminator('\n') {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    if !text.ends_with('\n') {
        patch.push_str("\\ No newline at end of file\n");
    }
    patch
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Checkout(PathBuf);

    impl Checkout {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "hoh-git-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }

        fn git(&self, args: &[&str]) {
            let output = Command::new("/usr/bin/git")
                .current_dir(&self.0)
                .env_clear()
                .args([
                    "-c",
                    "user.email=fixture@localhost",
                    "-c",
                    "user.name=HoH Fixture",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn init(&self) {
            self.git(&["init", "-b", "main"]);
            fs::write(self.0.join("tracked.txt"), "first\n").unwrap();
            self.git(&["add", "tracked.txt"]);
            self.git(&["commit", "-m", "Initial"]);
        }

        fn read(&self, input: Value) -> Result<Value, String> {
            query(
                File::open(&self.0).unwrap(),
                &self.0,
                serde_json::from_value(input).unwrap(),
            )
        }
    }

    impl Drop for Checkout {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reports_refs_patches_untracked_files_and_file_context() {
        let checkout = Checkout::new();
        checkout.init();
        checkout.git(&["switch", "-c", "topic"]);
        fs::write(checkout.0.join("tracked.txt"), "first\ncommitted\n").unwrap();
        checkout.git(&["commit", "-am", "Topic work"]);
        fs::write(checkout.0.join("tracked.txt"), "first\ncommitted\nplus\n").unwrap();
        fs::write(checkout.0.join("new file.md"), "untracked").unwrap();

        let status = checkout.read(json!({"action":"status"})).unwrap();
        assert_eq!(status["refName"], "topic");
        assert_eq!(status["hasWorkingTreeChanges"], true);
        assert_eq!(status["workingTree"]["insertions"], 2);
        let refs = checkout
            .read(json!({"action":"listRefs","query":"opi","limit":10}))
            .unwrap();
        assert_eq!(refs["refs"][0]["name"], "topic");
        assert_eq!(refs["totalCount"], 1);
        let commits = checkout.read(json!({"action":"branchCommits"})).unwrap();
        assert_eq!(commits["commits"][0]["subject"], "Topic work");
        let commit = commits["commits"][0]["sha"].as_str().unwrap();
        let committed = checkout
            .read(json!({"action":"diffPreview","commitSha":commit}))
            .unwrap();
        assert!(
            committed["sources"][0]["diff"]
                .as_str()
                .unwrap()
                .contains("+committed")
        );

        let preview = checkout
            .read(json!({"action":"diffPreview","sourceKind":"branch-range","metadataOnly":true}))
            .unwrap();
        let source = &preview["sources"][0];
        assert_eq!(source["baseRef"], "main");
        assert_eq!(source["patchesDeferred"], true);
        assert_eq!(source["files"][0]["path"], "new file.md");
        let patch = checkout
            .read(json!({
                "action":"diffPreview","sourceKind":"working-tree",
                "file":{"path":"new file.md","previousPath":null,"sourceKind":"working-tree"}
            }))
            .unwrap();
        assert!(
            patch["sources"][0]["diff"]
                .as_str()
                .unwrap()
                .contains("+untracked\n\\ No newline")
        );
        let file = checkout
            .read(json!({
                "action":"diffFileContents","sourceKind":"branch-range","changeType":"change",
                "baseRef":"main","headRef":"topic","oldPath":"tracked.txt","newPath":"tracked.txt"
            }))
            .unwrap();
        assert_eq!(file["oldContents"], "first\n");
        assert_eq!(file["newContents"], "first\ncommitted\nplus\n");
    }

    #[test]
    fn a_project_never_uses_a_parent_or_external_git_directory() {
        let checkout = Checkout::new();
        checkout.init();
        let nested = checkout.0.join("nested");
        fs::create_dir(&nested).unwrap();
        let status = || query(File::open(&nested).unwrap(), &nested, GitQuery::Status);
        assert_eq!(status().unwrap()["isRepo"], false);
        fs::write(
            nested.join(".git"),
            format!("gitdir: {}\n", checkout.0.join(".git").display()),
        )
        .unwrap();
        assert_eq!(status().unwrap()["isRepo"], false);

        fs::write(
            checkout.0.join(".git/objects/info/alternates"),
            "/other/repository/objects\n",
        )
        .unwrap();
        assert!(
            checkout
                .read(json!({"action":"status"}))
                .unwrap_err()
                .contains("outside")
        );
    }

    #[test]
    fn status_and_diff_do_not_start_configured_git_helpers() {
        let checkout = Checkout::new();
        checkout.init();
        let marker = checkout.0.join("helper-ran");
        checkout.git(&[
            "config",
            "core.fsmonitor",
            &format!("touch {}", marker.display()),
        ]);
        checkout.git(&[
            "config",
            "filter.probe.clean",
            &format!("touch {}; cat", marker.display()),
        ]);
        checkout.git(&["config", "filter.probe.required", "true"]);
        checkout.git(&[
            "config",
            "diff.probe.command",
            &format!("touch {}", marker.display()),
        ]);
        fs::write(
            checkout.0.join(".gitattributes"),
            "tracked.txt filter=probe diff=probe\n",
        )
        .unwrap();
        fs::write(checkout.0.join("tracked.txt"), "edited\n").unwrap();

        let status = checkout.read(json!({"action":"status"})).unwrap();
        assert_eq!(status["hasWorkingTreeChanges"], true);
        let diff = checkout
            .read(json!({"action":"diffPreview","sourceKind":"working-tree"}))
            .unwrap();
        assert!(
            diff["sources"][0]["diff"]
                .as_str()
                .unwrap()
                .contains("+edited")
        );
        assert!(!marker.exists());
        let outside = Checkout::new();
        let secret = outside.0.join("secret.txt");
        fs::write(&secret, "outside").unwrap();
        std::os::unix::fs::symlink(&secret, checkout.0.join("link.txt")).unwrap();
        assert!(
            checkout
                .read(json!({
                    "action":"diffFileContents","sourceKind":"working-tree","changeType":"new",
                    "baseRef":null,"headRef":null,"oldPath":"link.txt","newPath":"link.txt"
                }))
                .is_err()
        );
    }

    #[test]
    fn an_unborn_checkout_can_show_staged_and_untracked_changes() {
        let checkout = Checkout::new();
        checkout.git(&["init", "-b", "main"]);
        fs::write(checkout.0.join("staged.txt"), "staged\n").unwrap();
        checkout.git(&["add", "staged.txt"]);
        fs::write(checkout.0.join("untracked.txt"), "untracked\n").unwrap();
        let status = checkout.read(json!({"action":"status"})).unwrap();
        assert_eq!(status["workingTree"]["files"].as_array().unwrap().len(), 2);
    }
}
