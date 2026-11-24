export function folderTrustView({ running, applied, trusted, isolated, changed }) {
  let text = running
    ? `Настройки проекта в этой папке (Claude и Codex): ${applied ? 'применяются' : 'не применяются'}`
    : `Процесс не запущен; следующий ход — настройки проекта в этой папке (Claude и Codex) ${trusted && !isolated ? 'применяются' : 'не применяются'}`;
  if (running && trusted && !applied && !isolated) text += ' — применятся со следующего хода';
  if (running && !trusted && applied && !changed) text += ' — перестанут применяться со следующего хода';
  if (changed && !trusted) text += ' — настройки изменились, доверять снова?';
  return { text, canTrust: !trusted && !isolated };
}
