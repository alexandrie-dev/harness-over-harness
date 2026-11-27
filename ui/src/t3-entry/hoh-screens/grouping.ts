import type { HohExecutor } from '../../hoh-host/config';
export const GROUP_AGENTS_STORAGE_KEY = 'hoh-home.group-agents-by-project.v1';
export const PROJECT_COLOR_COUNT = 8;

export interface GroupableAgent {
  kind: string;
}

export interface GroupableEntry {
  id: string;
  projectId: string;
  state: 'working' | 'waiting' | 'done' | 'ended';
  startedAt: number;
  canComplete?: boolean;
  interrupted?: boolean;
  stopped?: boolean;
  agents: GroupableAgent[];
  runner?: HohExecutor;
}

export interface ProjectGroup<Entry extends GroupableEntry = GroupableEntry> {
  id: string;
  title: string;
  color: number;
  entries: Entry[];
  totalChats?: number;
  waiting: number;
  ready: number;
  states: { working: number; waiting: number; ready: number; stopped: number; interrupted: number; done: number; inactive: number };
  runners: Array<HohExecutor>;
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
  // Fold the well-distributed high bits down before taking a small palette modulus.
  hash ^= hash >>> 16;
  return (hash >>> 0) % Math.max(1, paletteSize);
}

export function runnerKinds(entries: readonly GroupableEntry[]): Array<HohExecutor> {
  // Agent.kind is a subagent role (explore, plan, ...), not its provider.
  return entries.map((entry) => entry.runner ?? 'claude');
}

export function groupEntriesByProject<Entry extends GroupableEntry>(
  entries: readonly Entry[],
  titleOf: (projectId: string) => string,
  keyOf: (projectId: string) => string,
): ProjectGroup<Entry>[] {
  const grouped = new Map<string, Entry[]>();
  const colorKeys = new Map<string, string>();
  for (const entry of entries) {
    const list = grouped.get(entry.projectId);
    if (list) list.push(entry); else grouped.set(entry.projectId, [entry]);
  }
  const groups = [...grouped].map(([id, projectEntries]) => {
    projectEntries.sort((a, b) => b.startedAt - a.startedAt);
    const states = { working: 0, waiting: 0, ready: 0, stopped: 0, interrupted: 0, done: 0, inactive: 0 };
    let ready = 0;
    for (const entry of projectEntries) {
      if (entry.state === 'working') states.working += 1;
      else if (entry.state === 'waiting' && entry.stopped) states.stopped += 1;
      else if (entry.state === 'waiting' && entry.interrupted) states.interrupted += 1;
      else if (entry.state === 'waiting' && entry.canComplete) { ready += 1; states.ready += 1; }
      else if (entry.state === 'waiting') states.waiting += 1;
      else if (entry.state === 'done') states.done += 1;
      else states.inactive += 1;
    }
    const colorKey = keyOf(id) || id;
    colorKeys.set(id, colorKey);
    return {
      id,
      title: id === 'hoh-project' ? 'Chats' : titleOf(id).trim() || 'Chats',
      color: projectColor(colorKey),
      entries: projectEntries,
      waiting: states.waiting,
      ready,
      states,
      runners: runnerKinds(projectEntries),
    };
  });
  // Resolve finite-palette collisions in path order, before presentation sorting. Renaming a
  // project or changing its state therefore cannot transfer a collision adjustment to it.
  const usedColors = new Set<number>();
  for (const group of [...groups].sort((a, b) => colorKeys.get(a.id)!.localeCompare(colorKeys.get(b.id)!))) {
    if (usedColors.size === PROJECT_COLOR_COUNT) usedColors.clear();
    while (usedColors.has(group.color)) group.color = (group.color + 1) % PROJECT_COLOR_COUNT;
    usedColors.add(group.color);
  }
  groups.sort((a, b) => (b.waiting - a.waiting) || a.title.localeCompare(b.title));
  // Keep adjacent folders visibly distinct even when the palette wraps after eight projects.
  const hues = [240, 180, 120, 45, 315, 25, 0, 280];
  for (let index = 1; index < groups.length; index += 1) {
    const previous = hues[groups[index - 1].color];
    const distance = (color: number) => Math.min(Math.abs(hues[color] - previous), 360 - Math.abs(hues[color] - previous));
    if (distance(groups[index].color) >= 40) continue;
    const occupied = new Set(groups.filter((_, other) => other !== index).map((group) => group.color));
    const candidate = hues.findIndex((_, color) => !occupied.has(color) && distance(color) >= 40);
    if (candidate >= 0) groups[index].color = candidate;
    else groups[index].color = hues.findIndex((_, color) => distance(color) >= 40);
  }
  return groups;
}
