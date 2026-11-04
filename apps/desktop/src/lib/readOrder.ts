/**
 * A newer change wins over a late read. Each key's reads and changes are numbered as they start:
 * a read that answers after a newer read or change of its key started is not applied. So a read
 * answering after a delete never brings the deleted back, and two reads answering in reverse
 * order never leave the older one showing.
 */
export type ReadOrder<K> = {
  /** Starts a read of `key`; `current()` while nothing newer happened to `key` since. */
  read(key: K): () => boolean;
  /** `key` changed (a delete, an edit heard live): reads of it started before no longer apply. */
  change(key: K): void;
};

export function readOrder<K = string>(): ReadOrder<K> {
  const latest = new Map<K, number>();
  let next = 0;
  return {
    read(key) {
      const mine = ++next;
      latest.set(key, mine);
      return () => latest.get(key) === mine;
    },
    change(key) {
      latest.set(key, ++next);
    },
  };
}
