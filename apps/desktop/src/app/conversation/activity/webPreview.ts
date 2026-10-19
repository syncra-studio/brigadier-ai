import { localUrl } from "@/state/linkRouting";

/** Web addresses in an answer, without trailing punctuation or a closing bracket. */
function addresses(text: string): string[] {
  return (text.match(/https?:\/\/[^\s<>()"'`\]]+/g) ?? []).map((url) => url.replace(/[.,;:!?]+$/, ""));
}

/**
 * The page a finished turn left for the user to try: its final answer names exactly one local
 * address with a port (`localhost:3000`, `127.0.0.1:5173`…), counted once however often it
 * appears. None, or more than one, and the answer has no web preview.
 */
export function webPreviewUrl(answer: string): string | null {
  const found = new Set<string>();
  for (const url of addresses(answer)) {
    try {
      const parsed = new URL(url);
      // The whole address: a route in its query or fragment (`/#/todos`) is the page.
      if (localUrl(url) && parsed.port) found.add(parsed.href);
    } catch {
      // Not an address after all.
    }
  }
  return found.size === 1 ? [...found][0]! : null;
}
