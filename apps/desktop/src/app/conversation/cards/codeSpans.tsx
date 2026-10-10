import { Fragment } from "react";

import { codeParts } from "@/app/conversation/cards/questionRound";

/** A card's question or answer line with its backticked names shown as code. */
export function CodeSpans({ text }: { text: string }) {
  return codeParts(text).map((part, index) =>
    part.code ? (
      <code key={index} className="bg-muted text-code-inline rounded-md px-1 py-px font-mono">
        {part.text}
      </code>
    ) : (
      <Fragment key={index}>{part.text}</Fragment>
    ),
  );
}
