import { Branch, Folders } from "@openai/apps-sdk-ui/components/Icon";
import { createContext, lazy, Suspense, type CSSProperties, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useState } from "react";

import { TitlebarButton } from "@/components/titlebar-button";
import { SidebarResizeHandle, useSidebarWidth } from "@/components/ui/sidebar-layout";
import { CHAT_TAB, useSessionTabs } from "@/state/sessionTabs";
import { isFilesSearchKey, openWorkspacePane, setWorkspacePaneOpen, toggleWorkspacePane, useWorkspacePaneState, WORKSPACE_PANE_WIDTH_KEY, type WorkspacePaneTab } from "@/state/workspacePane";
import { useApp } from "@/state/store";

const FilesTab = lazy(() => import("@/app/conversation/FilesTab").then((m) => ({ default: m.FilesTab })));
const SourcePanel = lazy(() => import("@/app/conversation/SourcePanel").then((m) => ({ default: m.SourcePanel })));
const ProjectDraftPane = lazy(() => import("@/app/conversation/ProjectDraftPane").then((m) => ({ default: m.ProjectDraftPane })));

export function useWorkspacePane(conversationId: string | null, projectId: string | null, enabled: boolean) {
  const id = conversationId ?? `draft:${projectId ?? "none"}`;
  const open = useWorkspacePaneState((s) => s.open);
  const active = useWorkspacePaneState((s) => s.tabs[id] ?? "files");
  const size = useSidebarWidth(WORKSPACE_PANE_WIDTH_KEY);
  const [searchRequest, setSearchRequest] = useState(0);
  const [placeholder, setPlaceholder] = useState<{ id: string; tabId: string } | null>(null);
  const sessionActive = useSessionTabs((s) => conversationId ? s.sessions[conversationId]?.active ?? CHAT_TAB : CHAT_TAB);
  if (placeholder && (!open || active !== "files" || sessionActive !== placeholder.tabId || id !== placeholder.id)) setPlaceholder(null);
  const fileOpened = useCallback(() => setPlaceholder(null), []);
  const searchHandled = useCallback(() => setSearchRequest(0), []);
  const toggle = useCallback((tab: WorkspacePaneTab) => {
    if (enabled) toggleWorkspacePane(id, tab);
  }, [enabled, id]);
  const openFilesSearch = useCallback((replaceTabId?: string) => {
    if (!enabled) return;
    openWorkspacePane(id, "files");
    setSearchRequest((value) => value + 1);
    setPlaceholder(replaceTabId ? { id, tabId: replaceTabId } : null);
  }, [enabled, id]);
  const mac = useApp((s) => s.info?.platform === "macos");
  useEffect(() => {
    if (!enabled) return;
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !isFilesSearchKey(event, mac)) return;
      event.preventDefault();
      openFilesSearch();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [enabled, mac, openFilesSearch]);
  return useMemo(() => ({
    ...size, enabled, open: enabled && open, active, toggle, openFilesSearch,
    setOpen: setWorkspacePaneOpen, conversationId, projectId, searchRequest, searchHandled,
    filePlaceholder: placeholder?.id === id ? placeholder.tabId : undefined, fileOpened,
  }), [size, enabled, open, active, toggle, openFilesSearch, conversationId, projectId, searchRequest, searchHandled, placeholder, id, fileOpened]);
}

export const WorkspacePaneContext = createContext<ReturnType<typeof useWorkspacePane> | null>(null);

export function WorkspacePaneToggle({ tab }: { tab: WorkspacePaneTab }) {
  const pane = useContext(WorkspacePaneContext);
  if (!pane?.enabled) return null;
  return (
    <TitlebarButton tooltip={tab === "files" ? "Files" : "Source"}
      aria-pressed={pane.open && pane.active === tab} aria-controls="workspace-pane-content"
      onClick={() => pane.toggle(tab)}>
      {tab === "files" ? <Folders /> : <Branch />}
    </TitlebarButton>
  );
}

/** Files or Source for the open session, or before it starts for its project's folder. */
export function WorkspacePane() {
  const pane = useContext(WorkspacePaneContext);
  const open = Boolean(pane?.open);
  // While it shows, the sidebar and the right panel leave it at least its minimum width.
  useLayoutEffect(() => {
    const wrapper = open ? document.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]') : null;
    wrapper?.style.setProperty("--workspace-pane-reserve", "var(--spacing-sidebar-min)");
    return () => {
      wrapper?.style.removeProperty("--workspace-pane-reserve");
    };
  }, [open]);
  if (!pane?.open) return null;
  return (
    <aside aria-label={pane.active === "files" ? "Files" : "Source"} data-slot="workspace-pane"
      className="border-border relative flex h-full min-h-0 shrink-0 border-s"
      style={{ "--right-sidebar-width": pane.width === null ? "var(--spacing-sidebar)" : `${pane.width}px` } as CSSProperties}>
      <div id="workspace-pane-content" data-pane={pane.active}
        className="workspace-pane-width bg-sidebar text-sidebar-foreground flex h-full min-h-0 flex-col overflow-hidden">
        {/* Keyed by whose files they are: one draft's list or message never shows for another. */}
        <Suspense key={pane.conversationId ?? `project:${pane.projectId}`} fallback={null}>
          {pane.conversationId ? (
            pane.active === "files" ? <FilesTab conversationId={pane.conversationId} searchRequest={pane.searchRequest}
              onSearchHandled={pane.searchHandled} replaceTabId={pane.filePlaceholder} onFileOpened={pane.fileOpened} />
              : <SourcePanel target={{ type: "conversation", conversationId: pane.conversationId }} />
          ) : pane.projectId && <ProjectDraftPane projectId={pane.projectId} tab={pane.active}
            searchRequest={pane.searchRequest} onSearchHandled={pane.searchHandled} />}
        </Suspense>
      </div>
      <SidebarResizeHandle {...pane} side="right" label="Resize workspace pane" belowToolbar />
    </aside>
  );
}
