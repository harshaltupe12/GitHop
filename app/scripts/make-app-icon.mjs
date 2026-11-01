// Renders the placeholder GitHop logo (Design §6.1) to a 1024 px PNG:
// rounded square tile in `aqua` (#4ABDAC) with a "G" in `aqua-ink` (#0E3B35).
//
// Local only: Node built-ins (zlib, fs), no packages, no network.
//
//   node app/scripts/make-app-icon.mjs [out.png]
//   npx @tauri-apps/cli icon app/src-tauri/icons/app-icon.png -o app/src-tauri/icons
//
// Replace with the real mark when it is designed (Design §6.1 open item).

import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SIZE = 1024;
const AQUA = [0x4a, 0xbd, 0xac];
const AQUA_INK = [0x0e, 0x3b, 0x35];
// Design radius-sm on a 16 px tile ≈ 4 px → 25 % of the tile side.
const RADIUS = SIZE * 0.22;
const SS = 4; // supersampling per axis

function insideTile(x, y) {
  const r = RADIUS;
  const cx = Math.min(Math.max(x, r), SIZE - r);
  const cy = Math.min(Math.max(y, r), SIZE - r);
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
}

// Geometric "G": a thick ring open at the upper right, plus a bar into the centre.
function insideG(x, y) {
  const c = SIZE / 2;
  const outer = SIZE * 0.3;
  const thick = SIZE * 0.1;
  const inner = outer - thick;
  const dx = x - c;
  const dy = y - c;
  const d = Math.hypot(dx, dy);
  // Angle in degrees, 0 = right, counter-clockwise positive (screen y is down).
  const a = (Math.atan2(-dy, dx) * 180) / Math.PI;
  const ring = d <= outer && d >= inner && !(a > 0 && a < 50);
  // Horizontal bar from the centre to the ring, just below the middle line.
  const bar = dx >= 0 && dy >= 0 && dy <= thick && d <= outer;
  // Short vertical stub on the right closing the bar.
  const stub = dx >= inner && dx <= outer && dy >= 0 && dy <= outer * 0.75 && d <= outer;
  return ring || bar || stub;
}

const rgba = Buffer.alloc(SIZE * SIZE * 4);
for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    let tile = 0;
    let g = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const px = x + (sx + 0.5) / SS;
        const py = y + (sy + 0.5) / SS;
        if (insideTile(px, py)) {
          tile++;
          if (insideG(px, py)) g++;
        }
      }
    }
    const n = SS * SS;
    const i = (y * SIZE + x) * 4;
    const gf = tile ? g / tile : 0;
    for (let k = 0; k < 3; k++) {
      rgba[i + k] = Math.round(AQUA[k] * (1 - gf) + AQUA_INK[k] * gf);
    }
    rgba[i + 3] = Math.round((tile / n) * 255);
  }
}

// --- minimal PNG encoder (RGBA8, filter 0) ---
const CRC_TABLE = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const raw = Buffer.alloc((SIZE * 4 + 1) * SIZE);
for (let y = 0; y < SIZE; y++) {
  raw[y * (SIZE * 4 + 1)] = 0;
  rgba.copy(raw, y * (SIZE * 4 + 1) + 1, y * SIZE * 4, (y + 1) * SIZE * 4);
}
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const here = dirname(fileURLToPath(import.meta.url));
const out = process.argv[2] ?? join(here, "..", "src-tauri", "icons", "app-icon.png");
writeFileSync(out, png);
console.log(`Wrote ${out}`);
