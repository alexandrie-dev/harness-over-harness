//! Instruction-file discovery and bounded, abort-aware reads.

use crate::{
    config::{Config, ResolvedConfig, ResolvedDiscoveryConfig, resolve_discovery_config},
    digest::trimmed_instruction_digest,
    render::{
        AGENTS_GLOBAL_DIRECTORY, RenderedAgentInstructions, USER_GLOBAL_DIRECTORIES,
        USER_GLOBAL_DIRECTORY, USER_GLOBAL_FILE, decode_scope_key, escape_project_display_path,
        instruction_candidate_group, render_agent_instruction_set, scope_for_display_path,
    },
};
use futures_util::StreamExt;
use hoh_error::{BoxError, HarnessError};
use hoh_fs::{FileSystem, FsInfo, FsResolveOptions, FsTarget, FsTextStream, FsType, FsVersion};
use hoh_home_paths::{agents_home_display, dsh_home_display};
use hoh_timeout::AbortSignal;
use hoh_util_workspace_path::node_path;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionFile {
    pub absolute_path: String,
    pub display_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedInstructionFile {
    pub absolute_path: String,
    pub display_path: String,
    pub content: String,
    pub version: Option<FsVersion>,
}

impl From<&LoadedInstructionFile> for InstructionFile {
    fn from(value: &LoadedInstructionFile) -> Self {
        Self {
            absolute_path: value.absolute_path.clone(),
            display_path: value.display_path.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct DiscoveredInstructionFile {
    file: InstructionFile,
    target: Option<FsTarget>,
    size: Option<u64>,
    version: Option<FsVersion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbedInstructionFile {
    pub absolute_path: String,
    pub display_path: String,
    pub target: FsTarget,
    pub version: FsVersion,
    pub size: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct DiscoverOptions {
    pub cwd: String,
    pub dsh_home: Option<String>,
    pub agents_home: Option<String>,
    pub project_root_markers: Option<Vec<String>>,
    pub instruction_file_candidates: Option<Vec<String>>,
    pub local_instruction_file_candidates: Option<Vec<String>>,
    pub project_root: Option<String>,
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub discovery: DiscoverOptions,
    pub max_bytes: f64,
    pub max_source_bytes: u64,
    pub replace_previous_baseline: bool,
}

#[derive(Clone, Debug)]
pub struct RenderedInstructionSet {
    pub rendered: RenderedAgentInstructions,
    pub observed: Vec<LoadedInstructionFile>,
    pub deduped: Vec<LoadedInstructionFile>,
    pub included: Vec<LoadedInstructionFile>,
}

#[derive(Clone, Debug)]
pub enum ScopeInstructionProbe {
    Present(ProbedInstructionFile),
    Absent,
    Unavailable,
}

enum StatProbe {
    Present {
        target: Option<FsTarget>,
        size: Option<u64>,
        version: Option<FsVersion>,
    },
    Absent,
    Unavailable,
}

fn check_signal(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    Ok(())
}

fn fs_options(signal: Option<&AbortSignal>) -> FsResolveOptions {
    FsResolveOptions {
        cwd: None,
        signal: signal.cloned(),
    }
}

async fn stat_file(
    path: &str,
    file_system: Option<&dyn FileSystem>,
    signal: Option<&AbortSignal>,
) -> Result<StatProbe, BoxError> {
    if let Some(fs) = file_system {
        let result = async {
            let target = fs.resolve(path, fs_options(signal)).await?;
            check_signal(signal)?;
            let info = fs.stat(&target, signal.cloned()).await?;
            check_signal(signal)?;
            Ok::<_, BoxError>((target, info))
        }
        .await;
        return classify_provider_stat_result(result, signal);
    }
    check_signal(signal)?;
    Ok(match tokio::fs::metadata(path).await {
        Ok(info) if info.is_file() => StatProbe::Present {
            target: None,
            size: Some(info.len()),
            version: None,
        },
        Ok(_) => StatProbe::Absent,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            StatProbe::Absent
        }
        Err(_) => {
            check_signal(signal)?;
            StatProbe::Unavailable
        }
    })
}

fn classify_provider_stat_result(
    result: Result<(FsTarget, Option<FsInfo>), BoxError>,
    signal: Option<&AbortSignal>,
) -> Result<StatProbe, BoxError> {
    Ok(match result {
        Ok((target, Some(info))) if info.r#type == FsType::File => StatProbe::Present {
            target: Some(target),
            size: info.size,
            version: Some(info.version),
        },
        Ok(_) => StatProbe::Absent,
        Err(_) => {
            check_signal(signal)?;
            StatProbe::Unavailable
        }
    })
}

async fn marker_exists(
    path: &str,
    file_system: Option<&dyn FileSystem>,
    signal: Option<&AbortSignal>,
) -> Result<bool, BoxError> {
    if let Some(fs) = file_system {
        let result = async {
            let target = fs.resolve(path, fs_options(signal)).await?;
            Ok::<_, BoxError>(fs.stat(&target, signal.cloned()).await?.is_some())
        }
        .await;
        return classify_provider_marker_result(result, signal);
    }
    check_signal(signal)?;
    match tokio::fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(error) => {
            check_signal(signal)?;
            Err(Box::new(error))
        }
    }
}

fn classify_provider_marker_result(
    result: Result<bool, BoxError>,
    signal: Option<&AbortSignal>,
) -> Result<bool, BoxError> {
    match result {
        Ok(present) => Ok(present),
        Err(error)
            if HarnessError::of(error.as_ref())
                .is_some_and(|error| error.code() == "FS_NOT_FOUND") =>
        {
            check_signal(signal)?;
            Ok(false)
        }
        Err(error) => {
            check_signal(signal)?;
            Err(error)
        }
    }
}

pub async fn find_project_root(
    cwd: &str,
    markers: &[String],
    file_system: Option<&dyn FileSystem>,
    signal: Option<&AbortSignal>,
) -> Result<String, BoxError> {
    let original = node_path::resolve("/", &[cwd]);
    let mut current = original.clone();
    loop {
        for marker in markers {
            let path = node_path::join(&[&current, marker]);
            if marker_exists(&path, file_system, signal).await? {
                return Ok(current);
            }
        }
        let parent = node_path::dirname(&current);
        if parent == current {
            return Ok(original);
        }
        current = parent;
    }
}

pub fn ancestor_chain(root: &str, cwd: &str) -> Vec<String> {
    let root = node_path::resolve("/", &[root]);
    let mut current = node_path::resolve("/", &[cwd]);
    let mut chain = Vec::new();
    while current != root {
        chain.push(current.clone());
        let parent = node_path::dirname(&current);
        if parent == current {
            break;
        }
        current = parent;
    }
    chain.push(root);
    chain.reverse();
    chain
}

pub fn descendant_dirs_between(root: &str, touched_path: &str) -> Vec<String> {
    let root = node_path::resolve("/", &[root]);
    let target = if node_path::is_absolute(touched_path) {
        node_path::resolve("/", &[touched_path])
    } else {
        node_path::resolve(&root, &[touched_path])
    };
    let target_dir = node_path::dirname(&target);
    let relative = node_path::relative("/", &root, &target_dir);
    if relative.is_empty() || relative == ".." || relative.starts_with("../") {
        return vec![];
    }
    ancestor_chain(&root, &target_dir)
        .into_iter()
        .skip(1)
        .collect()
}

pub fn relative_display(root: &str, path: &str) -> String {
    escape_project_display_path(&node_path::relative("/", root, path))
}

fn discovery_config(options: &DiscoverOptions) -> Result<ResolvedDiscoveryConfig, BoxError> {
    let config = Config {
        project_root_markers: options
            .project_root_markers
            .clone()
            .unwrap_or_else(|| vec![".git".into()]),
        max_bytes: 1.0,
        max_source_bytes: 1,
        instruction_file_candidates: options
            .instruction_file_candidates
            .clone()
            .unwrap_or_else(|| vec!["AGENTS.md".into(), "CLAUDE.md".into()]),
        local_instruction_file_candidates: options
            .local_instruction_file_candidates
            .clone()
            .unwrap_or_else(|| vec!["AGENTS.local.md".into(), "CLAUDE.local.md".into()]),
    };
    resolve_discovery_config(
        &config,
        options.dsh_home.as_deref(),
        options.agents_home.as_deref(),
    )
}

async fn discover(
    options: &DiscoverOptions,
    file_system: Option<&dyn FileSystem>,
) -> Result<Vec<DiscoveredInstructionFile>, BoxError> {
    let config = discovery_config(options)?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |file: DiscoveredInstructionFile| {
        if seen.insert(file.file.absolute_path.clone()) {
            files.push(file);
        }
    };
    for directory in USER_GLOBAL_DIRECTORIES {
        let (home, display) = if directory == USER_GLOBAL_DIRECTORY {
            (
                config.dsh_home.as_str(),
                dsh_home_display(Path::new(&config.dsh_home))?,
            )
        } else {
            (
                config.agents_home.as_str(),
                agents_home_display(Path::new(&config.agents_home))?,
            )
        };
        let path = node_path::join(&[home, USER_GLOBAL_FILE]);
        if let StatProbe::Present {
            target,
            size,
            version,
        } = stat_file(&path, file_system, options.signal.as_ref()).await?
        {
            add(DiscoveredInstructionFile {
                file: InstructionFile {
                    absolute_path: path,
                    display_path: format!("{display}/{USER_GLOBAL_FILE}"),
                },
                target,
                size,
                version,
            });
        }
    }
    let cwd = node_path::resolve("/", &[&options.cwd]);
    let root = match &options.project_root {
        Some(root) => root.clone(),
        None => {
            find_project_root(
                &cwd,
                &config.project_root_markers,
                file_system,
                options.signal.as_ref(),
            )
            .await?
        }
    };
    for directory in ancestor_chain(&root, &cwd) {
        for candidates in [
            &config.instruction_file_candidates,
            &config.local_instruction_file_candidates,
        ] {
            for candidate in candidates {
                let path = node_path::join(&[&directory, candidate]);
                if let StatProbe::Present {
                    target,
                    size,
                    version,
                } = stat_file(&path, file_system, options.signal.as_ref()).await?
                {
                    add(DiscoveredInstructionFile {
                        file: InstructionFile {
                            absolute_path: path.clone(),
                            display_path: relative_display(&root, &path),
                        },
                        target,
                        size,
                        version,
                    });
                }
            }
        }
    }
    Ok(files)
}

pub async fn discover_baseline_instruction_files(
    options: &DiscoverOptions,
    file_system: Option<&dyn FileSystem>,
) -> Result<Vec<InstructionFile>, BoxError> {
    Ok(discover(options, file_system)
        .await?
        .into_iter()
        .map(|file| file.file)
        .collect())
}

async fn read_bounded(
    file: &DiscoveredInstructionFile,
    max_source_bytes: u64,
    file_system: Option<&dyn FileSystem>,
    signal: Option<&AbortSignal>,
) -> Result<Option<String>, BoxError> {
    check_signal(signal)?;
    if file.size.is_some_and(|size| size > max_source_bytes) {
        return Ok(None);
    }
    if let (Some(fs), Some(target)) = (file_system, &file.target) {
        let stream = match fs.stream_text(target, signal.cloned()).await {
            Ok(stream) => stream,
            Err(_) => {
                check_signal(signal)?;
                return Ok(None);
            }
        };
        return read_bounded_stream(stream, max_source_bytes, signal).await;
    }
    let handle = match tokio::fs::File::open(&file.file.absolute_path).await {
        Ok(handle) => handle,
        Err(_) => return Ok(None),
    };
    let mut bytes = Vec::new();
    if handle
        .take(max_source_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .is_err()
    {
        return Ok(None);
    }
    check_signal(signal)?;
    if bytes.len() as u64 > max_source_bytes {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

async fn read_bounded_stream(
    mut stream: FsTextStream,
    max_source_bytes: u64,
    signal: Option<&AbortSignal>,
) -> Result<Option<String>, BoxError> {
    let mut output = String::new();
    let mut bytes = 0_u64;
    while let Some(chunk) = stream.next().await {
        check_signal(signal)?;
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => return Ok(None),
        };
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > max_source_bytes {
            return Ok(None);
        }
        output.push_str(&chunk);
    }
    Ok(Some(output))
}

pub fn dedup_instruction_files_by_candidate_group(
    files: &[LoadedInstructionFile],
) -> Vec<LoadedInstructionFile> {
    let mut groups: HashMap<String, HashSet<String>> = HashMap::new();
    let mut kept = Vec::new();
    for file in files {
        let group = instruction_candidate_group(&scope_for_display_path(&file.display_path));
        let digest = trimmed_instruction_digest(&file.content);
        if groups.entry(group).or_default().insert(digest) {
            kept.push(file.clone());
        }
    }
    kept
}

pub async fn load_baseline_instruction_set(
    options: &LoadOptions,
    file_system: Option<&dyn FileSystem>,
) -> Result<Option<RenderedInstructionSet>, BoxError> {
    if options.max_bytes <= 0.0 || !options.max_bytes.is_finite() || options.max_source_bytes == 0 {
        return Ok(None);
    }
    let discovered = discover(&options.discovery, file_system).await?;
    let mut observed = Vec::new();
    for file in discovered {
        if let Some(content) = read_bounded(
            &file,
            options.max_source_bytes,
            file_system,
            options.discovery.signal.as_ref(),
        )
        .await?
        {
            observed.push(LoadedInstructionFile {
                absolute_path: file.file.absolute_path,
                display_path: file.file.display_path,
                content,
                version: file.version,
            });
        }
    }
    let kept = dedup_instruction_files_by_candidate_group(&observed);
    if kept.is_empty() && !options.replace_previous_baseline {
        return Ok(None);
    }
    let kept_paths: HashSet<_> = kept
        .iter()
        .map(|file| file.absolute_path.as_str())
        .collect();
    let deduped = observed
        .iter()
        .filter(|file| !kept_paths.contains(file.absolute_path.as_str()))
        .cloned()
        .collect();
    let (rendered, included) =
        render_agent_instruction_set(&kept, options.max_bytes, options.replace_previous_baseline);
    Ok(Some(RenderedInstructionSet {
        rendered,
        observed,
        deduped,
        included,
    }))
}

pub async fn load_baseline_instructions(
    options: &LoadOptions,
    file_system: Option<&dyn FileSystem>,
) -> Result<Option<RenderedAgentInstructions>, BoxError> {
    Ok(load_baseline_instruction_set(options, file_system)
        .await?
        .map(|set| set.rendered))
}

pub fn scope_instruction_file(
    scope: &str,
    project_root: &str,
    resolved: &ResolvedDiscoveryConfig,
) -> InstructionFile {
    let (directory, candidate) = decode_scope_key(scope);
    let (directory_path, display) = match directory.as_str() {
        USER_GLOBAL_DIRECTORY => (
            resolved.dsh_home.clone(),
            dsh_home_display(Path::new(&resolved.dsh_home)).unwrap_or("$DSH_HOME"),
        ),
        AGENTS_GLOBAL_DIRECTORY => (
            resolved.agents_home.clone(),
            agents_home_display(Path::new(&resolved.agents_home)).unwrap_or("$DSH_AGENTS_HOME"),
        ),
        "." => (project_root.into(), ""),
        _ => (node_path::join(&[project_root, &directory]), ""),
    };
    let absolute_path = node_path::join(&[&directory_path, &candidate]);
    InstructionFile {
        display_path: if display.is_empty() {
            relative_display(project_root, &absolute_path)
        } else {
            format!("{display}/{candidate}")
        },
        absolute_path,
    }
}

pub async fn probe_scope_instruction(
    scope: &str,
    project_root: &str,
    resolved: &ResolvedConfig,
    file_system: &dyn FileSystem,
    signal: Option<&AbortSignal>,
) -> Result<ScopeInstructionProbe, BoxError> {
    let candidate = scope_instruction_file(scope, project_root, &resolved.discovery);
    let target = match file_system
        .resolve(&candidate.absolute_path, fs_options(signal))
        .await
    {
        Ok(target) => target,
        Err(_) => {
            check_signal(signal)?;
            return Ok(ScopeInstructionProbe::Unavailable);
        }
    };
    let info: Option<FsInfo> = match file_system.stat(&target, signal.cloned()).await {
        Ok(info) => info,
        Err(_) => {
            check_signal(signal)?;
            return Ok(ScopeInstructionProbe::Unavailable);
        }
    };
    match info {
        Some(info) if info.r#type == FsType::File => {
            Ok(ScopeInstructionProbe::Present(ProbedInstructionFile {
                absolute_path: candidate.absolute_path,
                display_path: candidate.display_path,
                target,
                version: info.version,
                size: info.size,
            }))
        }
        _ => Ok(ScopeInstructionProbe::Absent),
    }
}

pub async fn read_scope_instruction(
    file: &ProbedInstructionFile,
    max_source_bytes: u64,
    file_system: &dyn FileSystem,
    signal: Option<&AbortSignal>,
) -> Result<Option<LoadedInstructionFile>, BoxError> {
    let discovered = DiscoveredInstructionFile {
        file: InstructionFile {
            absolute_path: file.absolute_path.clone(),
            display_path: file.display_path.clone(),
        },
        target: Some(file.target.clone()),
        size: file.size,
        version: Some(file.version.clone()),
    };
    Ok(
        read_bounded(&discovered, max_source_bytes, Some(file_system), signal)
            .await?
            .map(|content| LoadedInstructionFile {
                absolute_path: file.absolute_path.clone(),
                display_path: file.display_path.clone(),
                content,
                version: Some(file.version.clone()),
            }),
    )
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use hoh_fs::{FsError, FsErrorCode, FsTargetKey};
    use hoh_timeout::AbortController;
    use std::sync::Arc;

    #[test]
    fn provider_not_found_marker_is_absent() {
        let error = FsError::new("missing marker", FsErrorCode::NotFound);
        assert!(!classify_provider_marker_result(Err(error.into()), None).unwrap());
    }

    #[test]
    fn provider_io_marker_error_is_not_treated_as_absent() {
        let error = FsError::new("provider unavailable", FsErrorCode::IoError);
        assert!(classify_provider_marker_result(Err(error.into()), None).is_err());
    }

    #[test]
    fn provider_stat_failure_is_unavailable_not_absent() {
        let error = FsError::new("provider unavailable", FsErrorCode::IoError);
        let probe = classify_provider_stat_result(Err(error.into()), None).unwrap();
        assert!(matches!(probe, StatProbe::Unavailable));
    }

    #[test]
    fn provider_stat_distinguishes_missing_non_file_and_sized_file() {
        let target = FsTarget {
            target_key: FsTargetKey::from("provider-target"),
            display_path: "/repo/AGENTS.md".into(),
        };
        assert!(matches!(
            classify_provider_stat_result(Ok((target.clone(), None)), None).unwrap(),
            StatProbe::Absent
        ));
        assert!(matches!(
            classify_provider_stat_result(
                Ok((
                    target.clone(),
                    Some(FsInfo {
                        version: FsVersion::from("directory-version"),
                        r#type: FsType::Directory,
                        size: None,
                    }),
                )),
                None,
            )
            .unwrap(),
            StatProbe::Absent
        ));
        let probe = classify_provider_stat_result(
            Ok((
                target,
                Some(FsInfo {
                    version: FsVersion::from("file-version"),
                    r#type: FsType::File,
                    size: Some(73),
                }),
            )),
            None,
        )
        .unwrap();
        assert!(matches!(probe, StatProbe::Present { size: Some(73), .. }));
        let probe = classify_provider_stat_result(
            Ok((
                FsTarget {
                    target_key: FsTargetKey::from("unsized-provider-target"),
                    display_path: "/repo/CLAUDE.md".into(),
                },
                Some(FsInfo {
                    version: FsVersion::from("unsized-file-version"),
                    r#type: FsType::File,
                    size: None,
                }),
            )),
            None,
        )
        .unwrap();
        assert!(matches!(probe, StatProbe::Present { size: None, .. }));
    }

    #[tokio::test]
    async fn provider_stream_without_a_size_is_still_bounded() {
        let stream = futures_util::stream::iter([
            Ok::<_, BoxError>("1234".to_owned()),
            Ok::<_, BoxError>("5678".to_owned()),
        ]);
        assert_eq!(
            read_bounded_stream(Box::pin(stream), 7, None)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn abort_during_provider_stream_read_is_propagated() {
        let controller = Arc::new(AbortController::new());
        let stream_controller = controller.clone();
        let stream = futures_util::stream::unfold(0, move |index| {
            let controller = stream_controller.clone();
            async move {
                match index {
                    0 => Some((Ok("first".to_owned()), 1)),
                    1 => {
                        controller.abort();
                        Some((Ok("second".to_owned()), 2))
                    }
                    _ => None,
                }
            }
        });
        let result = read_bounded_stream(Box::pin(stream), 1024, Some(&controller.signal())).await;
        assert!(result.is_err());
    }
}
