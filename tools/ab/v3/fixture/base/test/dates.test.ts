import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { addDays, compare, daysInMonth, format, isValidDate, maxDate, parse } from '../src/dates.ts';

describe('parse and format', () => {
  test('parses an ISO date', () => {
    assert.deepEqual(parse('2024-03-15'), { year: 2024, month: 3, day: 15 });
  });

  test('rejects malformed and impossible dates', () => {
    for (const bad of ['2024-3-15', '15/03/2024', '2024-13-01', '2024-00-10', '2024-04-31', '2023-02-29', '']) {
      assert.throws(() => parse(bad), RangeError, bad);
    }
  });

  test('formats with zero padding', () => {
    assert.equal(format({ year: 2024, month: 1, day: 5 }), '2024-01-05');
    assert.equal(format({ year: 987, month: 12, day: 31 }), '0987-12-31');
  });

  test('isValidDate', () => {
    assert.equal(isValidDate('2024-06-30'), true);
    assert.equal(isValidDate('2024-06-31'), false);
    assert.equal(isValidDate('yesterday'), false);
  });
});

describe('compare', () => {
  test('orders dates', () => {
    assert.equal(compare('2024-01-01', '2024-01-02'), -1);
    assert.equal(compare('2024-02-01', '2024-01-31'), 1);
    assert.equal(compare('2024-05-05', '2024-05-05'), 0);
  });

  test('validates both sides', () => {
    assert.throws(() => compare('2024-01-01', 'soon'), RangeError);
  });

  test('maxDate picks the later date', () => {
    assert.equal(maxDate('2024-01-01', '2023-12-31'), '2024-01-01');
    assert.equal(maxDate('2023-06-01', '2023-06-02'), '2023-06-02');
  });
});

describe('daysInMonth', () => {
  test('knows the month lengths', () => {
    assert.equal(daysInMonth(2023, 1), 31);
    assert.equal(daysInMonth(2023, 2), 28);
    assert.equal(daysInMonth(2023, 4), 30);
    assert.equal(daysInMonth(2023, 12), 31);
  });

  test('rejects invalid months', () => {
    assert.throws(() => daysInMonth(2023, 0), RangeError);
    assert.throws(() => daysInMonth(2023, 13), RangeError);
  });
});

describe('addDays', () => {
  test('stays within the month', () => {
    assert.equal(addDays('2024-03-01', 10), '2024-03-11');
    assert.equal(addDays('2024-03-15', 0), '2024-03-15');
  });

  test('crosses a month end', () => {
    assert.equal(addDays('2024-03-15', 30), '2024-04-14');
    assert.equal(addDays('2023-02-28', 1), '2023-03-01');
  });

  test('crosses a year end', () => {
    assert.equal(addDays('2023-12-20', 15), '2024-01-04');
  });

  test('goes back a few days', () => {
    assert.equal(addDays('2024-03-15', -5), '2024-03-10');
    assert.equal(addDays('2023-03-05', -10), '2023-02-23');
  });

  test('rejects fractional day counts', () => {
    assert.throws(() => addDays('2024-03-15', 1.5), RangeError);
  });
});
