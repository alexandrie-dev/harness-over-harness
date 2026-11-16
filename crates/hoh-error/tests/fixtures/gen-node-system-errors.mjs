// Writes node_system_errors.json: the names and texts Node gives to system errors, and the messages its
// fs and child_process functions really produce for common failures (Linux).
// Owned by the lead; run with Node 22, giving an empty scratch directory for the experiments:
//   node crates/hoh-error/tests/fixtures/gen-node-system-errors.mjs <scratch dir> crates/hoh-error/tests/fixtures/node_system_errors.json
import fs from 'node:fs'
import fsp from 'node:fs/promises'
import path from 'node:path'
import util from 'node:util'
import { execFile } from 'node:child_process'

const [root, target] = process.argv.slice(2)
fs.mkdirSync(root, { recursive: true })

const table = [...util.getSystemErrorMap()].map(([errno, [code, description]]) => ({ errno, code, description }))

const file = path.join(root, 'file')
const dir = path.join(root, 'dir')
const full = path.join(root, 'full')
const missing = path.join(root, 'missing')
const under = path.join(file, 'x')
fs.writeFileSync(file, 'x', { mode: 0o644 })
fs.mkdirSync(dir, { recursive: true })
fs.mkdirSync(full, { recursive: true })
fs.writeFileSync(path.join(full, 'inner'), '')

// mkdtemp puts six random characters into the name it reports.
const clean = text => text.replaceAll(root, '<root>').replace(/tmp-[A-Za-z0-9]{6}'/, "tmp-XXXXXX'")
const shape = error => ({ code: error.code, errno: error.errno, syscall: error.syscall, message: clean(error.message) })
const fsCases = {
  'readFile missing': () => fsp.readFile(missing, 'utf8'),
  'readFile directory': () => fsp.readFile(dir, 'utf8'),
  'readFile under a file': () => fsp.readFile(under, 'utf8'),
  'writeFile into missing directory': () => fsp.writeFile(path.join(missing, 'x'), ''),
  'writeFile onto directory': () => fsp.writeFile(dir, ''),
  'writeFile exclusive onto file': () => fsp.writeFile(file, '', { flag: 'wx' }),
  'open missing': () => fsp.open(missing, 'r'),
  'mkdir existing': () => fsp.mkdir(dir),
  'mkdir under missing': () => fsp.mkdir(path.join(missing, 'x')),
  'mkdir recursive under a file': () => fsp.mkdir(path.join(under, 'y'), { recursive: true }),
  'readdir missing': () => fsp.readdir(missing),
  'readdir file': () => fsp.readdir(file),
  'opendir missing': () => fsp.opendir(missing),
  'stat missing': () => fsp.stat(missing),
  'lstat missing': () => fsp.lstat(missing),
  'access missing': () => fsp.access(missing),
  'realpath missing': () => fsp.realpath(missing),
  'readlink file': () => fsp.readlink(file),
  'rename missing': () => fsp.rename(missing, path.join(root, 'other')),
  'unlink missing': () => fsp.unlink(missing),
  'unlink directory': () => fsp.unlink(dir),
  'rmdir not empty': () => fsp.rmdir(full),
  'rmdir missing': () => fsp.rmdir(missing),
  'rm missing': () => fsp.rm(missing),
  'rm directory without recursive': () => fsp.rm(dir),
  'copyFile missing': () => fsp.copyFile(missing, path.join(root, 'copy')),
  'link missing': () => fsp.link(missing, path.join(root, 'hard')),
  'symlink onto file': () => fsp.symlink('x', file),
  'chmod missing': () => fsp.chmod(missing, 0o600),
  'truncate missing': () => fsp.truncate(missing, 0),
  'mkdtemp under missing': () => fsp.mkdtemp(path.join(missing, 'tmp-')),
}
const fsMessages = {}
for (const [name, run] of Object.entries(fsCases)) {
  fsMessages[name] = await run().then(() => ({ ok: true }), shape)
}

const spawn = (command, args) => new Promise(resolve => {
  const report = thrown => error => resolve({
    thrownByExecFile: thrown, code: error.code, signal: error.signal ?? null, message: clean(error.message),
    hasStdout: 'stdout' in error,
  })
  try {
    execFile(command, args, { encoding: 'utf8' }, (error, stdout, stderr) => {
      if (error === null) resolve({ ok: true, stdout, stderr })
      else report(false)(Object.assign(error, { stdout, stderr }))
    })
  } catch (error) {
    report(true)(error)
  }
})
const spawnMessages = {
  'missing': await spawn(missing, []),
  'missing in PATH': await spawn('hoh-no-such-command', ['a']),
  'not executable': await spawn(file, []),
  'directory': await spawn(dir, []),
  'under a file': await spawn(under, []),
  'exit 3': await spawn('/bin/sh', ['-c', 'echo out; echo err >&2; exit 3']),
  'killed by TERM': await spawn('/bin/sh', ['-c', 'kill -TERM $$']),
}

fs.writeFileSync(target, JSON.stringify({ table, fsMessages, spawnMessages }, undefined, 1) + '\n')
console.log(`${table.length} system errors, ${Object.keys(fsMessages).length} fs cases, ${Object.keys(spawnMessages).length} spawn cases`)
