// Writes explorer_target.jsonl: the target the source hands to explorer.exe for a Windows path, one per
// line: {"path","target"}. The function is copied from packages/util/native-command/src/path-opener.ts
// (`explorerTarget`); Node's `pathToFileURL(path, { windows: true })` does the work.
// Owned by the lead; run with Node 22:
//   node crates/hoh-native-command/tests/fixtures/gen-explorer-target.mjs crates/hoh-native-command/tests/fixtures/explorer_target.jsonl
// Only absolute paths with a drive or a UNC share: for anything else Node consults the process's directory.
import { writeFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'

function explorerTarget(windowsPath) {
  const href = pathToFileURL(windowsPath, { windows: true }).href
  return href
    .replace(/(?:%[89A-F][0-9A-F])+/gi, escaped => decodeURIComponent(escaped))
    .replaceAll(',', '%2C')
    .replaceAll('=', '%3D')
}

const paths = [
  'C:\\', 'C:\\w', 'C:\\w\\', 'C:\\w\\file.txt', 'c:\\w\\file.txt', 'C:/w/file.txt', 'C:\\w/mixed\\file.txt',
  'C:\\w\\..\\file.txt', 'C:\\..\\file.txt', 'C:\\w\\.\\file.txt', 'C:\\w\\a\\..\\..\\..\\b', 'C:\\w\\\\double\\\\file.txt',
  'C:\\w\\dir\\..', 'C:\\w\\dir\\.', 'C:\\w\\dir\\\\', 'C:\\w\\dir.\\file', 'C:\\w\\trailing space \\file',
  'C:\\Program Files\\App\\run me.exe', 'C:\\w\\100%.txt', 'C:\\w\\%C3%A9.txt', 'C:\\w\\a,b=c.txt',
  'C:\\w\\Ünï 文 файл.txt', 'C:\\w\\emoji 😀.txt', 'C:\\w\\e\u0301.txt', 'C:\\Пользователи\\Имя\\Документ.docx',
  'C:\\w\\tab\there.txt', 'C:\\w\\line\nbreak.txt', 'C:\\w\\ctl\u0001.txt', 'C:\\w\\del\u007f.txt',
  '\\\\server\\share', '\\\\server\\share\\', '\\\\server\\share\\dir\\file.txt', '\\\\SERVER\\Share\\File.TXT',
  '\\\\server\\share\\..\\file.txt', '\\\\server\\share\\a b,c=d.txt', '\\\\server\\share\\файл.txt',
  '\\\\wsl.localhost\\Ubuntu\\home\\u\\file.txt', '\\\\wsl$\\Ubuntu\\home\\u\\file.txt', '\\\\192.168.1.5\\share\\f.txt',
  '\\\\?\\C:\\w\\file.txt', '\\\\?\\UNC\\server\\share\\file.txt', '\\\\.\\pipe\\name', '//server/share/file.txt',
]
for (let code = 0x20; code <= 0x7e; code++) {
  const ch = String.fromCharCode(code)
  if (ch !== '\\' && ch !== '/') paths.push(`C:\\w\\a${ch}b.txt`)
}

const lines = paths.map((path) => {
  let target
  try { target = explorerTarget(path) } catch (error) { target = null }
  return JSON.stringify({ path, target })
})
writeFileSync(process.argv[2], lines.join('\n') + '\n')
console.log(`${lines.length} paths, ${lines.filter(line => line.endsWith('"target":null}')).length} refused by Node`)
