import { useCallback, useEffect, useMemo, useState } from "react";

import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { ArtifactRef, ArtifactText } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";
import { filePatch, patchPaths } from "@/lib/patch";
import { readArtifact } from "@/state/actions";

/** Bytes read per page: enough for most reports and diffs, small enough to stay responsive. */
const PAGE_BYTES = 256 * 1024;

/** Shows an artifact's text (transcript, diff, command output, note), a page at a time. */
export function ArtifactDialog({
  artifact,
  onOpenChange,
}: {
  artifact: ArtifactRef | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={artifact !== null} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-thread flex max-h-full flex-col">
        {artifact && <ArtifactBody key={artifact.id} artifact={artifact} />}
      </DialogContent>
    </Dialog>
  );
}

function ArtifactBody({ artifact }: { artifact: ArtifactRef }) {
  const [text, setText] = useState("");
  const [offset, setOffset] = useState(0);
  const [total, setTotal] = useState<number | null>(null);
  const [binary, setBinary] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const apply = useCallback((from: number, page: ArtifactText) => {
    setBinary(page.binary);
    setTotal(page.totalBytes);
    setText((previous) => (from === 0 ? page.text : previous + page.text));
    // A page can end inside a character; the next read starts after what decoded.
    setOffset(from + (page.binary ? 0 : new TextEncoder().encode(page.text).length));
  }, []);

  const read = useCallback(
    (from: number, live: () => boolean) =>
      readArtifact(artifact.id, from, PAGE_BYTES)
        .then((page) => live() && apply(from, page))
        .catch((cause: unknown) => {
          if (live()) setError(cause instanceof Error ? cause.message : String(cause));
        })
        .finally(() => {
          if (live()) setLoading(false);
        }),
    [artifact.id, apply],
  );

  useEffect(() => {
    let live = true;
    void read(0, () => live);
    return () => {
      live = false;
    };
  }, [read]);

  const loadMore = () => {
    setLoading(true);
    setError(null);
    void read(offset, () => true);
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{artifact.title}</DialogTitle>
        <DialogDescription>
          {artifact.kind} · {artifact.mime} · {formatBytes(artifact.bytes)}
        </DialogDescription>
      </DialogHeader>
      {error && (
        <p role="alert" className="text-destructive text-sm">
          {error}
        </p>
      )}
      {binary ? (
        <p className="text-muted-foreground text-sm">This artifact is not text.</p>
      ) : artifact.kind === "diff" && text ? (
        <PatchFiles patch={text} />
      ) : (
        <pre
          data-selectable
          className="bg-code-surface rounded-control min-h-0 flex-1 overflow-auto p-3 font-mono text-xs whitespace-pre-wrap"
        >
          {text || (loading ? "Loading…" : "")}
        </pre>
      )}
      {!binary && total !== null && offset < total && (
        <Button
          variant="ghost"
          size="sm"
          className="self-center"
          disabled={loading}
          onClick={loadMore}
        >
          Load more ({formatBytes(total - offset)} left)
        </Button>
      )}
    </>
  );
}

/** A diff artifact as the thread shows changes: each file's diff in its colours, numbered. */
function PatchFiles({ patch }: { patch: string }) {
  const files = useMemo(() => patchPaths(patch).map((path) => ({ path, diff: filePatch(patch, path) })), [patch]);
  if (files.length === 0) {
    return <pre data-selectable className="bg-code-surface rounded-control min-h-0 flex-1 overflow-auto p-3 font-mono text-xs whitespace-pre-wrap">{patch}</pre>;
  }
  return (
    <div data-slot="artifact-diff" data-selectable className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto">
      {files.map((file) => (
        <InlineDiff key={file.path} path={file.path} diff={file.diff} tall />
      ))}
    </div>
  );
}
