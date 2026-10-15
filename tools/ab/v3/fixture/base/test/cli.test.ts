import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { run, USAGE } from '../src/cli.ts';
import { PRODUCTS, withTempDir } from './helpers.ts';

const CLI = fileURLToPath(new URL('../src/cli.ts', import.meta.url));

function capture(argv: string[]) {
  let stdout = '';
  let stderr = '';
  const code = run(argv, { out: (t) => (stdout += t), err: (t) => (stderr += t) });
  return { code, stdout, stderr };
}

/** Writes a data file and a cart into `dir` and returns their paths. */
function setup(dir: string) {
  const data = join(dir, 'data.json');
  const cart = join(dir, 'cart.json');
  writeFileSync(data, JSON.stringify({ products: PRODUCTS, invoices: [] }));
  writeFileSync(cart, JSON.stringify({ lines: [{ sku: 'WID-001', qty: 2 }, { sku: 'TEA-200', qty: 1 }] }));
  return { data, cart };
}

describe('cli', () => {
  test('prints usage', () => {
    assert.deepEqual(capture([]), { code: 0, stdout: USAGE, stderr: '' });
    assert.equal(capture(['--help']).stdout, USAGE);
  });

  test('products lists the catalog', () => {
    withTempDir((dir) => {
      const { data } = setup(dir);
      const { code, stdout } = capture(['products', '--data', data]);
      assert.equal(code, 0);
      assert.equal(
        stdout,
        'BOOK-010  Field Guide to Ledgers           34.50  reduced\n' +
          'CBL-030   USB-C Cable 2m                   12.40  standard\n' +
          'TEA-200   Loose Leaf Tea 250g               8.75  exempt\n' +
          'WID-001   Widget                           19.99  standard\n',
      );
    });
  });

  test('invoice saves the invoice and prints it', () => {
    withTempDir((dir) => {
      const { data, cart } = setup(dir);
      const args = ['invoice', '--data', data, '--cart', cart, '--region', 'ON', '--date', '2024-06-01'];
      const first = capture([...args, '--terms', '14']);
      assert.equal(first.code, 0, first.stderr);
      assert.match(first.stdout, /^INVOICE INV-0001\n/);
      assert.match(first.stdout, /Due date: {3}2024-06-15\n/);
      assert.match(first.stdout, /Total: +53\.93\n$/);
      assert.match(capture(args).stdout, /^INVOICE INV-0002\n[^]*Due date: {3}2024-07-01\n/);

      const saved = JSON.parse(readFileSync(data, 'utf8'));
      assert.deepEqual(
        saved.invoices.map((i: { number: string }) => i.number),
        ['INV-0001', 'INV-0002'],
      );
    });
  });

  test('report and export read the saved invoices', () => {
    withTempDir((dir) => {
      const { data, cart } = setup(dir);
      capture(['invoice', '--data', data, '--cart', cart, '--region', 'AB', '--date', '2024-06-01']);
      const report = capture(['report', '--data', data]);
      assert.equal(report.code, 0);
      assert.match(report.stdout, /\n {3}1 {2}WID-001 {5}Widget +2 +39\.98\n/);
      assert.equal(capture(['report', '--data', data, '--from', '2024-06-02']).stdout, 'No sales.\n');

      const exported = capture(['export', '--data', data, '--format', 'json']);
      assert.equal(exported.code, 0);
      assert.equal(JSON.parse(exported.stdout)[0].total, 50.73);
    });
  });

  test('reports usage errors with exit code 2', () => {
    withTempDir((dir) => {
      const { data, cart } = setup(dir);
      assert.equal(capture(['frobnicate', '--data', data]).code, 2);
      assert.equal(capture(['products']).code, 2);
      assert.equal(capture(['products', '--bogus']).code, 2);
      const badRegion = capture(['invoice', '--data', data, '--cart', cart, '--region', 'XX', '--date', '2024-06-01']);
      assert.equal(badRegion.code, 2);
      assert.match(badRegion.stderr, /^error: unknown region: XX/);
      const badDate = capture(['report', '--data', data, '--from', 'June']);
      assert.match(badDate.stderr, /--from must be a date/);
    });
  });

  test('reports runtime errors with exit code 1', () => {
    withTempDir((dir) => {
      const { data } = setup(dir);
      const result = capture(['export', '--data', data, '--format', 'xml']);
      assert.deepEqual(result, { code: 1, stdout: '', stderr: 'error: unknown export format: xml (available: json)\n' });
      assert.equal(capture(['products', '--data', join(dir, 'missing.json')]).code, 1);
    });
  });

  test('runs as a script', () => {
    withTempDir((dir) => {
      const { data } = setup(dir);
      const result = spawnSync(process.execPath, [CLI, 'products', '--data', data], { encoding: 'utf8' });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout.split('\n').length, PRODUCTS.length + 1);
    });
  });
});
