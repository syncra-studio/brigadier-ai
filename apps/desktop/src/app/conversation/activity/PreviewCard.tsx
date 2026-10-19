import { ArrowUpRight, ChevronDown, ChevronUp, Globe, ImageSquare, Play, Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useState } from "react";

import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { FoldBody, FoldHeader } from "@/app/conversation/cardFold";
import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { useViewConversation } from "@/app/conversation/viewContext";
import { Pill, RoundBadge, RowText, StorageCard } from "@/app/settings/storage/parts";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { buttonVariants } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import type { OrchestratorStepKind, Preview, ProofKind } from "@/ipc/generated";
import { openUrl } from "@/ipc/client";
import { formatTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { resumePreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openLink } from "@/state/links";
import { openBrowserTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { useThreadImages } from "@/state/threadImages";
import { toast } from "@/state/toasts";

/**
 * The Preview card, under a finished turn's answer and above its Changes card (the user's design,
 * `K53xQ`): a round badge, what the turn left to look at ("Web preview"), its state ("Running",
 * "Stopped") and address, and the proofs the thread took of it (`attach_proof`) inside the card,
 * captioned thumbnails cropped to the top of the page. Running: Open, a split pill whose ▾ picks
 * where. Paused: Start again. The chevron folds the proofs away; folded, the header counts them.
 * Proofs with no page left running are the card's alone ("Screenshots").
 */

type Proof = Extract<OrchestratorStepKind, { type: "proof" }>;

function failed(cause: unknown): void {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

/** What a proof is: as it was recorded, else (a proof from before that) by its file's type. */
function kindOf(proof: Proof): ProofKind {
  if (proof.kind) return proof.kind;
  return proof.mime.startsWith("video/") ? "recording" : proof.mime.startsWith("image/") ? "screenshot" : "log";
}

/** Log and test output: read as text, not looked at. */
const printed = (proof: Proof) => ["log", "test"].includes(kindOf(proof));

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

/** The lines of log or test output shown: its last ones. */
const LAST_LINES = 5;

/** A text file read as a data URL, as words. */
function textOf(url: string): string {
  const data = url.slice(url.indexOf(",") + 1);
  return new TextDecoder().decode(Uint8Array.from(atob(data), (char) => char.charCodeAt(0)));
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

/** The design's split pill: Open opens the page where Settings › Browser sends it; ▾ picks where. */
const OpenPill: FC<{ url: string; conversationId: string | null }> = ({ url, conversationId }) => {
  const browser = useApp((s) => s.settings.browserEnabled && s.info?.platform !== "linux");
  const inApp = browser && conversationId !== null;
  // A Browser tab already on the page comes to the front (#425).
  const open = () => void openLink(url, conversationId ? { agent: true, conversationId } : { agent: true }).catch(failed);
  const half = "hover:bg-foreground/10 flex h-full items-center";
  return (
    <span
      data-slot="preview-open"
      className={cn(buttonVariants({ variant: "outline", size: "sm" }), "rounded-nav bg-foreground/5 text-label gap-0 overflow-hidden px-0 font-normal")}
    >
      <button type="button" onClick={open} title={url} className={cn(half, "gap-1.5 ps-3 pe-2")}>
        <ArrowUpRight aria-hidden className="size-icon-sm" />
        Open
      </button>
      <span aria-hidden className="bg-divider h-3.5 w-px" />
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label="Open in" className={cn(half, "text-foreground/65 ps-1.5 pe-2")}>
            <ChevronDown aria-hidden className="size-icon-sm" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {inApp && <DropdownMenuItem onSelect={() => conversationId && openBrowserTab(conversationId, url, true)}>Brigadier browser</DropdownMenuItem>}
          <DropdownMenuItem onSelect={() => void openUrl(url).catch(failed)}>Default browser</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  );
};

/** The proofs, two to a row: each cropped to the top of what it shows, "1. What it shows" and when under it. */
const Proofs: FC<{ proofs: readonly { proof: Proof; atMs: number | undefined }[]; conversationId: string | null }> = ({ proofs, conversationId }) => {
  const media = useThreadImages(conversationId, proofs.map(({ proof }) => proof.id));
  const images = proofs.flatMap(({ proof }, index) => (kindOf(proof) === "screenshot" ? [{ name: proof.caption ?? proof.name, url: media[index]?.url ?? null }] : []));
  const frame = "border-border block h-44.5 w-full overflow-hidden rounded-lg border";
  return (
    <div data-slot="turn-proofs" className="grid grid-cols-2 gap-3">
      {proofs.map(({ proof, atMs }, index) => {
        const { url, error } = media[index] ?? { url: null, error: null };
        // What it shows in the thread's words; a proof taken without them goes by its file's name.
        const label = proof.caption ?? proof.name;
        const kind = kindOf(proof);
        let shown: ReactNode;
        if (url && printed(proof)) {
          // Its last lines, under the command or file it came from.
          const lines = textOf(url).trimEnd().split("\n");
          shown = <ShellBox command={proof.takenOn ?? proof.name} output={lines.slice(-LAST_LINES).join("\n")} status="inProgress" exit={null} />;
        } else if (!url) {
          shown = (
            <div title={error ?? label} className={cn(frame, "bg-foreground/5 text-muted-foreground flex items-center justify-center p-1 text-center text-xs")}>
              {error ? "Not available" : <span className="shimmer">Loading</span>}
            </div>
          );
        } else if (kind === "recording") {
          shown = <video src={url} controls muted playsInline preload="metadata" aria-label={label} title={label} className={cn(frame, "object-cover object-top")} />;
        } else {
          const at = images.findIndex((image) => image.url === url);
          shown = (
            <ImagePreview url={url} name={label} gallery={{ images, index: Math.max(at, 0) }}>
              <button type="button" aria-label={`Open ${label}`} title={label} className={cn(frame, "focus-visible:ring-ring group/proof relative focus-visible:ring-2 focus-visible:outline-none")}>
                <img src={url} alt={label} className="size-full object-cover object-top" />
                <span aria-hidden className="group-hover/proof:bg-background/20 pointer-events-none absolute inset-0 transition-colors" />
              </button>
            </ImagePreview>
          );
        }
        return (
          <figure key={proof.id} data-slot="proof" data-kind={kind} data-caption={proof.caption} className={cn("flex min-w-0 flex-col gap-2", printed(proof) && "col-span-2")}>
            {shown}
            <figcaption className="flex min-w-0 items-center gap-1.5 text-xs">
              <span className="text-muted-foreground shrink-0">{index + 1}.</span>
              <span className="text-foreground min-w-0 truncate">{label}</span>
              {atMs !== undefined && <span className="text-muted-foreground shrink-0">{formatTime(atMs)}</span>}
            </figcaption>
          </figure>
        );
      })}
    </div>
  );
};

/**
 * What a finished turn left to look at: the one local page its answer names (`url`) and the
 * proofs it took (`steps`). Neither, and there is no card. `session`: the turn is a session's,
 * whose Browser tabs can show the page.
 */
export function PreviewCard({ url, steps, session }: { url: string | null; steps: readonly BlockOrchestratorStep[]; session: boolean }) {
  const viewed = useViewConversation()?.id ?? null;
  const conversationId = session ? viewed : null;
  const proofs = steps.flatMap((step) => (step.kind.type === "proof" ? [{ proof: step.kind as Proof, atMs: (step.kind as Proof).takenAtMs ?? step.atMs }] : []));
  const record = useBoard((s) =>
    url ? Object.values(s.board?.previews ?? {}).filter((each) => samePage(each, url)).toSorted((a, b) => a.startedAtMs - b.startedAtMs).at(-1) ?? null : null,
  );
  const [open, setOpen] = useState(true);
  if (!url && proofs.length === 0) return null;

  const counted = proofCount(proofs.map(({ proof }) => proof));
  const state = record ? standing(record) : null;
  const stopped = state !== null && !state.live && record?.state.type !== "starting";
  const address = url?.replace(/^https?:\/\//, "").replace(/\/$/, "") ?? null;
  const title = url ? "Web preview" : proofsTitle(proofs.map(({ proof }) => proof));
  const where = takenWhere(proofs.map(({ proof }) => proof));
  const logsOnly = proofs.length > 0 && proofs.every(({ proof }) => printed(proof));

  let action: ReactNode = null;
  if (url && record?.state.type === "paused" && viewed) {
    action = (
      <Pill data-slot="preview-start" onClick={() => void resumePreview(viewed, record.id).catch(failed)}>
        <Play aria-hidden className="size-icon-sm" />
        Start again
      </Pill>
    );
  } else if (url && !stopped) {
    action = <OpenPill url={url} conversationId={conversationId} />;
  }
  // TODO(w105): Start again for a stopped preview, restarting its own command from its record.

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
      <span key="address" data-slot="preview-target" title={url ?? undefined} className="text-muted-foreground min-w-0 truncate font-mono">
        {address}
      </span>,
    );
  }
  // Folded (or a card of proofs alone), the header counts them.
  if (counted && (!open || !url)) {
    line.push(
      <span key="count" data-slot="preview-count" className="text-muted-foreground shrink-0">
        {counted}
      </span>,
    );
  }

  return (
    <StorageCard data-slot="preview-card" data-state={state ? (stopped ? "stopped" : "running") : undefined} data-open={(open && proofs.length > 0) || undefined}>
      <FoldHeader foldable={proofs.length > 0} onToggle={() => setOpen(!open)} className="flex items-center gap-4 py-4">
        <RoundBadge>{url ? <Globe className={cn(stopped && "text-foreground/65")} /> : logsOnly ? <Terminal /> : <ImageSquare />}</RoundBadge>
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
          {proofs.length > 0 && (
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
      {proofs.length > 0 && (
        <FoldBody open={open}>
          <div data-slot="preview-proofs" className="border-divider flex flex-col gap-3 border-t pt-3.5 pb-4.5 ps-13">
            {url && (
              <div className="flex items-center gap-2 text-xs">
                {where && <span data-slot="preview-proofs-where" className="text-foreground/65 font-medium">{where}</span>}
                <span data-slot="preview-proofs-count" className="text-muted-foreground">{counted}</span>
              </div>
            )}
            <Proofs proofs={proofs} conversationId={viewed} />
          </div>
        </FoldBody>
      )}
    </StorageCard>
  );
}
