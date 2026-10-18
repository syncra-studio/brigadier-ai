const STORAGE_KEY = "brigadier.summaryShown";

/** Each conversation's summary card, shown or hidden as last left; one never left starts hidden. */
export function cachedSummaryShown(): Record<string, boolean> {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    return stored && typeof stored === "object" ? (stored as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

/** Keeps every conversation's card as left for the next launch. */
export function saveSummaryShown(shown: Record<string, boolean>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(shown));
  } catch {
    // Storage can be unavailable; the cards are then kept until the app quits.
  }
}
