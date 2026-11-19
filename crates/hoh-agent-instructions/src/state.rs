//! Durable, session-visible instruction state and dynamic reconciliation.

use crate::{
    config::ResolvedConfig,
    digest::{instruction_content_sha1, trimmed_instruction_digest},
    files::{
        LoadedInstructionFile, ScopeInstructionProbe, ancestor_chain, descendant_dirs_between,
        find_project_root, probe_scope_instruction, read_scope_instruction, relative_display,
        scope_instruction_file,
    },
    render::{
        AgentInstructionChange, ChangeRenderItem, InstructionAction, USER_GLOBAL_DIRECTORIES,
        USER_GLOBAL_FILE, candidate_scope_key, decode_scope_key, instruction_candidate_group,
        instruction_scope_key, is_user_global_directory, render_instruction_changes,
    },
};
use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_fs::{FileSystem, FsVersion};
use hoh_llm::{
    ContentBlock, Message, MessageSource, NewUserMessage, UserMessage, create_user_message,
};
use hoh_session::{Session, SessionId};
use hoh_timeout::AbortSignal;
use indexmap::{IndexMap, IndexSet};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

pub const NAME: &str = "agent-instructions";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionVersionState {
    pub path: String,
    pub version: FsVersion,
    pub digest: String,
    pub trimmed_digest: String,
}

#[derive(Default)]
pub struct InstructionVersionCache(
    Mutex<HashMap<SessionId, HashMap<String, InstructionVersionState>>>,
);

impl InstructionVersionCache {
    pub fn states(&self, session: &Session) -> HashMap<String, InstructionVersionState> {
        lock(&self.0).get(session.id()).cloned().unwrap_or_default()
    }

    pub fn replace_states(
        &self,
        session: &Session,
        states: HashMap<String, InstructionVersionState>,
    ) {
        let mut cache = lock(&self.0);
        if states.is_empty() {
            cache.remove(session.id());
        } else {
            cache.insert(session.id().clone(), states);
        }
    }

    pub fn merge_states(
        &self,
        session: &Session,
        additions: impl IntoIterator<Item = (String, InstructionVersionState)>,
    ) {
        let mut states = self.states(session);
        states.extend(additions);
        self.replace_states(session, states);
    }

    pub fn remove_session(&self, session: &Session) {
        lock(&self.0).remove(session.id());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionVersionUpdate {
    pub change: AgentInstructionChange,
    pub state: Option<InstructionVersionState>,
}

#[derive(Clone, Debug)]
pub struct ReconciledInstructionContext {
    pub context: UserMessage,
    pub version_updates: Vec<InstructionVersionUpdate>,
}

pub struct ReconcileOptions<'a> {
    pub cwd: &'a str,
    pub authority_messages: &'a [UserMessage],
    pub scope_messages: &'a [UserMessage],
    pub touched_paths: &'a [String],
    pub include_baseline_scopes: bool,
    pub excluded_baseline_scopes: Option<&'a HashSet<String>>,
    pub project_root: Option<&'a str>,
    pub signal: Option<&'a AbortSignal>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn changes_value(changes: &[AgentInstructionChange]) -> Value {
    serde_json::to_value(changes).expect("instruction changes are JSON")
}

fn instruction_source(
    changes: &[AgentInstructionChange],
    baseline: bool,
    identity: Option<&str>,
) -> MessageSource {
    let mut data = Map::new();
    data.insert("form".into(), Value::String("instructions".into()));
    if baseline {
        data.insert("baseline".into(), Value::Bool(true));
    }
    if let Some(identity) = identity {
        data.insert("baselineIdentity".into(), Value::String(identity.into()));
    }
    data.insert("changes".into(), changes_value(changes));
    MessageSource {
        kind: NAME.into(),
        data,
    }
}

pub fn workspace_context_hook(
    text: impl Into<String>,
    changes: &[AgentInstructionChange],
) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: instruction_source(changes, false, None),
    })
}

pub fn workspace_context_message(text: impl Into<String>) -> Message {
    Message::User(create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: instruction_source(&[], false, None),
    }))
}

pub fn baseline_context_message(
    content: Vec<ContentBlock>,
    changes: &[AgentInstructionChange],
    identity: &str,
) -> UserMessage {
    create_user_message(NewUserMessage {
        content,
        source: instruction_source(changes, true, Some(identity)),
    })
}

pub fn workspace_instruction_changes(source: &MessageSource) -> Vec<AgentInstructionChange> {
    if source.kind != NAME {
        return vec![];
    }
    source
        .data
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value(value.clone()).ok())
        .collect()
}

fn same_change(left: &AgentInstructionChange, right: &AgentInstructionChange) -> bool {
    left == right
}

fn visible_instruction_changes(
    agent: &dyn Agent,
    authority_messages: &[UserMessage],
) -> Result<IndexMap<String, AgentInstructionChange>, BoxError> {
    let visible_seqs: HashSet<_> = agent.session().surface()?.nodes()?.into_iter().collect();
    let mut visible = IndexMap::new();
    for event in agent.session().snapshot_events(None, None).iter() {
        if event.event_type() != "user/message" || !visible_seqs.contains(&event.seq()) {
            continue;
        }
        let Ok(message) = serde_json::from_value::<UserMessage>(event.data().clone()) else {
            continue;
        };
        for change in workspace_instruction_changes(&message.source) {
            visible.insert(change.scope.clone(), change);
        }
    }
    for message in authority_messages {
        for change in workspace_instruction_changes(&message.source) {
            visible.insert(change.scope.clone(), change);
        }
    }
    Ok(visible)
}

pub fn baseline_instruction_state(
    files: &[LoadedInstructionFile],
) -> (
    IndexMap<String, AgentInstructionChange>,
    HashMap<String, InstructionVersionState>,
) {
    let mut changes = IndexMap::new();
    let mut versions = HashMap::new();
    for file in files {
        let digest = instruction_content_sha1(&file.content);
        let change = AgentInstructionChange {
            action: InstructionAction::Set,
            scope: instruction_scope_key(&file.display_path),
            path: file.display_path.clone(),
            digest: Some(digest.clone()),
        };
        changes.insert(change.scope.clone(), change.clone());
        if let Some(version) = &file.version {
            versions.insert(
                change.scope.clone(),
                InstructionVersionState {
                    path: file.display_path.clone(),
                    version: version.clone(),
                    digest,
                    trimmed_digest: trimmed_instruction_digest(&file.content),
                },
            );
        }
    }
    (changes, versions)
}

pub fn retained_instruction_version_updates(
    updates: &[InstructionVersionUpdate],
    rendered_changes: &[AgentInstructionChange],
) -> Vec<InstructionVersionUpdate> {
    updates
        .iter()
        .filter(|update| {
            rendered_changes
                .iter()
                .any(|change| same_change(&update.change, change))
        })
        .cloned()
        .collect()
}

pub fn apply_instruction_version_updates(
    session: &Session,
    updates: &[InstructionVersionUpdate],
    cache: &InstructionVersionCache,
) {
    if updates.is_empty() {
        return;
    }
    let mut states = cache.states(session);
    for update in updates {
        match &update.state {
            Some(state) => {
                states.insert(update.change.scope.clone(), state.clone());
            }
            None => {
                states.remove(&update.change.scope);
            }
        }
    }
    cache.replace_states(session, states);
}

fn relative_scope(project_root: &str, directory: &str) -> String {
    let scope = relative_display(project_root, directory);
    if scope.is_empty() { ".".into() } else { scope }
}

fn push_removal(
    items: &mut Vec<ChangeRenderItem>,
    updates: &mut Vec<InstructionVersionUpdate>,
    scope: &str,
    path: &str,
    state: Option<InstructionVersionState>,
) {
    let change = AgentInstructionChange {
        action: InstructionAction::Remove,
        scope: scope.into(),
        path: path.into(),
        digest: None,
    };
    items.push(ChangeRenderItem {
        file: LoadedInstructionFile {
            absolute_path: format!("removed:{scope}"),
            display_path: path.into(),
            content: String::new(),
            version: None,
        },
        change: change.clone(),
    });
    updates.push(InstructionVersionUpdate { change, state });
}

pub async fn reconcile_instruction_context(
    agent: &dyn Agent,
    resolved: &ResolvedConfig,
    version_cache: &InstructionVersionCache,
    file_system: &dyn FileSystem,
    options: ReconcileOptions<'_>,
) -> Result<Option<ReconciledInstructionContext>, BoxError> {
    let effective = visible_instruction_changes(agent, options.authority_messages)?;
    let cwd = options.cwd;
    let project_root_storage;
    let project_root = if let Some(root) = options.project_root {
        root
    } else {
        project_root_storage = find_project_root(
            cwd,
            &resolved.project_root_markers,
            Some(file_system),
            options.signal,
        )
        .await?;
        &project_root_storage
    };

    let mut scopes = IndexSet::new();
    let mut baseline_scopes = IndexSet::new();
    let add_dir_scopes = |target: &mut IndexSet<String>, directory: &str| {
        for candidate in &resolved.instruction_file_candidates {
            target.insert(candidate_scope_key(directory, candidate));
        }
        for candidate in &resolved.local_instruction_file_candidates {
            target.insert(candidate_scope_key(directory, candidate));
        }
    };
    for directory in USER_GLOBAL_DIRECTORIES {
        baseline_scopes.insert(candidate_scope_key(directory, USER_GLOBAL_FILE));
    }
    for directory in ancestor_chain(project_root, cwd) {
        add_dir_scopes(
            &mut baseline_scopes,
            &relative_scope(project_root, &directory),
        );
    }
    if options.include_baseline_scopes {
        scopes.extend(baseline_scopes.iter().cloned());
    }
    for message in options.scope_messages {
        for change in workspace_instruction_changes(&message.source) {
            if !options.include_baseline_scopes && baseline_scopes.contains(&change.scope) {
                continue;
            }
            scopes.insert(change.scope);
        }
    }
    for scope in effective.keys() {
        if !options.include_baseline_scopes && baseline_scopes.contains(scope) {
            continue;
        }
        let (directory, _) = decode_scope_key(scope);
        if is_user_global_directory(&directory) {
            scopes.insert(candidate_scope_key(&directory, USER_GLOBAL_FILE));
        } else {
            add_dir_scopes(&mut scopes, &directory);
        }
    }
    for path in options.touched_paths {
        for directory in descendant_dirs_between(cwd, path) {
            add_dir_scopes(&mut scopes, &relative_scope(project_root, &directory));
        }
    }

    let mut versions = version_cache.states(agent.session());
    let mut seen_paths = HashSet::new();
    let mut kept_trimmed: HashMap<String, HashSet<String>> = HashMap::new();
    let mut items = Vec::new();
    let mut updates = Vec::new();
    let mut grouped: IndexMap<String, Vec<String>> = IndexMap::new();
    for scope in scopes {
        let (directory, _) = decode_scope_key(&scope);
        grouped
            .entry(instruction_candidate_group(&directory))
            .or_default()
            .push(scope);
    }

    for (group, group_scopes) in grouped {
        let mut probed_scopes = Vec::new();
        for scope in group_scopes {
            let excluded = options.excluded_baseline_scopes.is_some_and(|excluded| {
                baseline_scopes.contains(&scope) && excluded.contains(&scope)
            });
            if excluded {
                match effective.get(&scope) {
                    Some(previous) if previous.action != InstructionAction::Remove => {
                        push_removal(&mut items, &mut updates, &scope, &previous.path, None);
                    }
                    _ => {
                        versions.remove(&scope);
                    }
                }
            } else {
                probed_scopes.push(scope);
            }
        }

        let item_start = items.len();
        let update_start = updates.len();
        let prior_versions: HashMap<_, _> = probed_scopes
            .iter()
            .map(|scope| (scope.clone(), versions.get(scope).cloned()))
            .collect();
        let rollback_group =
            |items: &mut Vec<ChangeRenderItem>,
             updates: &mut Vec<InstructionVersionUpdate>,
             versions: &mut HashMap<String, InstructionVersionState>,
             seen_paths: &mut HashSet<String>,
             kept_trimmed: &mut HashMap<String, HashSet<String>>| {
                items.truncate(item_start);
                updates.truncate(update_start);
                for (candidate, prior) in &prior_versions {
                    match prior {
                        Some(state) => {
                            versions.insert(candidate.clone(), state.clone());
                        }
                        None => {
                            versions.remove(candidate);
                        }
                    }
                }
                for scope in &probed_scopes {
                    seen_paths.insert(
                        scope_instruction_file(scope, project_root, &resolved.discovery)
                            .absolute_path,
                    );
                }
                kept_trimmed.remove(&group);
            };
        for scope in &probed_scopes {
            let previous = effective.get(scope);
            let probe =
                probe_scope_instruction(scope, project_root, resolved, file_system, options.signal)
                    .await?;
            let probed = match probe {
                ScopeInstructionProbe::Unavailable => {
                    rollback_group(
                        &mut items,
                        &mut updates,
                        &mut versions,
                        &mut seen_paths,
                        &mut kept_trimmed,
                    );
                    break;
                }
                ScopeInstructionProbe::Absent => {
                    match previous {
                        Some(previous) if previous.action != InstructionAction::Remove => {
                            push_removal(&mut items, &mut updates, scope, &previous.path, None);
                        }
                        _ => {
                            versions.remove(scope);
                        }
                    }
                    continue;
                }
                ScopeInstructionProbe::Present(file) => file,
            };
            if !seen_paths.insert(probed.absolute_path.clone()) {
                continue;
            }
            let cached = versions.get(scope).cloned();
            let metadata_unchanged = cached.as_ref().is_some_and(|cached| {
                cached.path == probed.display_path && cached.version == probed.version
            });
            if metadata_unchanged {
                let cached = cached.as_ref().expect("metadata match has cache");
                let rendered = previous.is_some_and(|previous| {
                    previous.action != InstructionAction::Remove
                        && previous.path == cached.path
                        && previous.digest.as_deref() == Some(cached.digest.as_str())
                });
                if rendered {
                    let duplicate = !kept_trimmed
                        .entry(group.clone())
                        .or_default()
                        .insert(cached.trimmed_digest.clone());
                    if duplicate {
                        push_removal(
                            &mut items,
                            &mut updates,
                            scope,
                            &previous.expect("rendered has previous").path,
                            Some(cached.clone()),
                        );
                    }
                    continue;
                }
                if previous.is_none_or(|previous| previous.action == InstructionAction::Remove)
                    && kept_trimmed
                        .get(&group)
                        .is_some_and(|digests| digests.contains(&cached.trimmed_digest))
                {
                    continue;
                }
            }

            let Some(file) = read_scope_instruction(
                &probed,
                resolved.max_source_bytes,
                file_system,
                options.signal,
            )
            .await?
            else {
                rollback_group(
                    &mut items,
                    &mut updates,
                    &mut versions,
                    &mut seen_paths,
                    &mut kept_trimmed,
                );
                break;
            };
            let digest = instruction_content_sha1(&file.content);
            let trimmed_digest = trimmed_instruction_digest(&file.content);
            let next = InstructionVersionState {
                path: file.display_path.clone(),
                version: probed.version,
                digest: digest.clone(),
                trimmed_digest: trimmed_digest.clone(),
            };
            let duplicate = !kept_trimmed
                .entry(group.clone())
                .or_default()
                .insert(trimmed_digest);
            if duplicate {
                if let Some(previous) = previous
                    && previous.action != InstructionAction::Remove
                {
                    push_removal(&mut items, &mut updates, scope, &previous.path, Some(next));
                } else {
                    versions.insert(scope.clone(), next);
                }
                continue;
            }
            if let Some(previous) = previous
                && previous.action != InstructionAction::Remove
                && previous.path == file.display_path
                && previous.digest.as_deref() == Some(digest.as_str())
            {
                versions.insert(scope.clone(), next);
                continue;
            }
            let action = if previous.is_none_or(|value| value.action == InstructionAction::Remove) {
                InstructionAction::Set
            } else {
                InstructionAction::Replace
            };
            let change = AgentInstructionChange {
                action,
                scope: scope.clone(),
                path: file.display_path.clone(),
                digest: Some(digest),
            };
            items.push(ChangeRenderItem {
                change: change.clone(),
                file,
            });
            updates.push(InstructionVersionUpdate {
                change,
                state: Some(next),
            });
        }
    }
    version_cache.replace_states(agent.session(), versions);
    if items.is_empty() {
        return Ok(None);
    }
    let (text, changes) = render_instruction_changes(&items, resolved.max_bytes);
    if text.is_empty() || changes.is_empty() {
        return Ok(None);
    }
    Ok(Some(ReconciledInstructionContext {
        context: workspace_context_hook(text, &changes),
        version_updates: retained_instruction_version_updates(&updates, &changes),
    }))
}
