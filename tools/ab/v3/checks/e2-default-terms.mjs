// e2: `invoice` without --terms makes the invoice due 14 days after its date; --terms still wins.
// usage: node e2-default-terms.mjs <result-dir>
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const dir = process.argv[2];
const { run } = await import(`${dir}/src/cli.ts`);
function invoice(extra) {
  const tmp = mkdtempSync(join(tmpdir(), 'v3-e2-'));
  try {
    const data = join(tmp, 'data.json');
    copyFileSync(join(dir, 'examples/data.json'), data);
    let out = '';
    const code = run(['invoice', '--data', data, '--cart', join(dir, 'examples/cart.json'), '--region', 'ON',
      '--date', '2024-03-01', ...extra], { out: (t) => { out += t; }, err: (t) => { out += t; } });
    assert.equal(code, 0, out);
    return out;
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}
assert.match(invoice([]), /Due date:\s+2024-03-15/);
assert.match(invoice(['--terms', '30']), /Due date:\s+2024-03-31/);
console.log('e2 ok');
