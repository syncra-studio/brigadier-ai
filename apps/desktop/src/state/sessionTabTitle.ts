import { pastedTextLabel } from "@/components/conversation/elements/chipText";
import { type SessionTab, sideChatName } from "@/state/sessionTabs";

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

type CheckoutTab = Extract<SessionTab, { kind: "file" | "review" | "conflict" }>;

function inCheckout(tab: SessionTab): tab is CheckoutTab {
  return tab.kind === "file" || tab.kind === "review" || tab.kind === "conflict";
}

function pathOf(tab: CheckoutTab): string {
  if (tab.kind !== "review") return tab.path;
  return tab.target.type === "file" ? tab.target.path : "path" in tab.target ? tab.target.path ?? "" : "";
}

function folderOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

/**
 * What tells a tab from another open one of the same name, as VS Code does, dim after the name:
 * the checkout it reads, or within one checkout the file's folder. Null while no other shares its
 * name. `root` is the folder of a tab that names no checkout of its own.
 */
export function tabNameNote(tab: SessionTab, tabs: readonly SessionTab[], root: string | null): string | null {
  if (!inCheckout(tab)) return null;
  const name = tabTitle(tab, tabs);
  const same = tabs.filter((each): each is CheckoutTab => inCheckout(each) && tabTitle(each, tabs) === name);
  if (same.length < 2) return null;
  const checkoutName = (each: CheckoutTab) => baseName(each.checkout ?? root ?? "");
  if (new Set(same.map(checkoutName)).size > 1) return checkoutName(tab as CheckoutTab) || null;
  return folderOf(pathOf(tab as CheckoutTab)) || null;
}

/** The name a session tab shows. */
export function tabTitle(tab: SessionTab, tabs: readonly SessionTab[]): string {
  if (tab.kind === "review") {
    // A Changes card's file row: the file's part of a commit's or a turn's change.
    if ((tab.target.type === "commit" || tab.target.type === "lastTurn" || tab.target.type === "worker") && tab.target.path) return `${baseName(tab.target.path)} (diff)`;
    if (tab.target.type === "changes") return tab.target.staged ? "Staged Changes" : "Changes";
    if (tab.target.type === "branch") return "Changes";
    if (tab.target.type === "lastTurn") return "Last turn";
    if (tab.target.type === "uncommitted") return "Uncommitted changes";
    if (tab.target.type === "worker") return "Worker changes";
    if (tab.target.type === "commit") return `Merge ${tab.target.commit.slice(0, 7)}`;
    return `${baseName(tab.target.path)} (diff)`;
  }
  if (tab.kind === "newTab") return "New tab";
  if (tab.kind === "browser") return tab.title || tab.url || "New browser";
  if (tab.kind === "terminal") return tab.title || (tab.cwd ? baseName(tab.cwd) : "Terminal");
  if (tab.kind === "sideChat") return sideChatName(tab, tabs);
  if (tab.kind === "page") return tab.title || "Untitled page";
  if (tab.kind === "mockup") return "Mockup";
  if (tab.kind === "panel") return tab.title;
  if (tab.kind === "pasted") return pastedTextLabel(tab.n);
  if (tab.kind === "conflict") {
    return tab.view === "compare" ? `${baseName(tab.path)}: Current Changes ↔ Incoming Changes`
      : tab.view === "merge" ? `Merging: ${baseName(tab.path)}` : baseName(tab.path);
  }
  return baseName(tab.path);
}
