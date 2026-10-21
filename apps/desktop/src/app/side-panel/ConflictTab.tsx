import { type FC, Fragment, type ReactNode, type RefObject, useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  acceptConflict,
  type Choice,
  type Conflict,
  conflictCount,
  conflictIndices,
  editorSegments,
  mergedResult,
  parseConflicts,
  type Pick,
  type Side,
  pickedLines,
  toggleSide,
  UNTOUCHED,
} from "@/app/side-panel/conflicts";
import { DiffView } from "@/app/side-panel/diffView";
import { RebaseStep } from "@/app/side-panel/RebaseStep";
import { StepCounter } from "@/app/conversation/StepCounter";
import { Button } from "@/components/controls";
import { ConfirmDialog } from "@/components/overlays";
import { request } from "@/ipc/client";
import type { ConflictFile, SourceState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { closeTab, type ConflictTabState, openConflictTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";
import { emitWorkspaceChanged, keepSource, onWorkspaceChanged, type WorkspaceTarget } from "@/state/workspaceCache";

/**
 * A conflicted file during a merge or rebase (a rebase names the commit it replays), in one of three views: its conflicts marked in place, each
 * with Accept Current Change, Accept Incoming Change, Accept Both Changes and Compare Changes;
 * both sides compared whole; or the merge editor, with the incoming and current versions above
 * and the result below, written and staged by Complete Merge. The current side is blue, the
 * incoming one green.
 */
export function ConflictTab({ conversationId, tab }: { conversationId: string; tab: ConflictTabState }) {
  const target = useMemo<WorkspaceTarget>(() => ({ type: "conversation", conversationId }), [conversationId]);
  const { file, error, load } = useConflictFile(target, tab.path);
  if (!file) {
    return (
      <div data-slot="conflict-tab" className="text-muted-foreground flex min-h-0 flex-1 items-center justify-center p-4 text-sm">
        {error ?? "Loading…"}
      </div>
    );
  }
  return tab.view === "compare" ? (
    <CompareView path={tab.path} file={file} />
  ) : tab.view === "merge" ? (
    editorSegments(file.text, file.merged) ? (
      <MergeEditor target={target} conversationId={conversationId} tab={tab} file={file} />
    ) : (
      <div data-slot="merge-editor" className="text-muted-foreground flex min-h-0 flex-1 items-center justify-center p-4 text-sm">
        This file&apos;s conflicts changed since git marked them. Resolve the rest in the file itself.
      </div>
    )
  ) : (
    <ConflictView target={target} conversationId={conversationId} path={tab.path} file={file} onWritten={load} />
  );
}

function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The file's versions, read again whenever the checkout changes. */
function useConflictFile(target: WorkspaceTarget, path: string) {
  const [file, setFile] = useState<ConflictFile | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    request({ method: "getConflictFile", target, path })
      .then(({ file: next }) => {
        setFile(next);
        setError(null);
      })
      .catch((cause: unknown) => setError(reason(cause)));
  }, [target, path]);
  useEffect(() => {
    load();
    return onWorkspaceChanged(target, load);
  }, [load, target]);
  return { file, error, load };
}

/** Writes the user's resolution of `path` (only while it is still `expected`), then shows the panel the checkout as it is. */
async function resolve(target: WorkspaceTarget, path: string, expected: string | null, text: string, stage: boolean): Promise<SourceState> {
  const { state } = await request({
    method: "sourceAction",
    target,
    action: { type: "resolve", path, expected, text, stage },
  });
  keepSource(target, state);
  emitWorkspaceChanged(target);
  return state;
}

/* ---------- the code ---------- */

/** One shown row: a numbered line (tinted by its side), or a row of actions over a block. */
type Row =
  | { kind: "line"; text: string; tint?: Tint; muted?: string; conflict?: number }
  | { kind: "lens"; content: ReactNode; conflict?: number };

type Tint = "currentHead" | "current" | "incoming" | "incomingHead" | "base";

const TINT: Record<Tint, string> = {
  currentHead: "bg-link/30",
  current: "bg-link/15",
  incoming: "bg-success/15",
  incomingHead: "bg-success/30",
  base: "bg-muted",
};

/** Each row's line number (0 for a row of actions). */
function lineNumbers(rows: readonly Row[]): number[] {
  const numbers: number[] = [];
  let number = 0;
  for (const row of rows) {
    if (row.kind === "line") number += 1;
    numbers.push(row.kind === "line" ? number : 0);
  }
  return numbers;
}

const Code: FC<{ rows: Row[] }> = ({ rows }) => {
  const numbers = lineNumbers(rows);
  return (
    <div className="min-h-0 flex-1 overflow-auto py-2 font-mono text-diff">
      {rows.map((row, index) => {
        const mark = row.conflict === undefined ? {} : { "data-conflict": row.conflict };
        if (row.kind === "lens") {
          return (
            <div key={index} {...mark} data-slot="conflict-lens" className="text-muted-foreground flex min-h-5 items-center ps-14 font-sans text-xs">
              {row.content}
            </div>
          );
        }
        return (
          <div key={index} {...mark} className={cn("flex min-h-5 leading-5 whitespace-pre", row.tint && TINT[row.tint])}>
            <span className="text-muted-foreground w-12 shrink-0 pe-4 text-end select-none">{numbers[index]}</span>
            <span className="pe-4">
              {row.text.replace(/\r?\n$/, "")}
              {row.muted && <span className="text-muted-foreground"> {row.muted}</span>}
            </span>
          </div>
        );
      })}
    </div>
  );
};

/** Actions in a row, split by bars, as the design reference shows them over a conflict. */
const Lens: FC<{ actions: { label: string; run: () => void; disabled?: boolean }[] }> = ({ actions }) => (
  <>
    {actions.map((action, index) => (
      <Fragment key={action.label}>
        {index > 0 && <span className="px-1">|</span>}
        <button
          type="button"
          disabled={action.disabled}
          onClick={action.run}
          className="hover:text-foreground hover:underline disabled:pointer-events-none"
        >
          {action.label}
        </button>
      </Fragment>
    ))}
  </>
);

/** ↑ and ↓ between the conflicts of `root`, as the editor's Previous and Next Conflict. */
function useConflictSteps(root: RefObject<HTMLDivElement | null>, count: number) {
  const at = useRef(-1);
  return (step: 1 | -1) => {
    if (count === 0) return;
    at.current = (at.current + step + count) % count;
    root.current?.querySelector(`[data-conflict="${at.current}"]`)?.scrollIntoView({ block: "center" });
  };
}

/* ---------- conflicts in place ---------- */

const ConflictView: FC<{
  target: WorkspaceTarget;
  conversationId: string;
  path: string;
  file: ConflictFile;
  onWritten: () => void;
}> = ({ target, conversationId, path, file, onWritten }) => {
  const root = useRef<HTMLDivElement>(null);
  const [busy, setBusy] = useState(false);
  const segments = useMemo(() => parseConflicts(file.text ?? ""), [file.text]);
  const count = conflictCount(segments);
  const step = useConflictSteps(root, count);
  const accept = (index: number, choice: Choice) => {
    if (file.text === null) return;
    setBusy(true);
    resolve(target, path, file.text, acceptConflict(file.text, index, choice), false)
      .catch((cause: unknown) => toast(`Failed to accept the change: ${reason(cause)}`, { tone: "error" }))
      .finally(() => {
        setBusy(false);
        onWritten();
      });
  };
  const rows: Row[] = [];
  let seen = -1;
  for (const segment of segments) {
    if (segment.type === "common") {
      for (const text of segment.lines) rows.push({ kind: "line", text });
      continue;
    }
    seen += 1;
    const index = seen;
    rows.push({
      kind: "lens",
      conflict: index,
      content: (
        <Lens
          actions={[
            { label: "Accept Current Change", run: () => accept(index, "current"), disabled: busy },
            { label: "Accept Incoming Change", run: () => accept(index, "incoming"), disabled: busy },
            { label: "Accept Both Changes", run: () => accept(index, "both"), disabled: busy },
            { label: "Compare Changes", run: () => openConflictTab(conversationId, path, "compare") },
          ]}
        />
      ),
    });
    rows.push(...markedRows(segment));
  }
  return (
    <div ref={root} data-slot="conflict-tab" className="relative flex min-h-0 flex-1 flex-col">
      <div className="border-border flex h-control-lg shrink-0 items-center gap-1 border-b px-2">
        <span className="text-muted-foreground flex-1 truncate px-1.5 text-sm">{path}</span>
        <StepCounter noun="conflict" vertical onStep={step} canBack={count > 0} canForward={count > 0} />
      </div>
      <RebaseStep file={file} />
      {file.text === null ? (
        <p className="text-muted-foreground p-4 text-sm">
          One side deleted this file. Stage it in the Source panel to keep it deleted, or discard it to keep your version.
        </p>
      ) : (
        <Code rows={rows} />
      )}
      {editorSegments(file.text, file.merged) && (
        <Button size="sm" className="absolute end-4 bottom-4" onClick={() => openConflictTab(conversationId, path, "merge")}>
          Resolve in Merge Editor
        </Button>
      )}
    </div>
  );
};

/** A conflict's lines as git marked them, each side tinted. */
function markedRows(conflict: Conflict): Row[] {
  return [
    { kind: "line", text: `${"<".repeat(conflict.size)} ${conflict.currentLabel}`, tint: "currentHead", muted: "(Current Change)" },
    ...conflict.current.map((text): Row => ({ kind: "line", text, tint: "current" })),
    ...(conflict.base
      ? [
          { kind: "line" as const, text: `${"|".repeat(conflict.size)} ${conflict.baseLabel}`.trimEnd(), tint: "base" as const },
          ...conflict.base.map((text): Row => ({ kind: "line", text, tint: "base" })),
        ]
      : []),
    { kind: "line", text: "=".repeat(conflict.size) },
    ...conflict.incoming.map((text): Row => ({ kind: "line", text, tint: "incoming" })),
    { kind: "line", text: `${">".repeat(conflict.size)} ${conflict.incomingLabel}`, tint: "incomingHead", muted: "(Incoming Change)" },
  ];
}

/* ---------- both sides compared ---------- */

const CompareView: FC<{ path: string; file: ConflictFile }> = ({ path, file }) => (
  <div data-slot="conflict-compare" className="min-h-0 flex-1 overflow-auto">
    <DiffView
      source={{ kind: "sides", path, before: file.current ?? "", after: file.incoming ?? "" }}
      look={{ numbered: true, split: true, wrap: false, words: true }}
    />
  </div>
);

/* ---------- the merge editor ---------- */

const MergeEditor: FC<{
  target: WorkspaceTarget;
  conversationId: string;
  tab: ConflictTabState;
  file: ConflictFile;
}> = ({ target, conversationId, tab, file }) => {
  const root = useRef<HTMLDivElement>(null);
  // The versions as the editor opened them: a change on disk meanwhile doesn't reset the picks.
  const [opened] = useState(file);
  // The working file, so edits and accepts made before opening the editor stay in the result.
  const segments = useMemo(() => editorSegments(opened.text, opened.merged) ?? [], [opened.text, opened.merged]);
  const count = conflictCount(segments);
  const [picks, setPicks] = useState<Pick[]>(() => Array.from({ length: count }, () => UNTOUCHED));
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const step = useConflictSteps(root, count);
  const remaining = picks.filter((pick) => !pick.handled).length;
  const change = (index: number, next: (pick: Pick) => Pick) =>
    setPicks((current) => current.map((pick, at) => (at === index ? next(pick) : pick)));

  const complete = () => {
    setAsking(false);
    setBusy(true);
    resolve(target, tab.path, opened.text, mergedResult(segments, picks), true)
      .then(() => closeTab(conversationId, tab.id))
      .catch((cause: unknown) => toast(`Failed to complete the merge: ${reason(cause)}`, { tone: "error" }))
      .finally(() => setBusy(false));
  };

  const indices = conflictIndices(segments);
  const side = (which: Side): Row[] => {
    const word = which === "incoming" ? "Incoming" : "Current";
    return segments.flatMap((segment, at): Row[] => {
      if (segment.type === "common") return segment.lines.map((text): Row => ({ kind: "line", text }));
      const index = indices[at]!;
      const pick = picks[index] ?? UNTOUCHED;
      const lines = which === "incoming" ? segment.incoming : segment.current;
      const actions = pick.order.includes(which)
        ? []
        : [
            { label: `Accept ${word}`, run: () => change(index, (p) => toggleSide(p, which)) },
            { label: `Accept Combination (${word} First)`, run: () => change(index, (p) => toggleSide(p, which, true)) },
            ...(pick.handled ? [] : [{ label: "Ignore", run: () => change(index, (p) => ({ ...p, handled: true })) }]),
          ];
      return [
        { kind: "lens", conflict: index, content: <Lens actions={actions} /> },
        ...lines.map((text): Row => ({ kind: "line", text, tint: which })),
      ];
    });
  };

  const result: Row[] = (() => {
    return segments.flatMap((segment, at): Row[] => {
      if (segment.type === "common") return segment.lines.map((text): Row => ({ kind: "line", text }));
      const index = indices[at]!;
      const pick = picks[index] ?? UNTOUCHED;
      const names = pick.order.map((entry) => (entry === "incoming" ? "Incoming" : "Current"));
      const tint: Tint | undefined = pick.order.length === 1 ? (pick.order[0] === "incoming" ? "incoming" : "current") : undefined;
      return [
        {
          kind: "lens",
          conflict: index,
          content: names.length === 0 ? (
            <span className={cn(!pick.handled && "text-warning")}>No Changes Accepted</span>
          ) : (
            <>
              <span>{names.join(", ")}</span>
              <span className="px-1">|</span>
              <Lens
                actions={pick.order.map((entry) => ({
                  label: `Remove ${entry === "incoming" ? "Incoming" : "Current"}`,
                  run: () => change(index, (p) => toggleSide(p, entry)),
                }))}
              />
            </>
          ),
        },
        ...pickedLines(segment, pick).map((text): Row => ({ kind: "line", text, ...(tint ? { tint } : {}) })),
      ];
    });
  })();

  return (
    <div ref={root} data-slot="merge-editor" className="relative flex min-h-0 flex-1 flex-col">
      <div className="border-border flex h-control-lg shrink-0 items-center gap-1 border-b px-2">
        <span className="text-muted-foreground flex-1 truncate px-1.5 text-sm">{tab.path}</span>
        <StepCounter noun="conflict" vertical onStep={step} canBack={count > 0} canForward={count > 0} />
      </div>
      <RebaseStep file={opened} />
      <div className="grid min-h-0 flex-1 grid-cols-2 grid-rows-2">
        <Pane title="Incoming" detail={opened.incomingLabel} className="border-e border-b">
          <Code rows={side("incoming")} />
        </Pane>
        <Pane title="Current" detail={opened.currentLabel} className="border-b">
          <Code rows={side("current")} />
        </Pane>
        <Pane
          title="Result"
          detail={tab.path}
          className="col-span-2"
          end={<span data-slot="conflicts-remaining">{remaining === 1 ? "1 Conflict Remaining" : `${remaining} Conflicts Remaining`}</span>}
        >
          <Code rows={result} />
        </Pane>
      </div>
      <Button
        size="sm"
        className="absolute end-4 bottom-4"
        disabled={busy}
        onClick={() => (remaining > 0 ? setAsking(true) : complete())}
      >
        Complete Merge
      </Button>
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={`Complete the merge of ${tab.path.slice(tab.path.lastIndexOf("/") + 1)}?`}
        description={`${remaining === 1 ? "1 conflict isn't" : `${remaining} conflicts aren't`} handled yet. The result keeps the common version there.`}
        confirm="Complete with Conflicts"
        onConfirm={complete}
      />
    </div>
  );
};

const Pane: FC<{ title: string; detail: string; className?: string; end?: ReactNode; children: ReactNode }> = ({
  title,
  detail,
  className,
  end,
  children,
}) => (
  <section aria-label={title} className={cn("border-border flex min-h-0 min-w-0 flex-col", className)}>
    <div className="text-muted-foreground flex h-control-sm shrink-0 items-center gap-2 px-3 text-xs">
      <span className="text-foreground font-semibold">{title}</span>
      <span className="min-w-0 flex-1 truncate">{detail}</span>
      {end}
    </div>
    {children}
  </section>
);
