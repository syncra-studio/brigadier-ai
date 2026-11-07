// Pixels from the OS. A capture that doesn't come back is an error, never a pass.
import { execFile } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { promisify } from "node:util";

import { binDir, evidenceDir } from "./paths.mjs";
import { readPng } from "./png.mjs";

const run = promisify(execFile);

export function windowIdOf(mediaSourceId) {
  const match = /^window:(\d+):/.exec(mediaSourceId);
  if (!match) throw new Error(`not a window source: ${mediaSourceId}`);
  return Number(match[1]);
}

/** Captures one window by its OS id, wherever it is and whatever covers it. */
export async function capture(windowId, name, folder = evidenceDir) {
  fs.mkdirSync(folder, { recursive: true });
  const file = path.join(folder, `${name}.png`);
  fs.rmSync(file, { force: true });
  await run("/usr/sbin/screencapture", ["-x", "-o", "-l", String(windowId), file]);
  if (!fs.existsSync(file) || fs.statSync(file).size === 0) throw new Error(`no capture of window ${windowId}`);
  return { file, image: readPng(file) };
}

export async function osWindows(pid) {
  const { stdout } = await run(path.join(binDir, "windows"), pid ? ["--pid", String(pid)] : [], { maxBuffer: 1 << 24 });
  return stdout.trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

/** Records a window to an mp4, with a .times file giving each frame's moment. */
export function record(windowId, seconds, out, fps = 60) {
  return run(path.join(binDir, "winrec"), [String(windowId), String(seconds), out, String(fps)], { maxBuffer: 1 << 24 });
}

export async function framesOf(video, { x, y, width, height }) {
  // Cropped raw frames, read back as RGB: enough for flash and tearing checks on long recordings.
  const { stdout } = await run("/opt/homebrew/bin/ffmpeg", ["-hide_banner", "-loglevel", "error", "-i", video, "-vf", `crop=${width}:${height}:${x}:${y}`, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], { encoding: "buffer", maxBuffer: 1 << 30 });
  const size = width * height * 3;
  const frames = [];
  for (let at = 0; at + size <= stdout.length; at += size) frames.push(stdout.subarray(at, at + size));
  return frames;
}
