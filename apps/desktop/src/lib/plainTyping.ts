/**
 * What a field that takes commands, code, paths or names sets so that what's typed is what's
 * sent: no spelling or auto-correct, no capitals, no autofill, and no macOS inline predictions.
 * WebKit offers those in any editable element that allows writing suggestions, whatever the
 * webview's own setting, and one can end up in the text: `seq 1` became `sequence 1`, `for i in`
 * `for instance in` (#546). Only `writingsuggestions="false"` turns them off, for the element
 * and everything in it.
 */
export const PLAIN_FIELD = {
  spellCheck: false,
  autoCorrect: "off",
  autoCapitalize: "off",
  autoComplete: "off",
  writingsuggestions: "false",
} as const;

declare module "react" {
  interface HTMLAttributes<T> {
    /** Whether the browser may offer writing suggestions (macOS inline predictions) here. */
    writingsuggestions?: "true" | "false" | "" | undefined;
  }
}
