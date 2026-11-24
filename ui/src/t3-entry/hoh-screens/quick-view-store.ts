export interface QuickViewFocusTarget {
  readonly isConnected?: boolean;
  focus(): void;
}

export interface QuickViewState {
  readonly threadId: string | null;
  readonly returnFocus: QuickViewFocusTarget | null;
}

const CLOSED: QuickViewState = Object.freeze({ threadId: null, returnFocus: null });
const listeners = new Set<() => void>();
let state = CLOSED;

function activeFocusTarget(): QuickViewFocusTarget | null {
  if (typeof document === 'undefined') return null;
  const active = document.activeElement as Partial<QuickViewFocusTarget> | null;
  return active && typeof active.focus === 'function' ? active as QuickViewFocusTarget : null;
}

function publish(next: QuickViewState): void {
  state = next;
  for (const listener of listeners) listener();
}

/** Opens the shared quick-view host from any agent card, including cards supplied by project folders. */
export function openQuickView(threadId: string): void {
  if (!threadId) return;
  publish(Object.freeze({ threadId, returnFocus: activeFocusTarget() }));
}

/** Closes quick view; full navigation opts out of restoring focus to the card being left behind. */
export function closeQuickView(restoreFocus = true): void {
  if (state.threadId === null) return;
  const target = restoreFocus ? state.returnFocus : null;
  publish(CLOSED);
  if (target) queueMicrotask(() => {
    if (target.isConnected !== false) target.focus();
  });
}

/** Unmount the embedded chat before routing to the same conversation. */
export function openFullFromQuickView(navigate: (threadId: string) => void): void {
  const threadId = state.threadId;
  if (threadId === null) return;
  closeQuickView(false);
  navigate(threadId);
}

export const quickViewStore = {
  getSnapshot: (): QuickViewState => state,
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};
