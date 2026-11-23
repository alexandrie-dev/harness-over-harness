export const GROUP_AGENTS_STORAGE_KEY = 'hoh-home.group-agents-by-project.v1';
export const PROJECT_COLOR_COUNT = 6;

export interface GroupableAgent {
  kind: string;
}

export interface GroupableEntry {
  id: string;
  projectId: string;
  state: 'working' | 'waiting' | 'done' | 'ended';
  startedAt: number;
  agents: GroupableAgent[];
  runner?: 'claude' | 'codex';
}

export interface ProjectGroup<Entry extends GroupableEntry = GroupableEntry> {
  id: string;
  title: string;
  color: number;
  entries: Entry[];
  waiting: number;
  states: { working: number; waiting: number; done: number };
  runners: Array<'claude' | 'codex'>;
}

export interface ProjectDisclosureState { hovered: string | null; pinned: string | null }
export type ProjectDisclosureEvent =
  | { type: 'hover'; id: string }
  | { type: 'leave' }
  | { type: 'toggle'; id: string }
  | { type: 'close' };

export function updateProjectDisclosure(state: ProjectDisclosureState, event: ProjectDisclosureEvent): ProjectDisclosureState {
  if (event.type === 'hover') return state.pinned === null ? { ...state, hovered: event.id } : state;
  if (event.type === 'leave') return state.pinned === null ? { ...state, hovered: null } : state;
  if (event.type === 'toggle') return { hovered: null, pinned: state.pinned === event.id ? null : event.id };
  return { hovered: null, pinned: null };
}

let memoryValue = false;
const listeners = new Set<() => void>();

export function getGroupAgentsByProject(): boolean {
  try {
    const stored = globalThis.localStorage?.getItem(GROUP_AGENTS_STORAGE_KEY);
    return stored === null || stored === undefined ? memoryValue : stored === 'true';
  } catch {
    return memoryValue;
  }
}

export function subscribeGroupAgentsByProject(listener: () => void): () => void {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => {
    if (event.key === GROUP_AGENTS_STORAGE_KEY) listener();
  };
  globalThis.addEventListener?.('storage', onStorage);
  return () => {
    listeners.delete(listener);
    globalThis.removeEventListener?.('storage', onStorage);
  };
}

export function setGroupAgentsByProject(value: boolean): void {
  memoryValue = value;
  try { globalThis.localStorage?.setItem(GROUP_AGENTS_STORAGE_KEY, String(value)); } catch { /* memory fallback */ }
  for (const listener of listeners) listener();
}

/** FNV-1a keeps a project's palette choice stable without exposing its path in the DOM. */
export function projectColor(path: string, paletteSize = PROJECT_COLOR_COUNT): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < path.length; index += 1) {
    hash ^= path.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % Math.max(1, paletteSize);
}

export function runnerKinds(entries: readonly GroupableEntry[]): Array<'claude' | 'codex'> {
  // Agent.kind is a subagent role (explore, plan, ...), not its provider.
  // Today the HoH window runs Claude; the optional field keeps the view model ready for Codex cards.
  return entries.map((entry) => entry.runner ?? 'claude');
}

export function groupEntriesByProject<Entry extends GroupableEntry>(
  entries: readonly Entry[],
  titleOf: (projectId: string) => string,
  keyOf: (projectId: string) => string,
): ProjectGroup<Entry>[] {
  const grouped = new Map<string, Entry[]>();
  for (const entry of entries) {
    const list = grouped.get(entry.projectId);
    if (list) list.push(entry); else grouped.set(entry.projectId, [entry]);
  }
  return [...grouped].map(([id, projectEntries]) => {
    projectEntries.sort((a, b) => b.startedAt - a.startedAt);
    const states = { working: 0, waiting: 0, done: 0 };
    for (const entry of projectEntries) {
      if (entry.state === 'working') states.working += 1;
      else if (entry.state === 'waiting') states.waiting += 1;
      else states.done += 1;
    }
    return {
      id,
      title: titleOf(id).trim() || 'No project',
      color: projectColor(keyOf(id) || id),
      entries: projectEntries,
      waiting: states.waiting,
      states,
      runners: runnerKinds(projectEntries),
    };
  }).sort((a, b) => (b.waiting - a.waiting) || a.title.localeCompare(b.title));
}
