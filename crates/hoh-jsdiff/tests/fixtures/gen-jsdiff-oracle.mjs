// Writes jsdiff_oracle.jsonl: what jsdiff 9.0.0 itself returns for generated pairs of texts.
// tests/oracle.rs compares the port with it line by line.
//
// Usage (Node 22.13 or newer, for module.stripTypeScriptTypes):
//   node gen-jsdiff-oracle.mjs <jsdiff>/src <empty scratch dir> <out.jsonl>
// The library's TypeScript is copied into the scratch directory with types stripped and run from there;
// the reference tree itself is only read. The output is deterministic: the generator is seeded.
import { stripTypeScriptTypes } from 'node:module'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { pathToFileURL } from 'node:url'

const [source, scratch, out] = process.argv.slice(2)
for (const file of ['patch/create', 'diff/line', 'diff/base', 'util/params']) {
  const target = join(scratch, `${file}.js`)
  mkdirSync(dirname(target), { recursive: true })
  writeFileSync(target, stripTypeScriptTypes(readFileSync(join(source, `${file}.ts`), 'utf8')))
}
writeFileSync(join(scratch, 'package.json'), '{"type":"module"}\n')
const { structuredPatch, createTwoFilesPatch } = await import(pathToFileURL(join(scratch, 'patch/create.js')).href)
const { diffLines } = await import(pathToFileURL(join(scratch, 'diff/line.js')).href)

let seed = 0x5eed1e55
function random() {
  seed = (seed + 0x6d2b79f5) | 0
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed)
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296
}
const below = n => Math.floor(random() * n)
const pick = items => items[below(items.length)]

// Small pools make equal lines frequent, so several shortest edit scripts exist and the library's
// choice among them is what gets compared.
const POOLS = [
  ['a', 'b'],
  ['a', 'b', 'c'],
  ['x', 'y', 'z', '', 'w'],
  ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten', '', ' '],
  ['я', 'ё', '日本', '😀', 'a\rb', '\t', '+x', '-x', '\\ y'],
]

function length() {
  const roll = random()
  if (roll < 0.05) return 0
  if (roll < 0.4) return 1 + below(6)
  if (roll < 0.85) return 7 + below(19)
  return 26 + below(35)
}

function mutate(lines, pool) {
  const result = [...lines]
  for (let edits = 1 + below(6); edits > 0; edits--) {
    const at = below(result.length + 1)
    switch (below(5)) {
      case 0: result.splice(at, 0, pick(pool)); break
      case 1: result.splice(at, 1 + below(3)); break
      case 2: if (at < result.length) result[at] = pick(pool); break
      case 3: if (at < result.length) result.splice(at, 0, result[at]); break
      default: if (at + 1 < result.length) [result[at], result[at + 1]] = [result[at + 1], result[at]]
    }
  }
  return result
}

function text(lines, ending, final) {
  let result = ''
  lines.forEach((line, index) => {
    const eol = ending === 'mixed' ? pick(['\n', '\r\n']) : ending
    result += line + (index + 1 < lines.length || final ? eol : '')
  })
  return result
}

function pair() {
  const pool = pick(POOLS)
  const oldLines = Array.from({ length: length() }, () => pick(pool))
  const newLines = random() < 0.75 ? mutate(oldLines, pool) : Array.from({ length: length() }, () => pick(pool))
  const ending = pick(['\n', '\n', '\n', '\n', '\n', '\r\n', 'mixed'])
  const otherEnding = random() < 0.1 ? pick(['\n', '\r\n']) : ending
  return [text(oldLines, ending, random() < 0.7), text(newLines, otherEnding, random() < 0.7)]
}

const hunksOf = patch =>
  patch === undefined ? null : patch.hunks.map(h => [h.oldStart, h.oldLines, h.newStart, h.newLines, h.lines])
const changesOf = changes =>
  changes === undefined ? null : changes.map(c => [c.value, c.added, c.removed, c.count])

const records = []
for (let index = 0; index < 1500; index++) {
  const [before, after] = pair()
  const context = pick([0, 1, 2, 3, 3, 4, 4, 7])
  const max = index % 12 === 11 ? below(6) : null
  const options = max === null ? { context } : { context, maxEditLength: max }
  records.push({
    old: before,
    new: after,
    context,
    max,
    hunks: hunksOf(structuredPatch('', '', before, after, undefined, undefined, { ...options })),
    changes: changesOf(diffLines(before, after, max === null ? {} : { maxEditLength: max })),
  })
}

const NAMES = ['a.txt', 'dir/b c.txt', 'tab\there', 'quote"d', 'back\\slash', 'имя.txt', '😀', 'bell\x07', 'del\x7f', '']
for (let index = 0; index < 60; index++) {
  const [before, after] = pair()
  const oldName = pick(NAMES)
  const newName = random() < 0.4 ? oldName : pick(NAMES)
  const oldHeader = pick([undefined, '', 'old header', '2026-01-01'])
  const newHeader = pick([undefined, '', 'new header'])
  const context = pick([0, 3, 4])
  records.push({
    oldName,
    newName,
    oldHeader: oldHeader ?? null,
    newHeader: newHeader ?? null,
    old: before,
    new: after,
    context,
    text: createTwoFilesPatch(oldName, newName, before, after, oldHeader, newHeader, { context }),
  })
}

writeFileSync(out, records.map(record => JSON.stringify(record)).join('\n') + '\n')
console.log(`${records.length} records`)
