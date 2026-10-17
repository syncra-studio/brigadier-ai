import { Check, Copy } from "@openai/apps-sdk-ui/components/Icon";
import { useMemo } from "react";

import { DiffView } from "@/app/conversation/diffView";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { deletesFile, diffCounts, parseDiff, singleFilePatch } from "@/lib/patch";
import { cn } from "@/lib/utils";

/** A diff longer than this many lines isn't drawn inline. */
const TOO_LARGE_LINES = 5000;

const CARD = "border-foreground/10 rounded-shell mt-1 flex min-h-0 min-w-0 flex-col overflow-clip border";

/**
 * One file's change, as a thread row opens to it: a card with the file's name, "+7 −2" and Copy,
 * then its lines in the file's colours, each changed line tinted with a bar and its changed words
 * marked, numbered when the change came as a real patch. Up to 240px, then it scrolls, its bottom
 * fading while there is more. `tall`: no height limit (a dialog of its own). In a group's list its
 * header sticks below the list's faded top edge (`--diff-header-top`) while the card scrolls by.
 */
export function InlineDiff({ path, diff, tall = false }: { path: string; diff: string | null | undefined; tall?: boolean }) {
  const parsed = useMemo(() => (diff === null || diff === undefined ? null : parseDiff(diff)), [diff]);
  const { added, removed } = parsed ? diffCounts(parsed.rows) : { added: 0, removed: 0 };
  const name = path.split("/").pop() || path;
  const tooLarge = !parsed || parsed.rows.length > TOO_LARGE_LINES;
  return (
    <div data-slot="diff-card" className={CARD}>
      <div className="bg-background sticky top-(--diff-header-top) z-10">
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
      </div>
      {tooLarge ? (
        <p data-slot="diff-empty" className="text-muted-foreground px-3 py-2 text-sm">
          Diff too large to preview
        </p>
      ) : parsed.rows.length === 0 ? (
        <p data-slot="diff-empty" className="text-muted-foreground px-3 py-2 text-sm">
          No changes
        </p>
      ) : added === 0 && deletesFile(diff ?? "") ? (
        <p data-slot="diff-empty" className="text-muted-foreground px-3 py-2 text-sm">
          Contents deleted
        </p>
      ) : (
        <DiffBody path={path} diff={diff ?? ""} numbered={parsed.numbered} tall={tall} />
      )}
    </div>
  );
}

function DiffBody({ path, diff, numbered, tall }: { path: string; diff: string; numbered: boolean; tall: boolean }) {
  const source = useMemo(() => ({ kind: "patch" as const, patch: singleFilePatch(path, diff) }), [path, diff]);
  return (
    <div
      data-slot="diff-lines"
      className={cn(
        "min-w-0 overflow-y-auto",
        !tall && "max-h-60 scroll-edge-fade [--spacing-scroll-fade-top:0] [--spacing-scroll-fade-bottom:var(--spacing-shell-fade)]",
      )}
    >
      <DiffView source={source} look={{ numbered, split: false, wrap: false, words: true }} />
    </div>
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
