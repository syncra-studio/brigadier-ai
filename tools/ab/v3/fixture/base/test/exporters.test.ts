import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { addItem, emptyCart } from '../src/cart.ts';
import { exportFormats, exporters, getExporter } from '../src/exporters/index.ts';
import { exportJson } from '../src/exporters/json.ts';
import { buildInvoice } from '../src/invoice.ts';
import { sampleCatalog } from './helpers.ts';

const INVOICE = buildInvoice(addItem(emptyCart(), 'TEA-200', 2), sampleCatalog(), {
  number: 'INV-0001',
  issueDate: '2024-05-01',
  termsDays: 10,
  region: 'QC',
});

describe('exporters', () => {
  test('json is pretty-printed with a trailing newline', () => {
    const text = exportJson([INVOICE]);
    assert.ok(text.startsWith('[\n  {\n    "number": "INV-0001",'));
    assert.ok(text.endsWith('}\n]\n'));
    assert.deepEqual(JSON.parse(text), [INVOICE]);
    assert.equal(exportJson([]), '[]\n');
  });

  test('the registry knows json', () => {
    assert.equal(exporters.json, exportJson);
    assert.deepEqual(exportFormats(), ['json']);
    assert.equal(getExporter('json'), exportJson);
  });

  test('unknown formats are rejected with the list of known ones', () => {
    assert.throws(() => getExporter('xml'), /unknown export format: xml \(available: json\)/);
    assert.throws(() => getExporter('constructor'), /unknown export format/);
  });
});
