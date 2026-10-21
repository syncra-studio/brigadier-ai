// Generates the app icon, the menu-bar icon and every platform raster from one vector master,
// src-tauri/icons/mark.svg (the striped disc, `currentColor` on a transparent 256-unit grid).
//
//   node scripts/generate-icons.mjs
//
// icon.svg puts the mark on the charcoal plate; tray.svg is the same mark in black for the
// macOS template image. The rasters come from the Tauri CLI, and only files this project
// already owns are replaced, not the mobile scaffolding `tauri icon` also writes.
//
// icon.ico is Windows' alone (its title bar, taskbar and notification area): its 16, 20, 24 and
// 32 px entries draw the mark's small cut (src/components/icons/marks/small-mark.ts) on the plate,
// as the master's strokes are under a pixel there.

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { smallMark } from "../src/components/icons/marks/small-mark.ts";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const iconsDir = resolve(appDir, "src-tauri/icons");
// Windows' .bin/tauri is a .cmd shim, which only a shell runs: there, Node runs the CLI itself.
const tauriCli = resolve(appDir, "node_modules/@tauri-apps/cli/tauri.js");
const [tauri, tauriArgs] =
  process.platform === "win32" ? [process.execPath, [tauriCli]] : [resolve(appDir, "node_modules/.bin/tauri"), []];

const source = readFileSync(join(iconsDir, "mark.svg"), "utf8");
const mark = source.slice(source.indexOf("  <g"), source.lastIndexOf("</svg>"));

// `#181818` is the plate and `#f2f5ff` the disc's ink, brand values with no theme token behind them.
writeFileSync(
  join(iconsDir, "icon.svg"),
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><rect x="16" y="16" width="992" height="992" rx="218" fill="#181818"/><g transform="translate(184 167) scale(2.55)" color="#f2f5ff">${mark}</g></svg>`,
);
writeFileSync(
  join(iconsDir, "tray.svg"),
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" color="#000000">\n${mark}</svg>\n`,
);

/** The Windows icon's sizes drawn from the small cut. */
const SMALL_SIZES = [16, 20, 24, 32];

/** The plate at `size` px with the small cut on it: the mark takes three quarters of it (the
 *  large icon's takes 64%), so its stripes get the pixels. */
function smallIcon(size) {
  const markSize = Math.round((size * 0.75) / 2) * 2;
  const { stripes, core } = smallMark(markSize);
  const shapes = stripes.map(([x, y, w, h]) => `<rect x="${x}" y="${y}" width="${w}" height="${h}"/>`);
  shapes.push(`<circle cx="${core.cx}" cy="${core.cy}" r="${core.r}"/>`);
  const at = (size - markSize) / 2;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}"><rect width="${size}" height="${size}" rx="${(size * 218) / 1024}" fill="#181818"/><g transform="translate(${at} ${at})" fill="#f2f5ff">${shapes.join("")}</g></svg>`;
}

/** An .ico of PNG entries, in the order given. */
function ico(entries) {
  const header = Buffer.alloc(6 + 16 * entries.length);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  let offset = header.length;
  entries.forEach(({ size, png }, i) => {
    const at = 6 + 16 * i;
    header.writeUInt8(size >= 256 ? 0 : size, at);
    header.writeUInt8(size >= 256 ? 0 : size, at + 1);
    header.writeUInt16LE(1, at + 4);
    header.writeUInt16LE(32, at + 6);
    header.writeUInt32LE(png.length, at + 8);
    header.writeUInt32LE(offset, at + 12);
    offset += png.length;
  });
  return Buffer.concat([header, ...entries.map(({ png }) => png)]);
}

/** The PNG entries of an .ico `tauri icon` wrote, by size. */
function icoEntries(buffer) {
  return Array.from({ length: buffer.readUInt16LE(4) }, (_, i) => {
    const at = 6 + 16 * i;
    const start = buffer.readUInt32LE(at + 12);
    return { size: buffer[at] || 256, png: buffer.subarray(start, start + buffer.readUInt32LE(at + 8)) };
  });
}

const output = mkdtempSync(join(tmpdir(), "brigadier-icons-"));
try {
  execFileSync(tauri, [...tauriArgs, "icon", join(iconsDir, "icon.svg"), "--output", output], { stdio: "ignore" });
  for (const name of readdirSync(iconsDir))
    if (name !== "tray.png" && name !== "icon.ico" && existsSync(join(output, name)))
      copyFileSync(join(output, name), join(iconsDir, name));

  // Tauri gives the window the first entry (Windows scales it to the title bar): 32 px first,
  // as `tauri icon` orders them.
  const small = SMALL_SIZES.map((size) => {
    const svgPath = join(output, `small-${size}.svg`);
    writeFileSync(svgPath, smallIcon(size));
    execFileSync(tauri, [...tauriArgs, "icon", svgPath, "--output", join(output, `small-${size}`), "--png", String(size)], {
      stdio: "ignore",
    });
    return { size, png: readFileSync(join(output, `small-${size}`, `${size}x${size}.png`)) };
  });
  const large = icoEntries(readFileSync(join(output, "icon.ico"))).filter(({ size }) => !SMALL_SIZES.includes(size));
  writeFileSync(join(iconsDir, "icon.ico"), ico([...small.slice(-1), ...small.slice(0, -1), ...large]));

  const tray = join(output, "tray");
  execFileSync(tauri, [...tauriArgs, "icon", join(iconsDir, "tray.svg"), "--output", tray, "--png", "64"], {
    stdio: "ignore",
  });
  copyFileSync(join(tray, "64x64.png"), join(iconsDir, "tray.png"));
} finally {
  rmSync(output, { recursive: true, force: true });
}
console.log("Generated app and tray icons from src-tauri/icons/mark.svg");
