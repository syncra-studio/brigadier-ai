import { type SessionTab, sideChatName } from "@/state/sessionTabs";

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** The name a session tab shows. */
export function tabTitle(tab: SessionTab, tabs: readonly SessionTab[]): string {
  if (tab.kind === "review") {
    if (tab.target.type === "changes") return tab.target.staged ? "Staged Changes" : "Changes";
    if (tab.target.type === "branch") return "Changes";
    if (tab.target.type === "lastTurn") return "Last turn";
    if (tab.target.type === "uncommitted") return "Uncommitted changes";
    if (tab.target.type === "commit") return `Merge ${tab.target.commit.slice(0, 7)}`;
    return `${baseName(tab.target.path)} (diff)`;
  }
  if (tab.kind === "newTab") return "New tab";
  if (tab.kind === "browser") return tab.title || tab.url || "New browser";
  if (tab.kind === "terminal") return tab.title || (tab.cwd ? baseName(tab.cwd) : "Terminal");
  if (tab.kind === "sideChat") return sideChatName(tab, tabs);
  if (tab.kind === "page") return tab.title || "Untitled page";
  if (tab.kind === "mockup") return "Mockup";
  if (tab.kind === "plan") return tab.title || "Plan";
  if (tab.kind === "panel") return tab.title;
  if (tab.kind === "conflict") {
    return tab.view === "compare" ? `${baseName(tab.path)}: Current Changes ↔ Incoming Changes`
      : tab.view === "merge" ? `Merging: ${baseName(tab.path)}` : baseName(tab.path);
  }
  return baseName(tab.path);
}
