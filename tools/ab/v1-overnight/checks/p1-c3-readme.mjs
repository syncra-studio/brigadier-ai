// p1-c3: the README's Usage shows an invoice command with --note. No suite covers it.
// usage: node p1-c3-readme.mjs <result-dir>
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const text = readFileSync(join(process.argv[2], 'README.md'), 'utf8');
const usage = text.split(/^## Usage\s*$/m)[1];
assert.ok(usage, 'the README has no Usage section');
const section = usage.split(/^## /m)[0];
assert.ok(
  section.split('\n').some((line) => /\binvoice\b/.test(line) && /--note\b/.test(line)),
  `no invoice command with --note in the README's Usage:\n${section}`,
);
console.log('p1-c3 ok');
