export function isKnownAbsoluteFolder(cwd) { return typeof cwd === 'string' && cwd.startsWith('/'); }

export function folderTrustView({ running, applied, trusted, isolated, changed }) {
  let text = trusted && !isolated && (applied || !running)
    ? 'Project settings in this folder (Claude and Codex): active'
    : trusted && !isolated && !applied
    ? 'Project settings in this folder (Claude and Codex) will be enabled from the next message'
    : running
    ? `Project settings in this folder (Claude and Codex): ${applied ? 'active' : 'inactive'}`
    : 'Project settings in this folder (Claude and Codex): inactive';
  if (running && !trusted && applied && !changed) text += ' — inactive from the next turn';
  if (changed && !trusted) text += ' — settings changed; trust again?';
  return { text, canTrust: !trusted && !isolated };
}
