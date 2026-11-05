import { Fragment, type ReactNode, useRef } from "react";

import { codeParts, selectedWithin } from "@/app/conversation/cards/questionRound";
import { openLink } from "@/state/links";
import { toast } from "@/state/toasts";

/**
 * A web link on a card, opening as the thread's do; hovering shows its whole address. It keeps a
 * click to itself, so the answer row around it isn't picked. A double-click selects its words and
 * opens nothing more, and a drag opens nothing (a browser starts no selection from a press on a
 * link: a drag selects it from outside it).
 */
function CardLink({ href, children }: { href: string; children: ReactNode }) {
  const pressed = useRef({ x: 0, y: 0 });
  return (
    <a
      data-slot="card-link"
      href={href}
      title={href}
      draggable={false}
      onMouseDown={(event) => {
        pressed.current = { x: event.clientX, y: event.clientY };
      }}
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        const dragged = event.detail > 0 && Math.hypot(event.clientX - pressed.current.x, event.clientY - pressed.current.y) > 4;
        if (dragged || event.detail > 1 || selectedWithin(event.currentTarget)) return;
        openLink(href, { agent: true }).catch((error: unknown) => {
          toast(error instanceof Error ? error.message : String(error), { tone: "error" });
        });
      }}
      className="text-link hover:text-link/80 underline-offset-2 hover:underline hover:decoration-dashed hover:decoration-0.5"
    >
      {children}
    </a>
  );
}

/**
 * A card's words with their backticked names shown as code and their web addresses as links.
 * `exact`: text that must read as written (a command, a tool's input); see `codeParts`.
 */
export function CodeSpans({ text, exact = false }: { text: string; exact?: boolean }) {
  return codeParts(text, exact).map((part, index) => {
    if (part.href) return <CardLink key={index} href={part.href}>{part.text}</CardLink>;
    if (part.code && !exact) {
      return (
        <code key={index} className="bg-muted text-code-inline rounded-md px-1 py-px font-mono">
          {part.text}
        </code>
      );
    }
    return <Fragment key={index}>{part.text}</Fragment>;
  });
}
