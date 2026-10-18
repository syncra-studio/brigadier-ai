import { useAskRequest } from "@/app/conversation/activity/askRequest";
import { type Merged, revertRequest } from "@/app/conversation/activity/revert";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { Button } from "@/components/ui/button";
import type { MergeRecord, OrchestratorStepKind } from "@/ipc/generated";
import { VENDOR_LABELS } from "@/lib/routing";
import { useThreadImages } from "@/state/threadImages";

/**
 * What the thread attached to its reply, the merge message: the screenshots and recordings it
 * made of the change (`attach_proof`), under the answer, each with what it shows; and, once the
 * turn merged, what was checked of the merged commit and Revert.
 */

type Proof = Extract<OrchestratorStepKind, { type: "proof" }>;

/**
 * Revert on the turn's last merge: one click sends the thread the fixed request, which it
 * carries out (a revert can conflict with later merges: the thread judges). Once sent, here or
 * queued, it reads "Revert asked".
 */
function Revert({ conversationId, merge }: { conversationId: string; merge: Merged }) {
  const text = revertRequest(merge);
  const { asked, ask } = useAskRequest(conversationId, text);
  if (!text) return null;
  if (asked) {
    return <p data-slot="merge-revert" className="text-muted-foreground text-sm">Revert asked</p>;
  }
  return (
    <Button
      data-slot="merge-revert"
      size="sm"
      variant="ghost"
      className="w-fit font-normal"
      title={text}
      onClick={ask}
    >
      Revert
    </Button>
  );
}

const short = (sha: string) => sha.slice(0, 7);

/**
 * What was checked of the merged commit, one plain line each, as recorded when it merged: a
 * check on an older commit, a failure or something missing reads as a warning. Never a gate.
 */
function checkedLines(record: MergeRecord, pr: boolean): { text: string; warn: boolean }[] {
  const head = short(record.head);
  const on = (sha: string, onHead: boolean) => (onHead ? `on ${head}` : `on ${short(sha)}, not the merged commit`);
  const lines: { text: string; warn: boolean }[] = [];
  for (const check of record.checks) {
    const passed = check.status === "exit 0";
    lines.push({ text: `${check.command}: ${passed ? "passed" : check.status} ${check.onHead ? `on ${head}` : "on an earlier version"}`, warn: !passed || !check.onHead });
  }
  if (record.checks.length === 0) lines.push({ text: "No tests or type checks ran", warn: true });
  for (const review of record.reviews) {
    const findings = review.fixed + review.dismissed + review.open;
    const outcome =
      review.state.type === "clean"
        ? "clean"
        : review.state.type === "running"
          ? "still running"
          : review.state.type === "failed"
            ? "couldn't run"
            : `${findings} finding${findings === 1 ? "" : "s"}, ${review.fixed} fixed, ${review.dismissed} dismissed, ${review.open} open`;
    lines.push({
      text: `${VENDOR_LABELS[review.reviewer]} review of ${short(review.from)}..${short(review.to)}: ${outcome}${review.onHead ? "" : ", not up to the merged commit"}`,
      warn: !review.onHead || review.open > 0 || review.state.type === "running" || review.state.type === "failed",
    });
  }
  if (record.reviews.length === 0) lines.push({ text: "No review", warn: true });
  if (record.ci) {
    const result = record.ci.result ?? "still running";
    lines.push({ text: `CI ${result} ${on(record.ci.sha, record.ci.onHead)}`, warn: result !== "passed" || !record.ci.onHead });
  } else if (pr) {
    lines.push({ text: "No CI result", warn: true });
  }
  if (record.verifier) {
    const result = record.verifier.result;
    const said = result === null ? "no report yet" : { passed: "passed", failed: "failed", notRun: "couldn't run every check", noChecks: "no checks to run" }[result];
    lines.push({ text: `Fresh verifier: ${said} ${on(record.verifier.commit, record.verifier.onHead)}`, warn: result !== "passed" || !record.verifier.onHead });
  }
  lines.push({ text: record.proofs === 0 ? "No screenshot or recording" : `${record.proofs} screenshot${record.proofs === 1 ? "" : "s"} or recording${record.proofs === 1 ? "" : "s"}`, warn: false });
  return lines;
}

function Checked({ record, pr }: { record: MergeRecord; pr: boolean }) {
  return (
    <div data-slot="merge-checked" className="flex flex-col gap-0.5 text-sm">
      <p className="text-muted-foreground font-medium">Checked</p>
      <ul className="flex flex-col gap-0.5">
        {checkedLines(record, pr).map((line) => (
          <li key={line.text} data-warn={line.warn || undefined} className={line.warn ? "text-warning" : "text-muted-foreground"}>
            {line.text}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** The proofs of a finished turn, in the order the thread attached them, then Revert. */
export function MergeMessage({ steps }: { steps: readonly BlockOrchestratorStep[] }) {
  const conversationId = useViewConversation()?.id ?? null;
  const proofs = steps.flatMap((step) => (step.kind.type === "proof" ? [step.kind as Proof] : []));
  const merge = steps.flatMap((step) => (step.kind.type === "merged" && step.kind.mergeCommit ? [step.kind as Merged] : [])).at(-1);
  const media = useThreadImages(conversationId, proofs.map((proof) => proof.id));
  if (proofs.length === 0 && !merge) return null;
  const images = proofs.flatMap((proof, index) => (proof.mime.startsWith("image/") ? [{ name: proof.caption ?? proof.name, url: media[index]?.url ?? null }] : []));
  return (
    <div data-slot="merge-message" className="mt-activity flex flex-col gap-3">
      {proofs.map((proof, index) => {
        const { url, error } = media[index] ?? { url: null, error: null };
        const label = proof.caption ?? proof.name;
        const frame = "border-border max-h-80 max-w-full rounded-lg border";
        let shown;
        if (!url) {
          shown = (
            <div title={error ?? undefined} className="border-border bg-foreground/5 text-muted-foreground flex h-24 w-40 items-center justify-center rounded-lg border p-1 text-center text-xs">
              {error ? "Not available" : <span className="shimmer">Loading</span>}
            </div>
          );
        } else if (proof.mime.startsWith("video/")) {
          shown = <video src={url} controls muted playsInline preload="metadata" aria-label={label} className={frame} />;
        } else {
          const at = images.findIndex((image) => image.url === url);
          shown = (
            <ImagePreview url={url} name={label} gallery={{ images, index: Math.max(at, 0) }}>
              <button type="button" aria-label={`Open ${label}`} className="focus-visible:ring-ring w-fit rounded-lg focus-visible:ring-2 focus-visible:outline-none">
                <img src={url} alt={label} className={frame} />
              </button>
            </ImagePreview>
          );
        }
        return (
          <figure key={proof.id} data-slot="proof" data-kind={proof.mime.startsWith("video/") ? "video" : "image"} className="flex flex-col gap-1">
            {shown}
            {proof.caption && <figcaption className="text-muted-foreground text-sm">{proof.caption}</figcaption>}
          </figure>
        );
      })}
      {merge?.record && <Checked record={merge.record} pr={merge.pr !== undefined} />}
      {merge && conversationId && merge.record?.firstAuto ? (
        // The project's first merge on its own says so once, with its undo beside it.
        <div data-slot="merge-first-auto" className="flex items-center gap-2">
          <p className="text-sm">Merged on its own, as your setting has it.</p>
          <Revert conversationId={conversationId} merge={merge} />
        </div>
      ) : (
        merge && conversationId && <Revert conversationId={conversationId} merge={merge} />
      )}
    </div>
  );
}
