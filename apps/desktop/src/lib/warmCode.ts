/**
 * At idle after startup, what opening code needs (#410): the file and Review tabs' and the diff
 * engine's code, and both highlighters with the common grammars, so View changes opens the
 * Review tab at once and a file from the tree shows its colours with its text. Nothing waits for
 * it: a view opened before it ends loads what it needs as before.
 */
export async function warmCode(): Promise<void> {
  const [highlight, diffs] = await Promise.all([
    import("@/lib/highlight"),
    import("@/components/views/diff"),
    import("@/app/side-panel/diffView").then((view) => view.preloadDiffView()),
    import("@/app/shell/SessionTabBar").then((bar) => bar.preloadCodeTabs()),
  ]);
  await highlight.warmHighlight();
  await diffs.warmDiffs();
}
