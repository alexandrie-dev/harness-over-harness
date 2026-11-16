// Writes js_white_space.json: what JavaScript itself treats as white space (`\s`, `trim`).
// Owned by the lead; run with Node 22:
//   node crates/hoh-util-values/tests/fixtures/gen-js-white-space.mjs crates/hoh-util-values/tests/fixtures/js_white_space.json
import { writeFileSync } from 'node:fs'

const whiteSpace = []
for (let cp = 0; cp <= 0x10ffff; cp++) {
  if (cp >= 0xd800 && cp <= 0xdfff) continue
  if (/^\s$/u.test(String.fromCodePoint(cp))) whiteSpace.push(cp)
}

// Characters that one of the two languages treats as white space and the other may not, plus ordinary text.
const edges = ['\u0085', '﻿', ' ', ' ', '᠎', ' ', ' ', '​', '　', '\t', '\n', '\r', '\v', '\f', ' ', '\u001c', '\u001f']
const inputs = ['', 'a', ' a ', 'a b']
for (const edge of edges) inputs.push(`${edge}a${edge}`, `${edge}${edge}`, `a${edge}b`, ` ${edge}a ${edge}`)
const trims = inputs.map(input => ({ input, trim: input.trim(), trimStart: input.trimStart(), trimEnd: input.trimEnd() }))

const text = JSON.stringify({ whiteSpace, trims }, undefined, 1)
  .replace(/[^\x20-\x7e\n]/g, ch => `\\u${ch.charCodeAt(0).toString(16).padStart(4, '0')}`)
writeFileSync(process.argv[2], text + '\n')
console.log(`${whiteSpace.length} white space code points, ${trims.length} trims`)
