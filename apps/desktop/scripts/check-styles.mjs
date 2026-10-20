// The style check (`pnpm lint` runs it after oxlint): the app takes every colour and length from
// styles/tokens.css and every control from components/ui. It covers what the oxlint rule
// brigadier/no-raw-design-values (lint/brigadier-plugin.js) doesn't, which already keeps raw
// colours, lengths and Tailwind arbitrary values out of ts/tsx:
//   - CSS files other than tokens.css: no hex colours or colour functions (color-mix of tokens is
//     fine), no px/rem/em lengths but 0 and 1px/2px hairlines (%, vw and vh are layout);
//   - `style={{…}}` holds only computed values: a property set to a plain literal is a class;
//   - @openai/apps-sdk-ui and Radix are imported only inside components/ui (the kit's icons only
//     in components/icons/kit.ts);
//   - icons live in components/icons: no <svg> markup anywhere else (charts and rings drawn from
//     live values are listed in check-styles.allow as `<svg`).
// What can't follow yet is listed in check-styles.allow, one file per line with the exact values
// it may keep and why: `<path from src> <value> [<value>…]  # reason`. An entry no longer needed
// fails the check too, so the list only shrinks.
//
// node scripts/check-styles.mjs [src dir] [allowlist]
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
const root = process.argv[2] ?? here("../src");
const allowFile = process.argv[3] ?? here("check-styles.allow");

const HEX = /#[0-9a-f]{3,8}\b/gi;
const COLOR_FUNCTION = /\b(?:rgba?|hsla?|hwb|oklch|oklab|lab|lch|color)\(/gi;
const LENGTH = /(?<![\w.#-])-?(\d*\.?\d+)(?:px|rem|em|ch|ex|lh|pt|pc|cm|mm|in|q)\b/gi;
const HAIRLINE = /^-?[12]px$/;
const KIT_IMPORT = /(?:\bfrom\s*|\bimport\s*\(?\s*)["']((?:@openai\/apps-sdk-ui|radix-ui|@radix-ui\/)[^"']*)["']/g;
const KIT_ICONS = "@openai/apps-sdk-ui/components/Icon";
const SVG = /<svg\b/g;
const STATIC_STYLE = /(?:^|[{,])\s*([\w$]+|"[^"]+")\s*:\s*(?:"[^"$`]*"|'[^']*'|-?\d*\.?\d+)\s*(?=[,}]|$)/g;

/** Every source file under `dir`, without fixtures and tests. */
function files(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "fixtures" ? [] : files(path);
    return /\.(css|tsx?)$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name) ? [path] : [];
  });
}

/** The text of each `style={{…}}` object in `text`. */
function styleObjects(text) {
  const objects = [];
  for (let at = text.indexOf("style={{"); at !== -1; at = text.indexOf("style={{", at + 1)) {
    let depth = 0;
    for (let i = at + 7; i < text.length; i++) {
      if (text[i] === "{") depth++;
      else if (text[i] === "}" && --depth === 0) {
        objects.push(text.slice(at + 8, i));
        break;
      }
    }
  }
  return objects;
}

/** The raw values in one file, as `[value, why]` pairs. */
export function problems(path, text) {
  const found = [];
  if (path.endsWith(".css")) {
    if (path.endsWith("styles/tokens.css")) return found;
    const css = text.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const [hex] of css.matchAll(HEX)) found.push([hex, "a raw colour: use a colour token"]);
    for (const [fn] of css.matchAll(COLOR_FUNCTION)) found.push([fn, "a raw colour: use a colour token"]);
    for (const [length, number] of css.matchAll(LENGTH)) {
      if (Number(number) !== 0 && !HAIRLINE.test(length)) found.push([length, "a raw length: use a size, text, radius or spacing token"]);
    }
    return found;
  }
  if (path !== "components/icons/kit.ts") {
    for (const [, name] of text.matchAll(KIT_IMPORT)) {
      if (name.startsWith(KIT_ICONS)) found.push([name, "a kit import: icons come from components/icons"]);
      else if (!path.includes("components/ui/")) found.push([name, "a kit import: use the components/ui wrapper"]);
    }
  }
  if (!path.startsWith("components/icons/")) {
    for (const [svg] of text.matchAll(SVG)) found.push([svg, "icon markup: icons live in components/icons"]);
  }
  for (const object of styleObjects(text)) {
    for (const [pair, key] of object.matchAll(STATIC_STYLE)) {
      if (/:\s*-?0*\.?0+\s*$/.test(pair)) continue;
      found.push([`style.${key.replaceAll('"', "")}`, "a fixed inline style: use a class (style={{}} is for computed values)"]);
    }
  }
  return found;
}

function main() {
  const allowed = new Map();
  for (const line of readFileSync(allowFile, "utf8").split("\n")) {
    if (!line.trim() || line.trimStart().startsWith("#")) continue;
    const entry = line.match(/^(\S+)\s+(.+?)\s+#\s*\S/);
    if (!entry) throw new Error(`check-styles.allow: "${line}" needs a path, its values and a # reason`);
    allowed.set(entry[1], new Set(entry[2].split(/\s+/)));
  }
  const errors = [];
  const used = new Set();
  for (const path of files(root)) {
    const name = relative(root, path).split("\\").join("/");
    const allow = allowed.get(name) ?? new Set();
    for (const [value, why] of problems(name, readFileSync(path, "utf8"))) {
      if (allow.has(value)) used.add(`${name} ${value}`);
      else errors.push(`src/${name}: "${value}" is ${why}`);
    }
  }
  for (const [name, values] of allowed) {
    for (const value of values) {
      if (!used.has(`${name} ${value}`)) errors.push(`check-styles.allow: src/${name} no longer has "${value}"; take it off the list`);
    }
  }
  for (const error of new Set(errors)) console.error(error);
  if (errors.length > 0) process.exit(1);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
