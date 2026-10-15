import { Check, Copy } from "@openai/apps-sdk-ui/components/Icon";
import { useMemo } from "react";

import { LineText, pairChanges, TINT, TokensContext, useSideTokens } from "@/app/conversation/diffLines";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { diffCounts, type DiffRow, parseDiff } from "@/lib/patch";
import { cn } from "@/lib/utils";

/** A diff longer than this many lines isn't drawn inline. */
const TOO_LARGE_LINES = 5000;

const CARD = "border-foreground/10 rounded-shell mt-1 flex min-w-0 flex-col overflow-hidden border";

/**
 * One file's change, as a thread row opens to it: a card with the file's name, "+7 −2" and Copy,
 * then its lines in the file's colours, each changed line tinted with a bar and its changed words
 * marked, numbered when the change came as a real patch. Up to 240px, then it scrolls, its bottom
 * fading while there is more. `tall`: no height limit (a dialog of its own).
 */
export function InlineDiff({ path, diff, tall = false }: { path: string; diff: string | null | undefined; tall?: boolean }) {
  const parsed = useMemo(() => (diff === null || diff === undefined ? null : parseDiff(diff)), [diff]);
  const { added, removed } = parsed ? diffCounts(parsed.rows) : { added: 0, removed: 0 };
  const name = path.split("/").pop() || path;
  const tooLarge = !parsed || parsed.rows.length > TOO_LARGE_LINES;
  return (
    <div data-slot="diff-card" className={CARD}>
      <div className="bg-foreground/5 text-foreground/80 flex min-h-8 items-center gap-2 ps-3 pe-1 text-sm">
        <span data-slot="diff-name" className="min-w-0 truncate" title={path}>
          {name}
        </span>
        {parsed && (
          <span data-slot="diff-counts" className="text-muted-foreground shrink-0 text-xs tabular-nums">
            <span className="text-success">+{added}</span> <span className="text-destructive">−{removed}</span>
          </span>
        )}
        {diff && <CopyDiff text={diff} />}
      </div>
      {tooLarge ? (
        <p data-slot="diff-empty" className="text-muted-foreground px-3 py-2 text-sm">
          Diff too large to preview
        </p>
      ) : parsed.rows.length === 0 ? (
        <p data-slot="diff-empty" className="text-muted-foreground px-3 py-2 text-sm">
          No changes
        </p>
      ) : (
        <DiffBody path={path} rows={parsed.rows} numbered={parsed.numbered} tall={tall} />
      )}
    </div>
  );
}

function DiffBody({ path, rows, numbered, tall }: { path: string; rows: DiffRow[]; numbered: boolean; tall: boolean }) {
  const tokens = useSideTokens(path, rows);
  const pairs = useMemo(() => pairChanges(rows), [rows]);
  return (
    <TokensContext.Provider value={tokens}>
      <div
        data-slot="diff-lines"
        className={cn(
          "min-w-0 overflow-auto py-1 font-mono text-xs leading-5 whitespace-pre",
          !tall && "max-h-60 scroll-edge-fade [--spacing-scroll-fade-top:0] [--spacing-scroll-fade-bottom:var(--spacing-shell-fade)]",
        )}
      >
        <div className="w-max min-w-full">
          {rows.map((row, index) =>
            row.kind === "gap" ? (
              <div key={`gap:${row.id}`} data-slot="diff-gap" className="text-muted-foreground px-3 font-sans">
                {row.count > 0 ? `⋯ ${row.count} unchanged ${row.count === 1 ? "line" : "lines"}` : "⋯"}
              </div>
            ) : (
              <div key={index} data-kind={row.kind} className={cn("flex border-s-2 border-transparent", TINT[row.kind])}>
                {numbered && (
                  <span
                    data-slot="diff-number"
                    className={cn(
                      "w-10 shrink-0 pe-2 text-end tabular-nums select-none",
                      row.kind === "add" ? "text-success" : row.kind === "del" ? "text-destructive" : "text-muted-foreground",
                    )}
                  >
                    {row.kind === "del" ? row.old : row.new}
                  </span>
                )}
                <span className={cn("min-w-0 flex-1 pe-3", !numbered && "ps-3")}>
                  <LineText line={row} partner={pairs.get(row) ?? null} />
                </span>
              </div>
            ),
          )}
        </div>
      </div>
    </TokensContext.Provider>
  );
}

function CopyDiff({ text }: { text: string }) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy diff"} size="xs" className="text-muted-foreground ms-auto" onClick={() => copyToClipboard(text)}>
      {isCopied ? <Check /> : <Copy />}
    </TooltipIconButton>
  );
}
