// T3 already persists the user's last provider selection across window launches.
// Read it only as a display fallback while the provider catalog is pending.
export function coldComposerModel(storage, instanceId, currentModel) {
  if (instanceId !== 'claudeAgent' && instanceId !== 'codex' && instanceId !== 'pi') return null;
  const usable = value => typeof value === 'string' && value.trim() && !['default', 'unknown'].includes(value.trim().toLowerCase()) && value.length <= 200 ? value.trim() : null;
  const current = usable(currentModel);
  if (current) return current;
  try {
    const saved = JSON.parse(storage?.getItem('t3code:composer-drafts:v1') ?? 'null');
    const entry = saved?.state?.stickyModelSelectionByProvider?.[instanceId];
    return entry?.instanceId === instanceId ? usable(entry.model) : null;
  } catch { return null; }
}
