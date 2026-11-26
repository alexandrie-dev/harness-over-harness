import { scopeProjectRef, scopedProjectKey } from '@t3tools/client-runtime/environment';
import { deriveLogicalProjectKeyFromSettings, selectProjectGroupingSettings } from '@t3tools/client-runtime/state/project-grouping';
import { inferProjectTitleFromPath, isScratchProject } from '@t3tools/client-runtime/state/projects';
import type { EnvironmentProject } from '@t3tools/client-runtime/state/shell';
import { useGT } from 'gt-react';
import { CheckIcon, ChevronDownIcon, FolderIcon, FolderPlusIcon, PlusIcon } from 'lucide-react';
import { openCommandPalette } from '~/commandPaletteBus';
import { composerDraftHasUserContent, useComposerDraftStore, type DraftId } from '~/composerDraftStore';
import { ProjectFavicon } from '~/components/ProjectFavicon';
import { WorkspaceBreadcrumbText } from '~/components/WorkspaceBreadcrumb';
import { Menu, MenuGroup, MenuGroupLabel, MenuItem, MenuPopup, MenuSeparator, MenuTrigger } from '~/components/ui/menu';
import { toastManager } from '~/components/ui/toast';
import { useNewThreadHandler, type NewThreadHandler } from '~/hooks/useHandleNewThread';
import { useClientSettings } from '~/hooks/useSettings';
import { seedDraftModelSelection } from '~/lib/seedDraftModelSelection';
import { useProjects } from '~/state/entities';
import { isKnownAbsoluteFolder } from './folder-trust-view.mjs';

export async function selectChatProject(
  project: EnvironmentProject,
  logicalProjectKey: string,
  draftId: DraftId | null,
  openProjectDraft: NewThreadHandler,
) {
  const projectRef = scopeProjectRef(project.environmentId, project.id);
  const store = useComposerDraftStore.getState();
  const draft = draftId ? store.getDraftSession(draftId) : null;
  // Empty drafts use the project's existing draft-opening flow.
  if (draftId && draft && !draft.promotedTo && composerDraftHasUserContent(store.getComposerDraft(draftId))) {
    store.setLogicalProjectDraftThreadId(logicalProjectKey, projectRef, draftId);
    seedDraftModelSelection(draftId, projectRef);
  } else {
    await openProjectDraft(projectRef);
  }
}

export function ChatProjectMenu({ project, cwd, draftId, onNewChat }: {
  project: EnvironmentProject | null;
  cwd: string | null;
  draftId: DraftId | null;
  onNewChat(): void;
}) {
  const gt = useGT();
  const projects = useProjects();
  const grouping = useClientSettings(selectProjectGroupingSettings);
  const openProjectDraft = useNewThreadHandler();
  const entries = projects
    .map((entry) => ({
      project: entry,
      current: entry.id === project?.id && entry.environmentId === project.environmentId,
      name: isScratchProject(entry, null) ? gt('No project') : entry.title,
    }))
    .filter((entry) => entry.current || entry.project.archivedAt == null)
    .sort((a, b) => Number(b.current) - Number(a.current)
      || Number(b.project.pinnedAt != null) - Number(a.project.pinnedAt != null)
      || a.name.localeCompare(b.name));
  const isScratch = project !== null && isScratchProject(project, null);
  const path = isScratch && !isKnownAbsoluteFolder(cwd) ? null : cwd;
  const title = project && !isScratch
    ? project.title
    : path ? inferProjectTitleFromPath(path) : isScratch ? gt('No project') : gt('Choose a project');

  return (
    <Menu>
      <MenuTrigger
        data-hoh-chat-project
        aria-label={project || path ? gt('Switch project: {project}', { project: title }) : title}
        aria-description={path ?? undefined}
        className="inline-flex min-w-0 max-w-full cursor-pointer items-center gap-1.5 rounded-md px-1.5 text-foreground hover:bg-accent data-popup-open:bg-accent focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring [-webkit-app-region:no-drag]"
      >
        {project && !isScratch ? <ProjectFavicon project={project} className="size-3.5" /> : <FolderIcon aria-hidden className="size-3.5 shrink-0" />}
        <WorkspaceBreadcrumbText>{title}</WorkspaceBreadcrumbText>
        <ChevronDownIcon aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
      </MenuTrigger>
      <MenuPopup align="start" className="w-80" data-hoh-chat-project-menu>
        {path ? (
          <div className="px-2 py-1.5 text-xs text-muted-foreground" data-hoh-chat-cwd>
            <span>{gt('Working directory')}</span>
            <span className="mt-0.5 block break-all text-foreground" dir="auto">{path}</span>
          </div>
        ) : null}
        {entries.length > 0 ? (
          <MenuGroup>
            <MenuGroupLabel>{gt('Projects')}</MenuGroupLabel>
            {entries.map(({ project: targetProject, current, name }) => {
              const root = targetProject.workspaceRoot;
              const showRoot = (!current || root !== path)
                && (!isScratchProject(targetProject, null) || isKnownAbsoluteFolder(root));
              return (
                <MenuItem
                  key={scopedProjectKey(scopeProjectRef(targetProject.environmentId, targetProject.id))}
                  label={name}
                  aria-current={current || undefined}
                  onClick={() => {
                    if (current) return;
                    const logicalKey = deriveLogicalProjectKeyFromSettings(targetProject, grouping);
                    void selectChatProject(targetProject, logicalKey, draftId, openProjectDraft).catch((error: unknown) => {
                      toastManager.add({ type: 'error', title: gt('Failed to open project'), description: error instanceof Error ? error.message : undefined });
                    });
                  }}
                >
                  <ProjectFavicon project={targetProject} className="size-4 shrink-0" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate" dir="auto">{name}</span>
                    {showRoot ? <span className="block truncate text-xs opacity-75" title={root} dir="auto">{root}</span> : null}
                  </span>
                  {current ? <CheckIcon aria-hidden className="size-4 shrink-0" /> : null}
                </MenuItem>
              );
            })}
          </MenuGroup>
        ) : null}
        {path || entries.length > 0 ? <MenuSeparator /> : null}
        {project && draftId === null ? (
          <MenuItem onClick={onNewChat}><PlusIcon aria-hidden />{gt('New chat in {project}', { project: title })}</MenuItem>
        ) : null}
        <MenuItem onClick={() => openCommandPalette({ open: 'add-project' })}><FolderPlusIcon aria-hidden />{gt('Add project')}</MenuItem>
      </MenuPopup>
    </Menu>
  );
}
