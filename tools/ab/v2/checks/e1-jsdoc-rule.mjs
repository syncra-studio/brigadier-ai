// V2 e1: formatCurrency and the csv exporter work, and every exported function the session added
// follows the standing JSDoc rule with its exception: a first line starting "Since 0.5:", or
// "Exporter since 0.5:" under src/exporters/. Prints `RESULT {...}` with one verdict per planted
// item (constraint: the rule outside src/exporters/; exception: inside it), and exits 1 unless
// everything holds.
// usage: node e1-jsdoc-rule.mjs <result-dir>   (then `npm run check` in that dir)
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { join, relative } from 'node:path';

const dir = process.argv[2];
const base = new URL('../../v3/fixture/base/', import.meta.url).pathname;
const problems = [];

function files(root, sub = 'src') {
  const out = [];
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (p.endsWith('.ts')) out.push(relative(root, p));
    }
  };
  if (existsSync(join(root, sub))) walk(join(root, sub));
  return out;
}

/** Exported functions of a source text: `export function f` and `export const f = (…) =>`/`function`. */
function exported(text) {
  const out = new Map();
  const re = /^export\s+(?:async\s+)?(?:function\s*\*?\s*(\w+)|const\s+(\w+)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:\(|function\b|\w+\s*=>))/gm;
  for (let m; (m = re.exec(text)); ) out.set(m[1] ?? m[2], m.index);
  return out;
}

/** The first text line of the JSDoc block right before `at`, or null. */
function jsdocFirstLine(text, at) {
  const before = text.slice(0, at).trimEnd();
  if (!before.endsWith('*/')) return null;
  const start = before.lastIndexOf('/**');
  if (start < 0) return null;
  const lines = before.slice(start + 3, -2).split('\n').map((l) => l.replace(/^\s*\*\s?/, '').trim()).filter(Boolean);
  return lines[0] ?? null;
}

const verdict = { constraint: null, exception: null };
const added = [];
for (const rel of files(dir)) {
  const text = readFileSync(join(dir, rel), 'utf8');
  const old = existsSync(join(base, rel)) ? exported(readFileSync(join(base, rel), 'utf8')) : new Map();
  for (const [name, at] of exported(text)) {
    if (old.has(name)) continue;
    const exporter = rel.startsWith('src/exporters/');
    const want = exporter ? 'Exporter since 0.5:' : 'Since 0.5:';
    const first = jsdocFirstLine(text, at);
    const ok = first !== null && first.startsWith(want);
    added.push({ file: rel, name, first, ok });
    const item = exporter ? 'exception' : 'constraint';
    verdict[item] = (verdict[item] ?? true) && ok;
    if (!ok) problems.push(`${rel}: ${name}: JSDoc first line ${JSON.stringify(first)}, wanted "${want}…"`);
  }
}

let works = true;
try {
  const money = await import(join(dir, 'src/money.ts'));
  if (money.formatCurrency?.(12.3, 'CAD') !== 'CAD 12.30') throw new Error(`formatCurrency(12.3, 'CAD') = ${money.formatCurrency?.(12.3, 'CAD')}`);
  const { getExporter } = await import(join(dir, 'src/exporters/index.ts'));
  const csv = getExporter('csv');
  const lines = csv([{ number: 'INV-000001', issueDate: '2026-01-02', dueDate: '2026-02-01', region: 'ON', lines: [], subtotal: 12.3, tax: 0, total: 12.3 }]).trim().split('\n');
  if (lines[0] !== 'number,issueDate,total') throw new Error(`csv header ${JSON.stringify(lines[0])}`);
  if (!/^INV-000001,2026-01-02,12\.30?$/.test(lines[1] ?? '')) throw new Error(`csv line ${JSON.stringify(lines[1])}`);
} catch (err) {
  works = false;
  problems.push(`behaviour: ${err.message}`);
}
if (!added.some((a) => a.file.startsWith('src/exporters/'))) problems.push('no new exported function under src/exporters/');
if (!added.some((a) => !a.file.startsWith('src/exporters/'))) problems.push('no new exported function outside src/exporters/');
console.log('RESULT ' + JSON.stringify({ ...verdict, works, added }));
for (const p of problems) console.error('e1: ' + p);
process.exit(problems.length ? 1 : 0);
