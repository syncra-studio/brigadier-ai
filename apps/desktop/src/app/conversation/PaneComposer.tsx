import {
  type ReactNode,
  useCallback,
  useContext,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import { ConversationComposer } from "@/app/conversation/Composer";
import { ComposerCapsule } from "@/app/conversation/ComposerCapsule";
import { SidePanelContext } from "@/app/conversation/SidePanel";
import type { ComposerProps } from "@/components/assistant-ui/thread";

/** One editor, moved between split and full view without losing its draft or attachments. */
export function ComposerPlacement({ children }: { children: ReactNode }) {
  const panel = useContext(SidePanelContext);
  const floating =
    panel.visible && panel.state.fullscreen && panel.state.active === "browser";
  const anchor = useRef<HTMLDivElement>(null);
  const [host] = useState(() => document.createElement("div"));
  const restore = useRef<{
    focused: HTMLElement;
    selected: {
      anchor: Node;
      anchorOffset: number;
      focus: Node;
      focusOffset: number;
    } | null;
  } | null>(null);
  const rememberFocus = useCallback(() => {
    if (!host.contains(document.activeElement)) {
      restore.current = null;
      return;
    }
    const selection = window.getSelection();
    restore.current = {
      focused: document.activeElement as HTMLElement,
      selected:
        selection?.anchorNode && selection.focusNode
          ? {
              anchor: selection.anchorNode,
              anchorOffset: selection.anchorOffset,
              focus: selection.focusNode,
              focusOffset: selection.focusOffset,
            }
          : null,
    };
  }, [host]);
  useLayoutEffect(() => {
    const inline = anchor.current;
    const slot = inline
      ?.closest('[data-slot="pane-workspace"]')
      ?.querySelector('[data-slot="floating-composer"]');
    if (host.contains(document.activeElement)) rememberFocus();
    const focus = restore.current;
    (floating && slot ? slot : inline)?.append(host);
    focus?.focused.focus({ preventScroll: true });
    const selected = focus?.selected;
    if (
      selected &&
      host.contains(selected.anchor) &&
      host.contains(selected.focus)
    )
      window
        .getSelection()
        ?.setBaseAndExtent(
          selected.anchor,
          selected.anchorOffset,
          selected.focus,
          selected.focusOffset,
        );
    return rememberFocus;
  }, [floating, host, rememberFocus]);
  useLayoutEffect(() => () => host.remove(), [host]);
  return (
    <>
      <div ref={anchor} />
      {createPortal(children, host)}
    </>
  );
}

export function PaneComposer(props: ComposerProps) {
  return (
    <ComposerPlacement>
      <ConversationComposer {...props} />
    </ComposerPlacement>
  );
}

export function FloatingComposerSlot({
  capsule = true,
}: {
  capsule?: boolean;
}) {
  const panel = useContext(SidePanelContext);
  const drag = useRef<{ x: number; width: number } | null>(null);
  const clamp = (width: number) =>
    Math.max(
      panel.composerLimits.min,
      Math.min(panel.composerLimits.max, width),
    );
  if (
    !panel.visible ||
    !panel.state.fullscreen ||
    panel.state.active !== "browser"
  )
    return null;
  return (
    <div className="pointer-events-none absolute inset-x-2.5 bottom-4 z-30 flex justify-end">
      <div
        className="pointer-events-auto relative max-w-full"
        style={{ width: panel.composerWidth }}
      >
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize floating composer"
          aria-valuenow={panel.composerWidth}
          aria-valuemin={panel.composerLimits.min}
          aria-valuemax={panel.composerLimits.max}
          tabIndex={0}
          className="w-resize-handle absolute inset-y-0 start-0 z-10 -translate-x-1/2 cursor-col-resize rounded-control focus-visible:ring-1 focus-visible:ring-ring"
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            event.currentTarget.setPointerCapture(event.pointerId);
            drag.current = { x: event.clientX, width: panel.composerWidth };
          }}
          onPointerMove={(event) => {
            if (drag.current)
              panel.resizeComposer(
                clamp(drag.current.width + drag.current.x - event.clientX),
              );
          }}
          onPointerUp={(event) => {
            drag.current = null;
            if (event.currentTarget.hasPointerCapture(event.pointerId))
              event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          onPointerCancel={() => {
            drag.current = null;
          }}
          onDoubleClick={() => panel.resizeComposer(null)}
          onKeyDown={(event) => {
            const next =
              event.key === "ArrowLeft"
                ? panel.composerWidth + 10
                : event.key === "ArrowRight"
                  ? panel.composerWidth - 10
                  : event.key === "Home"
                    ? panel.composerLimits.min
                    : event.key === "End"
                      ? panel.composerLimits.max
                      : null;
            if (next !== null) {
              event.preventDefault();
              panel.resizeComposer(clamp(next));
            }
          }}
        />
        {capsule && <ComposerCapsule />}
        <div data-slot="floating-composer" />
      </div>
    </div>
  );
}
