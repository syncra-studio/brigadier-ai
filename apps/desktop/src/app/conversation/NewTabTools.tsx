import { Chat, Document, DotsHorizontal, Search, Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { useContext } from "react";

import { RightSidebarContext } from "@/app/conversation/RightSidebar";
import { shortcutLabel } from "@/app/conversation/SidePanel";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { useCheckoutRoot } from "@/components/assistant-ui/file-links";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Kbd } from "@/components/ui/kbd";
import { openReviewTab, replaceNewTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { setTerminalOpen } from "@/state/terminalPlaces";

/** A tool consumes the New tab in place; Find file waits for the user to pick a file. */
export function NewTabTools({ conversationId, tabId }: { conversationId: string; tabId: string }) {
  const sidebar = useContext(RightSidebarContext);
  const cwd = useCheckoutRoot();
  const mac = useApp((s) => s.info?.platform === "macos");
  const archived = useApp((s) => s.conversations[conversationId]?.lifecycle === "archived");
  const tools = [
    { label: "Review changes", Icon: DiffGlyph, shortcut: "⌃⇧G", run: () => openReviewTab(conversationId, { type: "all" }, tabId) },
    { label: "Terminal", Icon: Terminal, shortcut: "⌃`", disabled: archived, run: () => replaceNewTab(conversationId, tabId, "terminal", cwd) },
    { label: "Find file", Icon: Search, shortcut: "⌘P", disabled: !sidebar?.enabled, run: () => sidebar?.openTab("files", true, tabId) },
    { label: "Side chat", Icon: Chat, shortcut: "⌥⌘S", disabled: archived, run: () => replaceNewTab(conversationId, tabId, "sideChat") },
    { label: "New file", Icon: Document, shortcut: "⌥⌘N", run: () => replaceNewTab(conversationId, tabId, "document") },
  ];
  return (
    <section aria-label="Tools" className="m-auto w-full max-w-new-tab-tools px-6 py-12">
      <h2 className="text-muted-foreground mb-3 text-sm font-medium">Tools</h2>
      <div className="grid grid-cols-1 gap-2 @sm/new-tab:grid-cols-2">
        {tools.map(({ label, Icon, shortcut, disabled, run }) => (
          <div key={label} className="bg-muted/60 flex min-w-0 items-center rounded-xl">
            <button type="button" disabled={disabled} onClick={run}
              className="hover:bg-toolbar-hover flex h-12 min-w-0 flex-1 items-center gap-3 rounded-xl px-3 text-sm disabled:opacity-40">
              <Icon aria-hidden className="size-icon-md shrink-0" />
              <span className="flex-1 text-start">{label}</span>
              <Kbd className="shrink-0 font-sans">{shortcutLabel(shortcut, mac)}</Kbd>
            </button>
            {label === "Terminal" && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <TooltipIconButton tooltip="Terminal options" disabled={disabled} className="me-2 shrink-0 rounded-full"><DotsHorizontal /></TooltipIconButton>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => setTerminalOpen(`conv:${conversationId}`, true)}>
                    Open bottom terminal
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
