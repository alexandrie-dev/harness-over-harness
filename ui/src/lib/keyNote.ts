/** Where the API key lives, said to the person. The engine keeps it in a file of its own data folder
 *  (hoh-credentials-local, mode 0600); the window never sees the key itself. `path` is added when the engine reports it. */
export function keyNote(path?: string): string {
  return `Ключ хранится в файле на этом компьютере${path ? `: ${path}` : ''}. Читать его может только ваша учётная запись.`;
}
