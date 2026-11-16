// Generates the fixtures for js_json_parse and js_json_stringify from the JSON of the Node that runs it.
// Usage: node gen-js-json.mjs <output directory>
// Output, one JSON value per line, the first line is {"node": "<version>"}:
//   js_json_roundtrip.jsonl      [text, JSON.stringify(JSON.parse(text)), JSON.stringify(JSON.parse(text), null, 2)]
//   js_json_parse_errors.jsonl   [text, message of the SyntaxError thrown by JSON.parse(text)]
// Deterministic: a fixed-seed xorshift generator, so the files can be regenerated.
import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

const dir = process.argv[2]
let state = 0x2545f4914f6cdd1dn
const next = () => {
  state ^= (state << 13n) & 0xffffffffffffffffn
  state ^= state >> 7n
  state ^= (state << 17n) & 0xffffffffffffffffn
  return state
}
const int = (n) => Number(next() % BigInt(n))
const pick = (list) => list[int(list.length)]

const numbers = ['0', '-0', '1', '-1', '1.0', '1.50', '0.1', '-0.0', '1e0', '1E5', '1e+5', '1e-5', '1e20', '1e21', '1e-6', '1e-7',
  '123456789012345678901234567890', '9007199254740991', '9007199254740992', '9007199254740993', '-9007199254740993',
  '9223372036854775807', '9223372036854775808', '18446744073709551615', '18446744073709551616', '1.7976931348623157e308',
  '5e-324', '1e-400', '0.000001', '0.0000001', '100000000000000000000', '1000000000000000000000', '4.35',
  '0.30000000000000004', '2.5e-8', '123.456e3', '0.1e1', '12345678.9', '1.5e300', '-1.5E-300', '2e0', '10.0e-1', '0e0', '-0e5']
const strings = ['""', '"a"', '"\\u0041"', '"\\/"', '"/"', '"\\b\\f\\n\\r\\t"', '"\\u0000"', '"\\u001f"', '"\\u007f"', '"\u007f"',
  '"\\u2028\\u2029"', '" "', '"привет"', '"\\u043f\\u0440"', '"😀"', '"\\ud83d\\ude00"', '"\\u00e9"', '"\\"quoted\\""',
  '"back\\\\slash"', '"<script>&amp;"', '"tab\\there"', '"\\u000b"', '"\\u0010\\u001F"', '"é"', '"a\\u0020b"']
const keys = ['"a"', '"b"', '"c"', '"0"', '"1"', '"2"', '"10"', '"01"', '"-1"', '"1.5"', '""', '"4294967294"', '"4294967295"',
  '"__proto__"', '"constructor"', '"length"', '"k\\u0065y"', '"ключ"', '"007"', '"9"', '"1e3"', '" 3"']
const spaces = ['', '', '', ' ', '\n', '\t', '\r\n', '  ']

function value(depth) {
  const kind = depth > 5 ? int(4) : int(7)
  if (kind === 0) return pick(numbers)
  if (kind === 1) return pick(strings)
  if (kind === 2) return pick(['true', 'false', 'null'])
  if (kind === 3) return String(int(2000) - 1000) + (int(3) === 0 ? '.' + String(int(1000)) : '')
  if (kind === 4 || kind === 5) {
    const items = []
    for (let i = int(5); i > 0; i--) items.push(pick(spaces) + value(depth + 1) + pick(spaces))
    return '[' + (items.length === 0 ? pick(spaces) : items.join(',')) + ']'
  }
  const members = []
  for (let i = int(6); i > 0; i--) members.push(pick(spaces) + pick(keys) + pick(spaces) + ':' + pick(spaces) + value(depth + 1) + pick(spaces))
  return '{' + (members.length === 0 ? pick(spaces) : members.join(',')) + '}'
}

const fixed = [...numbers, ...strings, 'true', 'false', 'null', '[]', '{}', '[ ]', '{ }', ' 1 ', '\n[1,2]\n',
  '{"b":1,"a":2}', '{"2":1,"1":2,"b":3,"01":4,"-1":5,"4294967294":6,"4294967295":7,"1.5":8,"":9,"0":10}',
  '{"a":1,"b":2,"a":3}', '{"1":"x","a":{"3":1,"2":2,"z":3},"0":[{"b":1,"5":2}]}', '{"__proto__":1,"a":2}',
  '[[],{},[[]],[{}],{"a":[]},{"a":{}}]', '[1,"a",null,true,false,{"a":[1,2,{"b":null}]}]',
  '{"a":1e400,"b":-1e400}', '[1e400]', '[' + '['.repeat(40) + ']'.repeat(40) + ']']
const roundtrip = [JSON.stringify({ node: process.version })]
const texts = [...fixed]
for (let i = 0; i < 2000; i++) texts.push(pick(spaces) + value(0) + pick(spaces))
for (const text of texts) {
  const parsed = JSON.parse(text)
  roundtrip.push(JSON.stringify([text, JSON.stringify(parsed), JSON.stringify(parsed, null, 2)]))
}
writeFileSync(join(dir, 'js_json_roundtrip.jsonl'), roundtrip.join('\n') + '\n')

const broken = ['', ' ', '\n', '{', '}', '[', ']', '[1,]', '{"a":1,}', '{a:1}', "{'a':1}", '{"a" 1}', '{"a":}', '{"a"}', '[1 2]', '01',
  '1.', '.5', '-', '+1', '1e', '1e+', '1.e3', 'tru', 'nul', 'fals', 'undefined', 'NaN', 'Infinity', '-Infinity', '"abc', '"\\x"',
  '"\\u12"', '"\\u12g4"', '"a\nb"', '"\t"', '"\u0001"', '{"a":1}x', '[1]]', 'null null', '﻿1', '/*c*/1', '1 // c', '[,]',
  '[1,,2]', '{,}', '{"a":1,,}', ' 1', '0x10', '1_000', '--1', '1e1.5', '{"a":[1,2}', '[{"a":1]', 'true false', 'a', 'abc',
  'abc def ghi jkl mno pqr stu vwx yz', '{"key": "value", "other": tru}', '[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, x, 14, 15, 16, 17, 18, 19, 20]',
  '{"a":1 "b":2}', '{"a":1;"b":2}', '[1;2]', '"a" "b"', '{"a":"b" "c"}', '{1:2}', '{"a":01}', '[-]', '[1.5.5]', '{"a":"\\q"}',
  '"\\', '"\\u', '{"a":"x', '{"a', '{"', '[tru', '[nul', '[1,', '{"a":', 'привет', '{"a":привет}', '😀', '["😀" x]', '[1] x',
  '{"a":1}\n\n{"b":2}', '{\n  "a": 1,\n  "b": ,\n}', '[\n1,\n2,\n,\n]', 'x'.repeat(40), '[' + '1,'.repeat(30) + 'x]', '{"a":"' + 'y'.repeat(50) + '"x}']
const errors = [JSON.stringify({ node: process.version })]
const seen = new Set()
const record = (text) => {
  if (seen.has(text)) return
  seen.add(text)
  try {
    JSON.parse(text)
  } catch (error) {
    errors.push(JSON.stringify([text, error.message]))
  }
}
broken.forEach(record)
const junk = ['x', ',', ':', '"', '{', '}', '[', ']', ' ', '\n', '-', '.', 'e', '0', '\\', '\t', 'é', '/']
for (let i = 0; i < 4000 && errors.length < 1800; i++) {
  const text = value(1)
  const at = int(text.length + 1)
  const kind = int(3)
  if (kind === 0) record(text.slice(0, at) + text.slice(at + 1))
  else if (kind === 1) record(text.slice(0, at) + pick(junk) + text.slice(at))
  else record(text.slice(0, at))
}
writeFileSync(join(dir, 'js_json_parse_errors.jsonl'), errors.join('\n') + '\n')
console.log('roundtrip', roundtrip.length - 1, 'errors', errors.length - 1)
