import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { formatMoney, isMoney, round2, sumMoney } from '../src/money.ts';

describe('round2', () => {
  test('rounds to cents', () => {
    assert.equal(round2(1.234), 1.23);
    assert.equal(round2(1.236), 1.24);
    assert.equal(round2(10), 10);
  });

  test('rounds halves away from zero', () => {
    assert.equal(round2(0.125), 0.13);
    assert.equal(round2(-0.125), -0.13);
  });

  test('cleans up binary noise', () => {
    assert.equal(round2(0.1 + 0.2), 0.3);
  });

  test('never returns negative zero', () => {
    assert.ok(Object.is(round2(-0.001), 0));
  });

  test('rejects non-finite values', () => {
    assert.throws(() => round2(Number.NaN), RangeError);
    assert.throws(() => round2(Infinity), RangeError);
  });
});

describe('formatMoney', () => {
  test('always prints two decimals', () => {
    assert.equal(formatMoney(5), '5.00');
    assert.equal(formatMoney(12.3), '12.30');
    assert.equal(formatMoney(1234.56), '1234.56');
  });

  test('prints negative amounts with a minus sign', () => {
    assert.equal(formatMoney(-0.5), '-0.50');
    assert.equal(formatMoney(-0.001), '0.00');
  });
});

describe('sumMoney', () => {
  test('adds and rounds', () => {
    assert.equal(sumMoney([0.1, 0.2]), 0.3);
    assert.equal(sumMoney([19.99, 19.99, 19.99]), 59.97);
    assert.equal(sumMoney([]), 0);
  });
});

describe('isMoney', () => {
  test('accepts amounts with up to two decimals', () => {
    assert.ok(isMoney(0));
    assert.ok(isMoney(19.99));
    assert.ok(isMoney(-3.5));
  });

  test('rejects everything else', () => {
    assert.ok(!isMoney(1.234));
    assert.ok(!isMoney(Number.NaN));
    assert.ok(!isMoney('1.00'));
    assert.ok(!isMoney(undefined));
  });
});
