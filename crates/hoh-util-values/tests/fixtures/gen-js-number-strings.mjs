// Generates a fixture of doubles and their JavaScript String() form.
// Output lines: <16 hex digits of the IEEE-754 bits>\t<String(value)>
// Deterministic: a fixed-seed xorshift generator, so the file can be regenerated.
import { writeFileSync } from 'node:fs'

const out = process.argv[2]
let s = 0x9e3779b97f4a7c15n
const next = () => {
  s ^= s << 13n
  s &= 0xffffffffffffffffn
  s ^= s >> 7n
  s ^= s << 17n
  s &= 0xffffffffffffffffn
  return s
}
const buf = new DataView(new ArrayBuffer(8))
const bitsOf = (x) => {
  buf.setFloat64(0, x)
  return buf.getBigUint64(0).toString(16).padStart(16, '0')
}
const fromBits = (b) => {
  buf.setBigUint64(0, b)
  return buf.getFloat64(0)
}

const values = [0, -0, 1, -1, 0.1, 0.2, 0.1 + 0.2, 0.5, 1.5, -1.5, 100, 123.456, 1e21, 1e20, 999999999999999900000, 1e-6, 1e-7, 1.5e-7,
  0.0000015, 5e-324, Number.MAX_VALUE, Number.MIN_VALUE, Number.MAX_SAFE_INTEGER, 2 ** 53, 2 ** 53 + 2, 1e300, 1.2345e25, 123456789012345680000,
  4.35, 0.000001234, 1e16, 1e17, 12345678901234567890, 0.1 * 3, 1 / 3, 2 / 3, 1e-5, 1.1e-5, 9.999999e20, 1.0000001e21]
for (let e = -330; e <= 310; e++) values.push(Number(`1e${e}`), Number(`9.87654321e${e}`), Number(`-2.5e${e}`))
for (let i = 0; i < 1500; i++) {
  const x = fromBits(next())
  if (Number.isFinite(x)) values.push(x)
}
for (let i = 0; i < 1500; i++) {
  const n = Number(next() % 10000000000n)
  const d = 10 ** Number(next() % 9n)
  values.push(n / d, -n / d, n * d)
}
for (let i = 0; i < 500; i++) values.push(Number(next() % (1n << 60n)), Number(next() % 100000n) / 100)

const lines = values.filter((x) => Number.isFinite(x)).map((x) => `${bitsOf(x)}\t${String(x)}`)
writeFileSync(out, lines.join('\n') + '\n')
console.log('lines', lines.length)
