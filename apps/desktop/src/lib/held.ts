/**
 * The last value read of something slow (git, the file list), kept across restarts, so a view
 * opened anew shows it in its first frame and only a real change moves it. The newest `cap`
 * entries stay.
 */
export function heldMap<T>(key: string, cap: number, valid: (value: unknown) => value is T) {
  const map = new Map<string, T>();
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(key) ?? "{}");
    if (saved && typeof saved === "object") {
      for (const [k, v] of Object.entries(saved)) if (valid(v)) map.set(k, v);
    }
  } catch {
    /* Nothing held yet, or storage is unreadable: read anew. */
  }
  return {
    get: (k: string): T | undefined => map.get(k),
    set(k: string, value: T | undefined): void {
      if (JSON.stringify(map.get(k)) === JSON.stringify(value)) return;
      map.delete(k);
      if (value !== undefined) map.set(k, value);
      for (const oldest of map.keys()) {
        if (map.size <= cap) break;
        map.delete(oldest);
      }
      try {
        localStorage.setItem(key, JSON.stringify(Object.fromEntries(map)));
      } catch {
        /* Keep in memory. */
      }
    },
  };
}
