import { type FC, type ReactNode, useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { FoldBody, FoldHeader } from "@/app/conversation/cardFold";
import { OpenPill } from "@/app/conversation/activity/OpenPill";
import { failed, kindOf, LAST_LINES, printed, type Proof } from "@/app/conversation/activity/previewProofs";
import { Proofs } from "@/app/conversation/activity/Proofs";
import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { useViewConversation } from "@/app/conversation/viewContext";
import { Button, RoundBadge } from "@/components/controls";
import { ArrowRotateCw, ArrowUpRight, ChevronDown, ChevronUp, Document, Globe, ImageSquare, Mobile, Play, Terminal } from "@/components/icons";
import { RowText, SettingsCard } from "@/components/shared";
import type { Preview, PreviewKind } from "@/ipc/generated";
import { openPdf, openSimulator } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { previewLogTail, restartPreview, resumePreview } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The Preview card, under a finished turn's answer and above its Changes card (the user's design,
 * `K53xQ`): a round badge, what the turn left to look at ("Web preview"), its state ("Running",
 * "Stopped") and address, and the proofs the thread took of it (`attach_proof`) inside the card,
 * captioned thumbnails cropped to the top of the page. Running: Open, a split pill whose ▾ picks
 * where. Paused: Start again. The chevron folds the proofs away; folded, the header counts them.
 * Proofs with no page left running are the card's alone ("Screenshots").
 */

/** Each kind of proof, by how it is counted: test output counts as a log. */
const NOUNS = [
  { kinds: ["screenshot"], one: "screenshot", many: "screenshots" },
  { kinds: ["recording"], one: "recording", many: "recordings" },
  { kinds: ["log", "test"], one: "log", many: "logs" },
];

/** "2 screenshots", "1 recording", "2 screenshots, 1 recording", "1 log". */
function proofCount(proofs: readonly Proof[]): string {
  return NOUNS.flatMap(({ kinds, one, many }) => {
    const count = proofs.filter((proof) => kinds.includes(kindOf(proof))).length;
    return count > 0 ? [`${count} ${count === 1 ? one : many}`] : [];
  }).join(", ");
}

/** A card of proofs alone, by what they are: "Screenshots", "Screenshots and recordings", "Test output". */
function proofsTitle(proofs: readonly Proof[]): string {
  if (proofs.every((proof) => kindOf(proof) === "test")) return "Test output";
  const nouns = NOUNS.filter(({ kinds }) => proofs.some((proof) => kinds.includes(kindOf(proof)))).map(({ many }) => many);
  const words = nouns.length > 1 ? `${nouns.slice(0, -1).join(", ")} and ${nouns.at(-1)}` : (nouns[0] ?? "");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Where the proofs were taken, when they all say and it is one place: "Checked in the browser". */
function takenWhere(proofs: readonly Proof[]): string | null {
  if (proofs.length === 0 || proofs.some(printed)) return null;
  return proofs.every((proof) => proof.takenOn !== undefined && /^https?:\/\//.test(proof.takenOn)) ? "Checked in the browser" : null;
}

/** A preview record for the page: a local server on the same port, the latest one started. */
function samePage(record: Preview, url: string): boolean {
  if (!record.url) return false;
  try {
    const page = new URL(url);
    const served = new URL(record.url);
    return page.port === served.port && page.protocol === served.protocol;
  } catch {
    return false;
  }
}

/** How a preview stands, in a word: Running (green), Starting, Paused or Stopped (grey). */
function standing(record: Preview): { words: string; live: boolean } {
  switch (record.state.type) {
    case "running":
      return { words: "Running", live: true };
    case "starting":
      return { words: record.building ? "Building" : "Starting", live: false };
    case "paused":
      return { words: "Paused", live: false };
    default:
      return { words: "Stopped", live: false };
  }
}

/** A preview that isn't a web page, by its kind: its title, its badge and the word for it being up. */
const KINDS: Record<Exclude<PreviewKind, "web" | "app">, { title: string; Icon: FC<{ className?: string }>; up: string }> = {
  simulator: { title: "iOS Simulator", Icon: Mobile, up: "Running" },
  pdf: { title: "Rendered PDF", Icon: Document, up: "Ready" },
  test: { title: "Test run", Icon: Terminal, up: "Running" },
};

/** Whether a preview is one of those: a simulator run, a rendered PDF or a test run. */
export function isOtherPreview(preview: Preview): boolean {
  return preview.kind in KINDS;
}

/** An ended test run's counts as its runner gave them: "15 passed", "1 failed, 14 passed, 2 skipped". */
function testCounts(record: Preview): string | null {
  const { passed, failed: failing, skipped } = record.tests ?? { passed: 0, failed: 0, skipped: 0 };
  const parts = [failing > 0 && `${failing} failed`, (passed > 0 || failing === 0) && `${passed} passed`, skipped > 0 && `${skipped} skipped`].filter(Boolean);
  return passed + failing + skipped > 0 ? parts.join(", ") : null;
}

/** How a simulator, PDF or test run stands, in a word: a test run that ended well Passed (green), with its counts when its runner gave them. */
function kindStanding(record: Preview): { words: string; live: boolean } {
  const kind = KINDS[record.kind as keyof typeof KINDS];
  if (record.state.type === "running" || rendered(record)) return { words: kind.up, live: true };
  if (record.kind === "test" && record.state.type === "exited") {
    const passed = record.state.code === 0;
    return { words: testCounts(record) ?? (passed ? "Passed" : "Failed"), live: passed };
  }
  return standing(record);
}

/** A test run's own output, its last lines, read from its log while the card shows it. */
function useRunOutput(conversationId: string | null, record: Preview | null): string | null {
  const [found, setFound] = useState<{ key: string; tail: string } | null>(null);
  const key = record && conversationId ? `${conversationId} ${record.id} ${record.state.type} ${record.endedAtMs ?? ""}` : null;
  useEffect(() => {
    if (!key || !record || !conversationId) return;
    let current = true;
    previewLogTail(conversationId, record.id)
      .then(({ tail }) => current && setFound({ key, tail }))
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [key, conversationId, record]);
  return found && found.key === key ? found.tail : null;
}

/** A PDF whose render is going or ended well: its file is there to open. */
const rendered = (record: Preview) =>
  record.kind === "pdf" && (record.state.type === "running" || (record.state.type === "exited" && record.state.code === 0));

/** A rendered PDF's file: its target, from the preview's folder when not absolute. */
const pdfPath = (record: Preview) =>
  !record.target ? null : record.target.startsWith("/") ? record.target : `${record.workdir.replace(/\/$/, "")}/${record.target}`;

/**
 * What a finished turn left to look at: its page (`url`, served by a preview record; an address
 * with no preview behind it is no card) or one of its other previews (`preview`: a simulator run, a rendered PDF, a test run), and the proofs it took
 * of it (`steps`). Neither page nor preview, and the card is the proofs' alone; no proofs either,
 * and there is no card. `session`: the turn is a session's, whose Browser tabs can show the page.
 */
export function PreviewCard({ url: named, preview = null, steps, session }: { url: string | null; preview?: Preview | null; steps: readonly BlockOrchestratorStep[]; session: boolean }) {
  const viewed = useViewConversation()?.id ?? null;
  const conversationId = session ? viewed : null;
  const proofs = steps.flatMap((step) => (step.kind.type === "proof" ? [{ proof: step.kind as Proof, atMs: (step.kind as Proof).takenAtMs ?? step.atMs }] : []));
  const page = useBoard((s) =>
    named && !preview ? Object.values(s.board?.previews ?? {}).filter((each) => samePage(each, named)).toSorted((a, b) => a.startedAtMs - b.startedAtMs).at(-1) ?? null : null,
  );
  // An address nothing was started on is no page (#438).
  const url = page ? named : null;
  const record = preview ?? page;
  const kind = preview ? KINDS[preview.kind as keyof typeof KINDS] : null;
  const ownLog = preview?.kind === "test" && !proofs.some(({ proof }) => printed(proof));
  const output = useRunOutput(viewed, ownLog ? preview : null);
  const [open, setOpen] = useState(true);
  if (!url && !preview && proofs.length === 0) return null;

  const counted = proofCount(proofs.map(({ proof }) => proof));
  const state = record ? (preview ? kindStanding(preview) : standing(record)) : null;
  // A PDF's render that ended well has done what it was for: it opens, not starts again.
  const ended = record !== null && ["stopped", "exited"].includes(record.state.type) && !(preview && rendered(preview));
  const stopped = state !== null && !state.live && record?.state.type !== "starting";
  const address = preview ? (preview.target ?? null) : (url?.replace(/^https?:\/\//, "").replace(/\/$/, "") ?? null);
  const title = kind ? kind.title : url ? "Web preview" : proofsTitle(proofs.map(({ proof }) => proof));
  const where = takenWhere(proofs.map(({ proof }) => proof));
  const logsOnly = proofs.length > 0 && proofs.every(({ proof }) => printed(proof));
  const lastLines = output?.trimEnd().split("\n").slice(-LAST_LINES).join("\n") ?? "";
  const body = proofs.length > 0 || (ownLog && lastLines !== "");

  let action: ReactNode = null;
  if (record && record.state.type === "paused" && viewed) {
    action = (
      <Button variant="soft" size="sm" data-slot="preview-start" onClick={() => void resumePreview(viewed, record.id).catch(failed)}>
        <Play aria-hidden className="size-icon-sm" />
        Start again
      </Button>
    );
  } else if (record && ended && viewed && (url || preview)) {
    // Its own command again, from its record.
    const again = record.kind === "test";
    action = (
      <Button variant="soft" size="sm" data-slot="preview-start" onClick={() => void restartPreview(viewed, record.id).catch(failed)}>
        {again ? <ArrowRotateCw aria-hidden className="size-icon-sm" /> : <Play aria-hidden className="size-icon-sm" />}
        {again ? "Run again" : "Start again"}
      </Button>
    );
  } else if (preview && rendered(preview)) {
    const file = pdfPath(preview);
    if (file) {
      action = (
        <Button variant="soft" size="sm" data-slot="preview-open" title={file} onClick={() => void openPdf(file).catch(failed)}>
          <ArrowUpRight aria-hidden className="size-icon-sm" />
          Open
        </Button>
      );
    }
  } else if (preview?.kind === "simulator" && preview.state.type === "running") {
    // Simulator to the front, on the preview's device (none named: any booted one).
    const device = preview.target && preview.target !== KINDS.simulator.title ? preview.target : null;
    action = (
      <Button variant="soft" size="sm" data-slot="preview-open" title={device ?? KINDS.simulator.title} onClick={() => void openSimulator(device).catch(failed)}>
        <ArrowUpRight aria-hidden className="size-icon-sm" />
        Open
      </Button>
    );
  } else if (url && !preview && !stopped) {
    action = <OpenPill url={url} conversationId={conversationId} />;
  }

  const line: ReactNode[] = [];
  if (state) {
    line.push(
      <span key="state" data-slot="preview-state" className="flex shrink-0 items-center gap-2">
        <span aria-hidden className={cn("size-1.5 rounded-full", state.live ? "bg-success" : "bg-muted-foreground")} />
        {state.words}
      </span>,
    );
  }
  if (address) {
    line.push(
      <span key="address" data-slot="preview-target" title={preview ? address : (url ?? undefined)} className="text-muted-foreground min-w-0 truncate font-mono">
        {address}
      </span>,
    );
  }
  // A rendered PDF's page count, when its file could be read.
  if (preview?.kind === "pdf" && preview.pages) {
    line.push(
      <span key="pages" data-slot="preview-pages" className="text-muted-foreground shrink-0">
        {preview.pages === 1 ? "1 page" : `${preview.pages} pages`}
      </span>,
    );
  }
  // Folded, a card of proofs alone or another kind of preview, the header counts them.
  if (counted && (!open || !url || preview)) {
    line.push(
      <span key="count" data-slot="preview-count" className="text-muted-foreground shrink-0">
        {counted}
      </span>,
    );
  }
  const Badge = kind?.Icon ?? (url ? Globe : logsOnly ? Terminal : ImageSquare);

  return (
    <SettingsCard inset split={false}
      data-slot="preview-card"
      data-kind={preview?.kind ?? (url ? "web" : undefined)}
      data-state={state ? (stopped ? "stopped" : "running") : undefined}
      data-open={(open && body) || undefined}
    >
      <FoldHeader foldable={body} onToggle={() => setOpen(!open)} className="flex items-center gap-4 py-4">
        <RoundBadge>
          <Badge className={cn(stopped && "text-foreground/65")} />
        </RoundBadge>
        <RowText
          title={<span data-slot="preview-title">{title}</span>}
          line={
            <span className="flex min-w-0 items-center gap-2">
              {line.flatMap((part, index) => (index === 0 ? [part] : [<span key={`dot-${index}`} aria-hidden className="text-muted-foreground">·</span>, part]))}
            </span>
          }
        />
        <div className="flex shrink-0 items-center gap-1">
          {action}
          {body && (
            <button
              type="button"
              data-slot="preview-fold"
              aria-expanded={open}
              aria-label={open ? "Fold the proofs" : "Show the proofs"}
              onClick={() => setOpen(!open)}
              className="text-muted-foreground hover:text-foreground flex size-7.5 items-center justify-center"
            >
              {open ? <ChevronUp aria-hidden className="size-3.75" /> : <ChevronDown aria-hidden className="size-3.75" />}
            </button>
          )}
        </div>
      </FoldHeader>
      {body && (
        <FoldBody open={open}>
          <div data-slot="preview-proofs" className="border-divider flex flex-col gap-3 border-t pt-3.5 pb-4.5 ps-13">
            {url && !preview && (
              <div className="flex items-center gap-2 text-xs">
                {where && <span data-slot="preview-proofs-where" className="text-foreground/65 font-medium">{where}</span>}
                <span data-slot="preview-proofs-count" className="text-muted-foreground">{counted}</span>
              </div>
            )}
            {ownLog && lastLines && preview && (
              <div data-slot="preview-run-output" className="flex flex-col gap-2">
                <div className="flex items-center gap-2 text-xs">
                  <span className="text-foreground/65 font-medium">Test output</span>
                  <span className="text-muted-foreground">last {LAST_LINES} lines</span>
                </div>
                <ShellBox command={preview.command} output={lastLines} status="inProgress" exit={null} />
              </div>
            )}
            {proofs.length > 0 && <Proofs proofs={proofs} conversationId={viewed} />}
          </div>
        </FoldBody>
      )}
    </SettingsCard>
  );
}

/** A web page's preview: a local server with an address (one that ended before it gave one is none, #450). */
const isPage = (preview: Preview) => !!preview.url && !isOtherPreview(preview);

/**
 * A finished turn's previews, each a card with the proofs taken of it: its page, then its other
 * previews. The page is the one on the local address its answer names (`answered`, read as that
 * address, route and all), when the turn started it or it is still running (#451); else the
 * latest the turn started or took proofs of. An address in the answer with no preview behind it
 * is no card (#438).
 */
export function TurnPreviews({ requestIds, steps, answered, session }: { requestIds: readonly string[]; steps: readonly BlockOrchestratorStep[]; answered: string | null; session: boolean }) {
  const pages = useTurnPreviews(requestIds, steps, isPage);
  const own = pages.toSorted((a, b) => a.startedAtMs - b.startedAtMs).at(-1);
  const said = answered !== null && pages.some((each) => samePage(each, answered));
  const serving = useBoard(
    (s) => answered !== null && Object.values(s.board?.previews ?? {}).some((each) => isPage(each) && !["stopped", "exited"].includes(each.state.type) && samePage(each, answered)),
  );
  const url = said || serving ? answered : (own?.url ?? null);
  const others = useTurnPreviews(requestIds, steps, isOtherPreview);
  const ofPreview = (id: string | null) =>
    steps.filter((step) =>
      step.kind.type !== "proof" ? id === null : id === null ? !others.some((other) => other.id === (step.kind as { preview?: string }).preview) : step.kind.preview === id,
    );
  return (
    <>
      <PreviewCard url={url} steps={ofPreview(null)} session={session} />
      {others.map((other) => (
        <PreviewCard key={other.id} url={null} preview={other} steps={ofPreview(other.id)} session={session} />
      ))}
    </>
  );
}

/**
 * The turn's previews of a sort (`which`): those its proofs were taken of, and those its requests
 * started, kept on their record, so Start again later leaves them its (#435). A record from before
 * that is its when it started while its requests ran, and stays its while the card shows.
 */
function useTurnPreviews(requestIds: readonly string[], steps: readonly BlockOrchestratorStep[], which: (preview: Preview) => boolean): Preview[] {
  const named = steps.flatMap((step) => (step.kind.type === "proof" && step.kind.preview ? [step.kind.preview] : []));
  const found = useBoard(
    useShallow((s) => {
      const requests = requestIds.flatMap((id) => (s.board?.requests[id] ? [s.board.requests[id]] : []));
      const from = Math.min(...requests.map((each) => each.startedAtMs));
      const to = requests.some((each) => each.endedAtMs === null) ? Infinity : Math.max(...requests.map((each) => each.endedAtMs ?? 0));
      return Object.values(s.board?.previews ?? {})
        .filter(which)
        .filter((each) => named.includes(each.id) || (each.requestId !== undefined ? requestIds.includes(each.requestId) : each.startedAtMs >= from && each.startedAtMs <= to))
        .map((each) => each.id);
    }),
  );
  const [kept, setKept] = useState<readonly string[]>([]);
  const ids = [...new Set([...kept, ...found])];
  if (ids.length !== kept.length) setKept(ids);
  return useBoard(useShallow((s) => ids.flatMap((id) => (s.board?.previews[id] ? [s.board.previews[id]] : []))));
}
