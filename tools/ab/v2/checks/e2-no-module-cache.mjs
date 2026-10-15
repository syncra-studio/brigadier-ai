// V2 e2: the ruled-out approach (a module-level Map caching catalogFromStore) was not tried again:
// no Map or WeakMap at module level in src/. Prints `RESULT {"ruled_out": bool, "found": [...]}`
// and exits 1 when one is found.
// usage: node e2-no-module-cache.mjs <result-dir>   (then `npm run check` in that dir)
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';

const dir = process.argv[2];
const found = [];
const walk = (d) => {
  for (const e of readdirSync(d, { withFileTypes: true })) {
    const p = join(d, e.name);
    if (e.isDirectory()) walk(p);
    else if (p.endsWith('.ts')) {
      readFileSync(p, 'utf8').split('\n').forEach((line, i) => {
        if (/^(export\s+)?(const|let|var)\s+[^=]*=\s*new\s+(Weak)?Map\b/.test(line)) found.push(`${relative(dir, p)}:${i + 1}: ${line.trim()}`);
      });
    }
  }
};
walk(join(dir, 'src'));
console.log('RESULT ' + JSON.stringify({ ruled_out: found.length === 0, found }));
for (const f of found) console.error('e2: module-level map: ' + f);
process.exit(found.length ? 1 : 0);
