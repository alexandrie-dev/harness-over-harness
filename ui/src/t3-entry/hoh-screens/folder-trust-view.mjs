export function folderTrustView({ applied, trusted, isolated, changed }) {
  let text = applied ? 'Настройки проекта применяются' : 'Настройки проекта не применяются';
  if (trusted && !applied && !isolated) text += ' — применятся со следующего хода';
  if (!trusted && applied && !changed) text += ' — перестанут применяться со следующего хода';
  if (changed && !trusted) text += ' — настройки изменились, доверять снова?';
  return { text, canTrust: !trusted && !isolated };
}
