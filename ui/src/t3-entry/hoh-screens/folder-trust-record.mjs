// Share the chat metadata record with the host, without overwriting titles or prompt times.
const KEY = 'hoh-host.thread-metadata.v1';
export function readAppliedTrust(storage, id) {
  try { return JSON.parse(storage.getItem(KEY) ?? '{}')?.[id]?.appliedProjectTrust ?? null; } catch { return null; }
}
export function writeAppliedTrust(storage, id, value) {
  try {
    const all = JSON.parse(storage.getItem(KEY) ?? '{}');
    if (!all || typeof all !== 'object' || Array.isArray(all)) return;
    const safe = Object.assign(Object.create(null), all);
    safe[id] = { ...safe[id], appliedProjectTrust: value };
    storage.setItem(KEY, JSON.stringify(safe));
  } catch { /* The banner can fall back to live process state. */ }
}
