// Writes node_path.jsonl: what Node's `path.posix` returns, one call per line: {"fn","args","out"}.
// Owned by the lead; run with Node 22:
//   node crates/hoh-util-workspace-path/tests/fixtures/gen-node-path.mjs crates/hoh-util-workspace-path/tests/fixtures/node_path.jsonl
// `resolve` and `relative` of Node read the process's directory; here it is always given: `resolve` gets it
// as its first argument, and `relative` is recorded as [cwd, from, to].
import { writeFileSync } from 'node:fs'
import { posix } from 'node:path'

const parts = [
  '', '.', '..', '/', '//', '///', 'a', 'a/', '/a', '/a/', 'a/b', 'a//b', './a', '../a', 'a/..', 'a/../..', '/..',
  '/a/../..', 'a/.', 'a/./b', '.a', 'a.', 'a.txt', 'a.b.txt', '.hidden.txt', 'dir.d/file', 'dir.d/', '/a/b/c.d',
  'a b', 'ü/ф.txt', '...', 'a/...', '..a', 'a\\b', '/a/b/../../../c', 'a/b/c/../..', './', '../', '../..', '/./',
]
const few = ['', '.', '..', '/', 'a', '/b', 'c/', '../d']
const cwds = ['/home/u/w', '/']

const lines = []
const call = (fn, args, out) => lines.push(JSON.stringify({ fn, args, out }))

for (const p of parts) {
  call('normalize', [p], posix.normalize(p))
  call('dirname', [p], posix.dirname(p))
  call('basename', [p], posix.basename(p))
  call('extname', [p], posix.extname(p))
  call('isAbsolute', [p], posix.isAbsolute(p))
  call('join', [p], posix.join(p))
}
call('join', [], posix.join())
for (const a of parts) for (const b of parts) call('join', [a, b], posix.join(a, b))
for (const a of few) for (const b of few) for (const c of few) call('join', [a, b, c], posix.join(a, b, c))
for (const cwd of cwds) {
  call('resolve', [cwd], posix.resolve(cwd))
  for (const a of parts) call('resolve', [cwd, a], posix.resolve(cwd, a))
  for (const a of parts) for (const b of parts) call('resolve', [cwd, a, b], posix.resolve(cwd, a, b))
  for (const from of parts) for (const to of parts) {
    call('relative', [cwd, from, to], posix.relative(posix.resolve(cwd, from), posix.resolve(cwd, to)))
  }
}

writeFileSync(process.argv[2], lines.join('\n') + '\n')
console.log(`${lines.length} calls`)
