/**
 * Whether the composer is the rich field (its text Markdown, shown as formatting) or the plain
 * one. The rich field stays off until it is complete (issue #158); fixtures turn it on.
 */
let rich = false;

export function useRichComposer(): boolean {
  return rich;
}

/** For fixtures and tests: the rich field on or off. */
export function setRichComposer(on: boolean): void {
  rich = on;
}
