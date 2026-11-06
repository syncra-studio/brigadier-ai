// Reads a PNG's pixels, so a check judges what the OS captured without trusting anything in the app.
import fs from "node:fs";
import zlib from "node:zlib";

export function readPng(file) {
  const data = fs.readFileSync(file);
  if (data.toString("latin1", 1, 4) !== "PNG") throw new Error(`${file} is not a PNG`);
  let at = 8;
  let width = 0;
  let height = 0;
  let depth = 0;
  let colorType = 0;
  const parts = [];
  while (at < data.length) {
    const length = data.readUInt32BE(at);
    const type = data.toString("latin1", at + 4, at + 8);
    const body = data.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      depth = body[8];
      colorType = body[9];
      if (body[12] !== 0) throw new Error("interlaced PNG");
    } else if (type === "IDAT") parts.push(body);
    else if (type === "IEND") break;
    at += 12 + length;
  }
  const channels = { 2: 3, 6: 4, 0: 1 }[colorType];
  if (!channels || depth !== 8) throw new Error(`unsupported PNG ${colorType}/${depth}`);
  const raw = zlib.inflateSync(Buffer.concat(parts));
  const stride = width * channels;
  const out = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? out[y * stride + x - channels] : 0;
      const b = y > 0 ? out[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? out[(y - 1) * stride + x - channels] : 0;
      let value = line[x];
      if (filter === 1) value += a;
      else if (filter === 2) value += b;
      else if (filter === 3) value += (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        value += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      out[y * stride + x] = value & 255;
    }
  }
  return {
    width,
    height,
    channels,
    /** [r, g, b] at a pixel. */
    at(x, y) {
      const i = y * stride + x * channels;
      return channels === 1 ? [out[i], out[i], out[i]] : [out[i], out[i + 1], out[i + 2]];
    },
  };
}

/** Distinct colours on a grid inside a rectangle: 1 means a flat area, as a blank page would be. */
export function distinctColors(image, { x, y, width, height }, step = 8) {
  const seen = new Set();
  for (let j = y; j < y + height; j += step) {
    for (let i = x; i < x + width; i += step) {
      const [r, g, b] = image.at(Math.min(i, image.width - 1), Math.min(j, image.height - 1));
      seen.add((r << 16) | (g << 8) | b);
    }
  }
  return seen.size;
}

export function near(a, b, tolerance = 12) {
  return Math.abs(a[0] - b[0]) <= tolerance && Math.abs(a[1] - b[1]) <= tolerance && Math.abs(a[2] - b[2]) <= tolerance;
}
