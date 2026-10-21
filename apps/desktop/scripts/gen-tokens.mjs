// Writes the design tokens in styles/tokens.css from foundations.pen: the block between
// `/* generated from foundations.pen … */` and `/* end generated */` holds one declaration per Pen
// variable. Everything outside it (Tailwind's resets and @theme inline aliases, the font stacks,
// calc() and runtime values) stays hand-written.
//
// Pen files are encrypted, so the variables come from an export, scripts/pen-vars.json. To update
// it (docs/design-system.md, "Updating tokens"): with foundations.pen open in Pen, an agent runs
// the pencil MCP's execute on ~/Documents/brigadier-design/foundations.pen with the read-only
// snippet `Print(JSON.stringify(GetVariables().variables))` and writes the printed object to
// scripts/pen-vars.json. Then `node scripts/gen-tokens.mjs`.
//
// A token whose Pen value equals its current value in the block keeps its text and its comment,
// so oklch(… var(--tint)), color-mix() and rem stay as written: colours count as equal within
// one step per channel (Pen stores 8-bit hex), lengths when they're the same px. A changed token
// gets Pen's value (px as rem, colours as hex), a new one goes at the end, a removed one goes.
//
// node scripts/gen-tokens.mjs [pen-vars.json] [--check]
//   --check: change nothing, exit 1 if the block isn't what Pen gives (pnpm lint runs this).
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
const check = process.argv.includes("--check");
const varsFile = process.argv.slice(2).find((arg) => arg !== "--check") ?? here("pen-vars.json");
const tokensFile = here("../src/styles/tokens.css");

const START = "/* generated from foundations.pen — edit there, then run gen-tokens */";
const END = "/* end generated */";

// Pen variables that are not written here, and why.
const SKIP = new Map([
  ["--font-sans", "Pen can't use system fonts: Inter stands in for the stack"],
  ["--font-display", "Pen can't use system fonts: Inter stands in for the stack"],
  ["--font-mono", "Pen can't use system fonts: JetBrains Mono stands in for the stack"],
  ["--font-terminal", "Pen can't use system fonts: JetBrains Mono stands in for the stack"],
  ["--radius-capsule", "999 in Pen, 9999px in code: both fully round"],
  ["--wash-10", "Pen's stand-in for the opacity modifier foreground/10"],
  ["--warning-fill", "Pen's stand-in for the opacity modifier warning/15"],
  ["--success-fill", "Pen's stand-in for the opacity modifier success/15"],
]);

const pen = JSON.parse(readFileSync(varsFile, "utf8"));
// Read with \n line ends and written back with the file's own: a Windows checkout has \r\n, which
// the block's generated lines (joined with \n) would never match.
const raw = readFileSync(tokensFile, "utf8");
const eol = raw.includes("\r\n") ? "\r\n" : "\n";
const css = raw.replaceAll("\r\n", "\n");
// A changed colour can change what the colours mixed from it come to, so write until nothing moves.
let next = css;
for (let pass = 0; pass < 9; pass++) {
  const again = generate(next);
  if (again === next) break;
  next = again;
}

if (check) {
  if (next === css) process.exit(0);
  const before = parse(css).entries;
  const after = parse(next).entries;
  const names = [...new Set([...before.keys(), ...after.keys()])];
  const differs = names.filter((name) => before.get(name)?.value !== after.get(name)?.value);
  console.error("tokens.css is out of step with scripts/pen-vars.json (foundations.pen). Run `node scripts/gen-tokens.mjs`.");
  console.error(`  differs: ${differs.join(" ")}`);
  process.exit(1);
}
if (next !== css) writeFileSync(tokensFile, next.replaceAll("\n", eol));

/** tokens.css with the block written from Pen. */
function generate(text) {
  const { start, end, indent, entries } = parse(text);
  // Every value the file declares (the first one of a name), for resolving var().
  const declared = new Map();
  for (const [, name, value] of text.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/(--[\w-]+)\s*:\s*([^;{}]+);/g)) {
    if (!declared.has(name)) declared.set(name, value.trim());
  }
  const resolve = (value, depth = 0) =>
    depth > 9 ? value : value.replace(/var\((--[\w-]+)\)/g, (_, name) => resolve(declared.get(name) ?? "", depth + 1));
  const out = [];
  for (const [name, { lead, value }] of entries) {
    if (name in pen && !SKIP.has(name)) out.push(`${lead}${name}: ${same(pen[name], value, resolve) ? value : write(pen[name])};`);
  }
  for (const [name, variable] of Object.entries(pen)) {
    if (!entries.has(name) && !SKIP.has(name)) out.push(`\n${indent}${name}: ${write(variable)};`);
  }
  return text.slice(0, start) + out.join("") + (out.length ? "\n" : "") + indent + text.slice(end);
}

/** The block's place in the file and its declarations, each with the comment and blank lines above it. */
function parse(text) {
  const at = text.indexOf(START);
  const end = text.indexOf(END);
  if (at < 0 || end < at) throw new Error(`tokens.css has no "${START}" … "${END}" block`);
  const start = at + START.length;
  const entries = new Map();
  let lead = "";
  for (const [part, name, value] of text.slice(start, end).matchAll(/\/\*[\s\S]*?\*\/|(--[\w-]+)\s*:\s*([^;]*);|[^]/g)) {
    if (name) {
      entries.set(name, { lead, value: value.trim() });
      lead = "";
    } else lead += part;
  }
  return { start, end, indent: text.slice(text.lastIndexOf("\n", at) + 1, at), entries };
}

/** The CSS for a Pen variable. */
function write({ type, value }) {
  if (typeof value === "string" && value.startsWith("$")) return `var(${value.slice(1)})`;
  if (type === "number") return value === 0 ? "0" : `${+(value / 16).toFixed(6)}rem`;
  if (type === "color") return value.toLowerCase();
  return value;
}

/** Whether the CSS `value` already says what the Pen variable does. */
function same({ type, value: want }, value, resolve) {
  if (typeof want === "string" && want.startsWith("$")) return value === `var(${want.slice(1)})`;
  if (type === "number") {
    // A length needs its unit; only 0 may go without.
    const length = /^(-?[\d.]+)(rem|px)?$/.exec(value);
    if (!length || (!length[2] && Number(length[1]) !== 0)) return false;
    return Math.abs(Number(length[1]) * (length[2] === "rem" ? 16 : 1) - want) < 1e-6;
  }
  if (type === "color") {
    const have = toBytes(oklab(resolve(value)));
    return !!have && hexBytes(want).every((byte, i) => Math.abs(byte - have[i]) <= 1);
  }
  return value.replace(/\s+/g, " ") === String(want);
}

// Colours: CSS colour text → Oklab and alpha [L, a, b, alpha], enough of CSS for tokens.css.
function oklab(text) {
  text = text.trim();
  if (text === "transparent") return [0, 0, 0, 0];
  if (text.startsWith("#")) {
    const [r, g, b, a] = hexBytes(text);
    return [...fromLinear(...[r, g, b].map((c) => toLinear(c / 255))), a / 255];
  }
  const fn = /^([\w-]+)\(([\s\S]*)\)$/.exec(text);
  if (fn?.[1] === "oklch") {
    const [l, c, h, a = "1"] = fn[2].split(/[\s/]+/).filter(Boolean);
    const rad = (Number(h) * Math.PI) / 180;
    return [number(l), Number(c) * Math.cos(rad), Number(c) * Math.sin(rad), number(a)];
  }
  if (fn?.[1] === "color-mix") {
    const [space, ...colors] = split(fn[2]);
    if (space.trim() !== "in oklab" || colors.length !== 2) return null;
    const parts = colors.map((part) => {
      const percent = /\s(-?[\d.]+)%$/.exec(part.trim());
      return { color: oklab(percent ? part.trim().slice(0, -percent[0].length) : part), p: percent ? Number(percent[1]) / 100 : null };
    });
    const [x, y] = parts;
    if (!x.color || !y.color) return null;
    const p = x.p ?? 1 - (y.p ?? 0.5);
    const q = y.p ?? 1 - p;
    const alpha = x.color[3] * p + y.color[3] * q;
    if (alpha === 0) return [0, 0, 0, 0];
    const mix = [0, 1, 2].map((i) => (x.color[i] * x.color[3] * p + y.color[i] * y.color[3] * q) / alpha);
    return [...mix, alpha];
  }
  return null;
}

function toBytes(lab) {
  if (!lab) return null;
  const [L, a, b, alpha] = lab;
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const rgb = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ].map((c) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.abs(c) ** (1 / 2.4) * Math.sign(c) - 0.055));
  return [...rgb, alpha].map((c) => Math.round(Math.min(1, Math.max(0, c)) * 255));
}

function fromLinear(r, g, b) {
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

function toLinear(c) {
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function hexBytes(hex) {
  const digits = hex.slice(1);
  const bytes = digits.match(/../g).map((byte) => parseInt(byte, 16));
  return bytes.length === 3 ? [...bytes, 255] : bytes;
}

function number(text) {
  return text.endsWith("%") ? Number(text.slice(0, -1)) / 100 : Number(text);
}

/** Splits a function's arguments at the commas outside parentheses. */
function split(args) {
  const parts = [""];
  let depth = 0;
  for (const char of args) {
    if (char === "(") depth++;
    if (char === ")") depth--;
    if (char === "," && depth === 0) parts.push("");
    else parts[parts.length - 1] += char;
  }
  return parts;
}
