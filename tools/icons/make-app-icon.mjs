// Writes src-tauri/icons/app-icon.png (1024x1024): a #1F6FEB rounded square with white
// signal bars. Node built-ins only. Then run, from the repo root:
//   pnpm tauri icon src-tauri/icons/app-icon.png
import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const N = 1024;
const OUT = resolve(dirname(fileURLToPath(import.meta.url)), '../../src-tauri/icons/app-icon.png');
const BLUE = [0x1f, 0x6f, 0xeb];
const WHITE = [255, 255, 255];
const R = 224; // corner radius
const M = 64; // margin around the square

// Signed distance to a rounded rect (negative inside), centre cx,cy, half-size hw,hh.
function sdRound(x, y, cx, cy, hw, hh, r) {
  const qx = Math.abs(x - cx) - (hw - r);
  const qy = Math.abs(y - cy) - (hh - r);
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
}
const cover = (d) => Math.min(1, Math.max(0, 0.5 - d)); // 1px anti-aliasing

// Four ascending bars.
const bars = [0, 1, 2, 3].map((i) => {
  const w = 120, gap = 52, total = 4 * w + 3 * gap;
  const left = (N - total) / 2 + i * (w + gap);
  const h = 190 + i * 130;
  const bottom = N - 250;
  return { cx: left + w / 2, cy: bottom - h / 2, hw: w / 2, hh: h / 2 };
});

const raw = Buffer.alloc((N * 4 + 1) * N);
for (let y = 0; y < N; y++) {
  raw[y * (N * 4 + 1)] = 0; // filter: none
  for (let x = 0; x < N; x++) {
    const px = x + 0.5, py = y + 0.5;
    const a = cover(sdRound(px, py, N / 2, N / 2, N / 2 - M, N / 2 - M, R));
    let c = BLUE;
    let mix = 0;
    for (const b of bars) mix = Math.max(mix, cover(sdRound(px, py, b.cx, b.cy, b.hw, b.hh, 28)));
    c = BLUE.map((v, k) => Math.round(v + (WHITE[k] - v) * mix));
    const o = y * (N * 4 + 1) + 1 + x * 4;
    raw[o] = c[0]; raw[o + 1] = c[1]; raw[o + 2] = c[2]; raw[o + 3] = Math.round(a * 255);
  }
}

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(N, 0); ihdr.writeUInt32BE(N, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, png);
console.log(`wrote ${OUT} (${png.length} bytes)`);
