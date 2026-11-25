// The shared PCM worklet delivers 16 kHz mono float32 samples.
export function encodeWhisperWav(chunks: readonly Float32Array[]): string {
  const sampleCount = chunks.reduce((total, chunk) => total + chunk.length, 0);
  const buffer = new ArrayBuffer(44 + sampleCount * 2);
  const wav = new DataView(buffer);
  wav.setUint32(0, 0x52494646); // RIFF
  wav.setUint32(4, buffer.byteLength - 8, true);
  wav.setUint32(8, 0x57415645); // WAVE
  wav.setUint32(12, 0x666d7420); // fmt
  wav.setUint32(16, 16, true);
  wav.setUint16(20, 1, true); // PCM
  wav.setUint16(22, 1, true); // mono
  wav.setUint32(24, 16_000, true);
  wav.setUint32(28, 32_000, true);
  wav.setUint16(32, 2, true);
  wav.setUint16(34, 16, true);
  wav.setUint32(36, 0x64617461); // data
  wav.setUint32(40, sampleCount * 2, true);

  let offset = 44;
  for (const chunk of chunks) {
    for (const sample of chunk) {
      const clamped = Math.max(-1, Math.min(1, sample));
      wav.setInt16(offset, clamped < 0 ? clamped * 32_768 : clamped * 32_767, true);
      offset += 2;
    }
  }

  const bytes = new Uint8Array(buffer);
  const pieces: string[] = [];
  for (let start = 0; start < bytes.length; start += 8192) {
    pieces.push(String.fromCharCode(...bytes.subarray(start, start + 8192)));
  }
  return btoa(pieces.join(''));
}
