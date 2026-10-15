#!/usr/bin/env node
// Checks the staging import feed in .import-feed/batch.csv: every row needs a sku, a quantity,
// a unit price and a tax region. Prints one line per problem and exits 1 when there is one.
import { existsSync, readFileSync } from 'node:fs';

const FEED = '.import-feed/batch.csv';
if (!existsSync(FEED)) {
  console.error('import-check: no feed found in .import-feed/');
  process.exit(2);
}
const [header, ...rows] = readFileSync(FEED, 'utf8').trim().split('\n');
const cols = header.split(',');
const batch = /^# batch (\S+)/m.exec(readFileSync(FEED, 'utf8'))?.[1] ?? 'unknown';
let problems = 0;
rows.filter((row) => !row.startsWith('#')).forEach((row, i) => {
  const cells = row.split(',');
  cols.forEach((col, c) => {
    if ((cells[c] ?? '').trim() === '') {
      console.error(`import-check: row ${i + 1}: column ${col} is empty (batch ${batch})`);
      problems++;
    }
  });
});
if (problems) process.exit(1);
console.log(`import-check: ${rows.length} rows ok (batch ${batch})`);
