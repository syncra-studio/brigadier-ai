import { useCallback, useLayoutEffect, useRef, type MouseEvent as ReactMouseEvent } from "react";

import { MockupTabIcon, MockupTabStatus } from "@/app/conversation/MockupTabIcon";
import { BrowserTabIcon } from "@/app/conversation/BrowserTabIcon";
import { TabMenu } from "@/app/shell/TabMenu";
import { FileTypeIcon } from "@/components/conversation";
import { ChatCompose, DiffGlyph, Dock, Document, FileDocument, Globe, Lightbulb, ShieldCheck, Terminal, X } from "@/components/icons";
import type { useDragReorder } from "@/hooks/use-drag-reorder";
import { cn } from "@/lib/utils";
import { keepTabOpen, type SessionTab as Tab, selectTab } from "@/state/sessionTabs";

/** Tabs share the available row, then scroll once they reach their minimum width. */
export const TAB =
  "session-tab group/tab relative flex h-control-md min-w-0 items-center gap-1 rounded-session-tab ps-2.5 pe-1.75 select-none";

/** Mouse down with the middle button would start autoscroll. */
export function noAutoscroll(event: ReactMouseEvent): void {
  if (event.button === 1) event.preventDefault();
}

/**
 * A tab the session opened: its kind's icon and its name (italic while a preview), a dot while
 * it has something new, and its ×. The tab itself is the drag handle; its menu is TabMenu's.
 */
export function SessionTab({
  conversationId,
  tab,
  name,
  selected,
  separator,
  closing,
  dragging,
  handlers,
  onMouseUp,
  onClose,
}: {
  conversationId: string;
  tab: Tab;
  name: string;
  selected: boolean;
  separator: true | undefined;
  closing: boolean;
  dragging: boolean;
  handlers: ReturnType<ReturnType<typeof useDragReorder>["grip"]>;
  onMouseUp: (event: ReactMouseEvent) => void;
  onClose: () => void;
}) {
  return (
    <TabMenu conversationId={conversationId} tab={tab}>
      <AnimatedTab
        data-session-tab
        closing={closing}
        inert={closing}
        aria-hidden={closing || undefined}
        data-separator={separator}
        data-tab-id={tab.id}
        data-active={selected || undefined}
        data-dragging={dragging || undefined}
        className={cn(
          TAB,
          selected
            ? "bg-panel-tab shadow-session-tab text-foreground"
            : "text-toolbar-foreground hover:bg-toolbar-hover",
          "data-dragging:z-10 data-dragging:opacity-80",
        )}
      >
        {/* The tab itself is the drag handle; Alt+←/→ moves it from the keyboard. */}
        <button
          type="button"
          role="tab"
          aria-selected={selected}
          tabIndex={selected ? 0 : -1}
          title={tab.kind === "file" ? tab.path : tab.kind === "sideChat" && tab.title ? `Side chat · ${tab.title}` : name}
          onPointerDown={handlers.onPointerDown}
          onPointerMove={handlers.onPointerMove}
          onPointerUp={handlers.onPointerUp}
          onPointerCancel={handlers.onPointerCancel}
          onMouseDown={noAutoscroll}
          onMouseUp={onMouseUp}
          onClick={() => selectTab(conversationId, tab.id)}
          onDoubleClick={() => keepTabOpen(conversationId, tab.id)}
          onKeyDown={(event) => {
            if (event.altKey) handlers.onKeyDown(event);
          }}
          className="flex h-full min-w-0 flex-1 items-center gap-1 rounded-sm"
        >
          {tab.kind === "file" || tab.kind === "conflict" ? (
            <FileTypeIcon name={tab.path} className="size-icon-sm shrink-0" />
          ) : tab.kind === "newTab" ? <Globe className="size-icon-sm shrink-0" />
            : tab.kind === "browser" ? <BrowserTabIcon tabId={tab.id} />
            : tab.kind === "terminal" ? <Dock className="size-icon-sm shrink-0" />
            : tab.kind === "sideChat" ? <ChatCompose className="size-icon-sm shrink-0" />
            : tab.kind === "page" ? <Document className="size-icon-sm shrink-0" />
            : tab.kind === "mockup" ? <MockupTabIcon conversationId={conversationId} />
            : tab.kind === "plan" ? <Lightbulb className="size-icon-sm shrink-0" />
            : tab.kind === "pasted" ? <FileDocument className="size-icon-sm shrink-0" />
            : tab.kind === "panel" ? tab.view.type === "output" ? <Terminal className="size-icon-sm shrink-0" /> : tab.view.type === "sources" ? <Globe className="size-icon-sm shrink-0" /> : <ShieldCheck className="size-icon-sm shrink-0" /> : (
            <DiffGlyph className="size-icon-sm shrink-0" />
          )}
          <span className={cn("session-tab-title min-w-0 flex-1 overflow-hidden text-start whitespace-nowrap", tab.kind === "file" && tab.preview && "italic", tab.kind === "conflict" && tab.view !== "compare" && "text-destructive")}>
            {name}
            {tab.kind === "mockup" && <MockupTabStatus conversationId={conversationId} />}
          </span>
          {tab.unseen && <span data-slot="tab-unseen" role="img" aria-label="New" className="bg-link size-2 shrink-0 rounded-full" />}
        </button>
        <button
          type="button"
          aria-label={`Close ${name} tab`}
          onClick={(event) => {
            event.stopPropagation();
            onClose();
          }}
          className={cn(
            "hover:bg-toolbar-hover flex size-5 shrink-0 items-center justify-center rounded-full opacity-60 hover:opacity-100",
            !selected && "invisible group-focus-within/tab:visible group-hover/tab:visible",
          )}
        >
          <X aria-hidden className="size-icon-xs" />
        </button>
      </AnimatedTab>
    </TabMenu>
  );
}

/** A new tab grows into its slot; the final width still comes from the responsive CSS rule. */
function AnimatedTab({ closing, ref: triggerRef, ...props }: React.ComponentProps<"div"> & { closing: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const attach = useCallback((element: HTMLDivElement | null) => {
    ref.current = element;
    if (typeof triggerRef === "function") return triggerRef(element);
    if (triggerRef) triggerRef.current = element;
  }, [triggerRef]);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element || closing || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const style = getComputedStyle(element);
    const animation = element.animate([
      { flexBasis: "0", paddingInlineStart: "0", paddingInlineEnd: "0", opacity: 0, overflow: "hidden" },
      { flexBasis: `${element.getBoundingClientRect().width}px`, paddingInlineStart: style.paddingInlineStart, paddingInlineEnd: style.paddingInlineEnd, opacity: 1, overflow: "hidden" },
    ], { duration: 200, easing: style.getPropertyValue("--ease-session-tab").trim() });
    return () => animation.cancel();
  }, [closing]);
  return <div ref={attach} data-closing={closing || undefined} {...props} />;
}
