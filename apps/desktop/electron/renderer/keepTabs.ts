// The open tabs live in the page's localStorage (src/state/sessionTabs.ts), which Chromium commits late: each
// change asks for a commit, so a crash right after it still finds the tabs as they were.
export async function keepTabsOnDisk(): Promise<void> {
  const { useSessionTabs } = await import("@/state/sessionTabs");
  useSessionTabs.subscribe(() => window.brigadierShell.flushStorage());
}
