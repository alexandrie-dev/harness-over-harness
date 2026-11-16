// Generates doubles whose exact value often lies halfway between two shortest decimals, with their String() form.
// JavaScript breaks such a tie towards the even digit (1000000000000000.25 prints as 1000000000000000.2).
// Usage: node gen-js-number-ties.mjs <output file>
// Output lines: <16 hex digits of the IEEE-754 bits>\t<String(value)>
// Deterministic: a fixed-seed xorshift generator, so the file can be regenerated.
import { writeFileSync } from 'node:fs'

const out = process.argv[2]
let state = 0x6a09e667f3bcc909n
const next = () => {
  state ^= (state << 13n) & 0xffffffffffffffffn
  state ^= state >> 7n
  state ^= (state << 17n) & 0xffffffffffffffffn
  return state
}
const buf = new DataView(new ArrayBuffer(8))
const bitsOf = (x) => {
  buf.setFloat64(0, x)
  return buf.getBigUint64(0).toString(16).padStart(16, '0')
}
const mantissa = () => Number((1n << 52n) + (next() % (1n << 52n)))

const values = [1000000000000000.25, -1000000000000000.25, 1000000000000000.75, 2000000000000000.5, 4503599627370496.5,
  1125899906842624.125, 1125899906842624.375, 562949953421312.0625, 9007199254740991, 9007199254740992, 0.5, 0.25, 0.125]
// A 53-bit integer divided by 2^k: the unit in the last place is 2^-k, so the exact decimal has k fraction digits.
for (let k = 1; k <= 24; k++) {
  for (let i = 0; i < 800; i++) {
    const x = mantissa() / 2 ** k
    values.push(i % 4 === 0 ? -x : x)
  }
}
// The same mantissas across the whole exponent range, including large integers and small fractions.
for (let i = 0; i < 6000; i++) {
  const e = Number(next() % 1900n) - 1000
  values.push(mantissa() * 2 ** e)
}
// Powers of two and their neighbours: the rounding interval is asymmetric there.
for (let e = -1070; e <= 1020; e += 7) {
  const p = 2 ** e
  values.push(p, p * (1 + 2 ** -52), p * (1 - 2 ** -53), p * 1.5, p * 1.25)
}

const lines = values.filter((x) => Number.isFinite(x) && x !== 0).map((x) => `${bitsOf(x)}\t${String(x)}`)
writeFileSync(out, lines.join('\n') + '\n')
console.log('lines', lines.length)
