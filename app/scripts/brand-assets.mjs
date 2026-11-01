// Copies the GitHop brand kit (brand/) into the app: Tauri bundle icons and
// the SVGs the UI imports. The kit is the single source; never hand-edit the
// copies, re-run this script instead (brand/README.md "Regenerating").
//
//   npm --prefix app run icon
//
// Local only: Node built-ins + the Tauri CLI that is already a devDependency.
// Processes are spawned with argument arrays, never a shell string.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const brand = join(app, "..", "brand");
const icons = join(app, "src-tauri", "icons");
const uiBrand = join(app, "ui", "src", "assets", "brand");

// 1. Tauri bundle icons: only the files tauri.conf.json references.
const tmp = mkdtempSync(join(tmpdir(), "githop-icons-"));
try {
  const tauri = createRequire(import.meta.url).resolve("@tauri-apps/cli/tauri.js");
  const master = join(brand, "png", "icon-light", "githop-icon-light-1024.png");
  const r = spawnSync(process.execPath, [tauri, "icon", master, "-o", tmp], { stdio: "inherit" });
  if (r.status !== 0) throw new Error(`tauri icon failed (exit ${r.status})`);
  for (const f of ["128x128.png", "128x128@2x.png", "icon.png"]) {
    copyFileSync(join(tmp, f), join(icons, f));
  }
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
// 32 px and below use the heavier favicon geometry (brand/README.md).
copyFileSync(join(brand, "png", "icon-light", "githop-icon-light-32.png"), join(icons, "32x32.png"));
// Windows app + installer icon: 16–256 px, favicon geometry at ≤32 px.
copyFileSync(join(brand, "ico", "githop.ico"), join(icons, "icon.ico"));

// 2. UI SVGs (bundled by Vite; nothing loads from the network).
mkdirSync(uiBrand, { recursive: true });
const svgs = [
  "githop-favicon.svg",
  "githop-icon-light.svg",
  "githop-lockup-horizontal.svg",
  "githop-lockup-horizontal-reverse.svg",
  "githop-mark-color.svg",
  "githop-mark-reverse.svg",
  "githop-mark-mono-ink.svg",
  "githop-mark-mono-white.svg",
];
for (const f of svgs) copyFileSync(join(brand, "svg", f), join(uiBrand, f));

// The kit's favicon geometry (loop stroke 7 instead of 5.5 on the 64 grid) is
// the one to use at ≤32 px. The kit has no small lockup, so derive one the same
// way: identical file, only the loop stroke swapped. Fails loudly if the kit's
// markup changes so the swap can't be applied exactly once.
const LOOP = 'stroke-width="5.5" stroke-linecap="round"';
const SMALL_LOOP = 'stroke-width="7" stroke-linecap="round"';
for (const f of ["githop-lockup-horizontal", "githop-lockup-horizontal-reverse"]) {
  const src = readFileSync(join(brand, "svg", `${f}.svg`), "utf8");
  if (src.split(LOOP).length !== 2) throw new Error(`${f}.svg: expected one loop stroke`);
  writeFileSync(join(uiBrand, `${f}-small.svg`), src.replace(LOOP, SMALL_LOOP));
}

console.log(`Brand icons -> ${icons}\nBrand SVGs  -> ${uiBrand}`);
