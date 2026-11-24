export function folderTrustView({ running, applied, trusted, isolated, changed }) {
  let text = running
    ? `Project settings in this folder (Claude and Codex): ${applied ? 'active' : 'inactive'}`
    : `No process running; project settings in this folder (Claude and Codex) will be ${trusted && !isolated ? 'active' : 'inactive'} on the next turn`;
  if (running && trusted && !applied && !isolated) text += ' — active from the next turn';
  if (running && !trusted && applied && !changed) text += ' — inactive from the next turn';
  if (changed && !trusted) text += ' — settings changed; trust again?';
  return { text, canTrust: !trusted && !isolated };
}
