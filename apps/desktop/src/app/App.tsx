import { useEffect } from "react";

import { AppSidebar, AppStrip, TitlebarNav } from "@/app/AppSidebar";
import { DeleteDialog } from "@/app/dialogs/DeleteDialog";
import { useLifecycleShortcuts } from "@/app/lifecycleShortcuts";
import { AddProjectDialog } from "@/app/dialogs/AddProjectDialog";
import { StorageDialog } from "@/app/dialogs/StorageDialog";
import { UninstallDialog } from "@/app/dialogs/UninstallDialog";
import { FolderDropZone } from "@/app/FolderDropZone";
import { ConversationView } from "@/app/ConversationView";
import { OnboardingDialog } from "@/app/onboarding/OnboardingDialog";
import { GlobalSearch } from "@/app/SearchDialog";
import { SettingsNav, SettingsStrip } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TerminalButton } from "@/app/conversation/SidePanel";
import { TerminalPane } from "@/app/conversation/TerminalTab";
import { useNavigationShortcuts } from "@/app/navigationShortcuts";
import { usePaneShortcuts } from "@/app/paneShortcuts";
import { runSmoke } from "@/app/smoke";
import { SidebarFoot } from "@/app/SidebarFoot";
import { TitlebarTips } from "@/components/titlebar-button";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/toast";
import { appReady, nowEpochMs } from "@/ipc/client";
import { waitForAgents } from "@/lib/agentsReady";
import { nextPaint, setFrameSampling } from "@/lib/perf";
import { markMounted, revealApp } from "@/lib/splash";
import { markStartup } from "@/lib/startup";
import { setMetricsStreaming, toggleInspector, toggleSettings } from "@/state/actions";
import { openFolderPicker } from "@/state/addProject";
import { useApp, type Selection } from "@/state/store";
import { HOME_PLACE } from "@/state/terminalPlaces";

let readyReported = false;

/** Drafts share one key: switching the composer's project must not lose the typed text. */
function viewKey(selection: Selection): string {
  switch (selection.type) {
    case "conversation":
      return selection.id;
    case "draft":
      return "draft";
    case "settings":
      return "settings";
    case "none":
      return "none";
  }
}

/** Keeps the page surface's corners round over the columns that fill it (see page-corner). */
function PageCorners() {
  return (
    <div
      aria-hidden
      className="top-titlebar start-surface-inset end-surface-inset bottom-surface-inset pointer-events-none absolute z-20"
    >
      <span className="page-corner page-corner-top-start" />
      <span className="page-corner page-corner-top-end" />
      <span className="page-corner page-corner-bottom-start" />
      <span className="page-corner page-corner-bottom-end" />
    </div>
  );
}

export function App() {
  const selection = useApp((s) => s.selection);
  const inspectorOpen = useApp(
    (s) => s.selection.type === "settings" && s.selection.page === "inspector",
  );
  const windowVisible = useApp((s) => s.windowVisible);
  const connected = useApp((s) => s.connection.status === "connected");
  const catalogLoaded = useApp((s) => s.catalogLoaded);

  useEffect(markMounted, []);
  useLifecycleShortcuts();
  usePaneShortcuts();
  useNavigationShortcuts();

  // Cold start ends when the app is usable: the loaded catalog painted, the agent CLIs ready
  // (or no longer waited for) and the startup screen gone.
  useEffect(() => {
    if (!catalogLoaded || readyReported) return;
    readyReported = true;
    markStartup("catalog");
    void nextPaint()
      .then(() => {
        markStartup("paint");
        return waitForAgents();
      })
      .then(() => {
        markStartup("agents");
        return revealApp();
      })
      .then(() => {
        markStartup("revealed");
        return appReady(nowEpochMs());
      })
      .then((coldStartMs) => {
        useApp.setState({ coldStartMs });
        if (useApp.getState().info?.smoke) return runSmoke();
      })
      .catch((error: unknown) => console.error("startup report failed", error));
  }, [catalogLoaded]);

  // Metrics stream and frame sampling run only while someone can see them.
  const sampling = inspectorOpen && windowVisible;
  useEffect(() => {
    setFrameSampling(sampling);
    return () => setFrameSampling(false);
  }, [sampling]);
  useEffect(() => {
    if (!connected) return;
    void setMetricsStreaming(sampling).catch(() => {
      // Reapplied by the shell on reconnect.
    });
  }, [sampling, connected]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.code === "KeyI" && event.altKey && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        toggleInspector();
      }
      const mac = useApp.getState().info?.platform === "macos";
      // Settings: on macOS the app menu's ⌘, does it.
      if (!mac && event.key === "," && event.ctrlKey && !event.altKey && !event.shiftKey) {
        event.preventDefault();
        toggleSettings();
      }
      // Open Folder…: on macOS the File menu's ⌘O does it.
      if (!mac && event.code === "KeyO" && event.ctrlKey && !event.altKey && !event.shiftKey) {
        event.preventDefault();
        void openFolderPicker();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="bg-chrome flex h-full flex-col">
      <SidebarProvider className="relative min-h-0 flex-1 flex-col">
        {/* The page surface the sidebar panel and the content sit on; their headers stay
            above it, in the titlebar strip. */}
        <div
          aria-hidden
          data-slot="page-surface"
          className="bg-background rounded-page shadow-page top-titlebar start-surface-inset end-surface-inset bottom-surface-inset pointer-events-none absolute"
        />
        <PageCorners />
        <div className="relative flex min-h-0 min-w-0 flex-1 ps-surface-inset pe-surface-inset pb-surface-inset">
          <SidebarPanel
            strip={selection.type === "settings" ? <SettingsStrip /> : <AppStrip />}
            foot={<SidebarFoot />}
          >
            {selection.type === "settings" ? <SettingsNav /> : <AppSidebar />}
          </SidebarPanel>
          <main className="body-divider relative flex h-full min-w-0 flex-1">
            <div className="relative flex h-full min-w-0 flex-1 flex-col">
              {selection.type === "settings" ? (
                <>
                  {/* The page's strip of the titlebar, for dragging the window: only Terminal. */}
                  <div
                    data-tauri-drag-region
                    className="h-titlebar flex shrink-0 items-center justify-end pe-1"
                  >
                    <TitlebarTips>
                      <TerminalButton />
                    </TitlebarTips>
                  </div>
                  <div className="min-h-0 flex-1">
                    <SettingsView page={selection.page} />
                  </div>
                  {/* Home's terminal, under the page. */}
                  <TerminalPane place={HOME_PLACE} />
                </>
              ) : (
                <ConversationView key={viewKey(selection)} selection={selection} />
              )}
              <Toaster className="top-titlebar pt-2" />
            </div>
          </main>
        </div>
        <TitlebarNav />
        <GlobalSearch />
        <OnboardingDialog />
        <AddProjectDialog />
        <StorageDialog />
        <UninstallDialog />
        <DeleteDialog />
        <FolderDropZone />
      </SidebarProvider>
    </div>
  );
}
