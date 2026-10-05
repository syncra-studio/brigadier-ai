import { type ReactNode, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Conversation, UnlandedBranch } from "@/ipc/generated";
import { conversationNoun, deleteAll, previewDelete } from "@/state/actions";
import { clearPicked, useDeleteAsk } from "@/state/picking";
import { useApp } from "@/state/store";

/** Branches named in the line before it gives a count instead. */
const NAMED_BRANCHES = 3;

/**
 * Confirms deleting the conversations {@link askDelete} names: their transcripts and everything
 * they made go for good, with the branches Brigadier created for them. One plain line says so
 * when one of those branches holds work that never landed.
 */
export function DeleteDialog() {
  const ids = useDeleteAsk((s) => s.ids);
  return (
    <Dialog open={ids !== null} onOpenChange={(open) => !open && closeDeleteAsk()}>
      <DialogContent className="max-w-md">
        {ids && <DeleteForm key={ids.join(" ")} ids={ids} onClose={closeDeleteAsk} />}
      </DialogContent>
    </Dialog>
  );
}

function closeDeleteAsk(): void {
  useDeleteAsk.setState({ ids: null });
}

function DeleteForm({ ids, onClose }: { ids: string[]; onClose: () => void }) {
  // As they were when asked: the rows leave the store once confirmed.
  const [conversations] = useState(() => {
    const all = useApp.getState().conversations;
    return ids.map((id) => all[id]).filter((c): c is Conversation => c !== undefined);
  });
  // Delete waits for the preview, so the line about unlanded work is never skipped; `failed`
  // when it couldn't be told.
  const [branches, setBranches] = useState<UnlandedBranch[] | "failed" | null>(null);
  useEffect(() => {
    let current = true;
    previewDelete(ids)
      .then((found) => current && setBranches(found))
      .catch(() => current && setBranches("failed"));
    return () => {
      current = false;
    };
  }, [ids]);
  const deleteButton = useRef<HTMLButtonElement>(null);
  const ready = branches !== null;
  useEffect(() => {
    if (ready) deleteButton.current?.focus();
  }, [ready]);

  const count = conversations.length;
  const noun = conversationNoun(conversations);
  const confirm = () => {
    onClose();
    clearPicked();
    void deleteAll(conversations.map((c) => c.id));
  };

  return (
    <div className="grid gap-4">
      <DialogHeader>
        <DialogTitle>{count === 1 ? `Delete ${noun}?` : `Delete ${count} ${noun}?`}</DialogTitle>
        <DialogDescription>
          {count === 1
            ? `“${conversations[0]?.title}” and its transcript are removed for good.`
            : `These ${count} ${noun} and their transcripts are removed for good.`}
        </DialogDescription>
      </DialogHeader>
      {branches === "failed"
        ? conversations.some((c) => c.kind === "session") && (
            <p className="text-muted-foreground text-sm">
              {count === 1 ? "Its branches" : "Their branches"} may have work that never landed;
              they're deleted too.
            </p>
          )
        : branches &&
          branches.length > 0 && <UnlandedLine branches={branches} single={count === 1} />}
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button
          ref={deleteButton}
          type="button"
          variant="destructive"
          disabled={!ready}
          onClick={confirm}
        >
          Delete
        </Button>
      </DialogFooter>
    </div>
  );
}

/**
 * One line about the branches whose work never landed (or may not have): they go too. `single`:
 * one conversation is deleted ("Its branch …").
 */
function UnlandedLine({ branches, single }: { branches: UnlandedBranch[]; single: boolean }) {
  const known = branches.some((branch) => !branch.unknown);
  const has = known ? "has" : "may have";
  const have = known ? "have" : "may have";
  const [first] = branches;
  let line: ReactNode;
  if (branches.length === 1 && first) {
    line = (
      <>
        {single ? "Its" : "The"} branch {branchName(first)} {has} work that never landed; it's deleted
        too.
      </>
    );
  } else if (branches.length <= NAMED_BRANCHES) {
    const names = branches.map(branchName);
    line = (
      <>
        Branches{" "}
        {names.slice(0, -1).flatMap((entry, index) => (index === 0 ? [entry] : [", ", entry]))} and{" "}
        {names.at(-1)} {have} work that never landed; they're deleted too.
      </>
    );
  } else {
    line = (
      <>
        {branches.length} branches {have} work that never landed; they're deleted too.
      </>
    );
  }
  return <p className="text-muted-foreground text-sm">{line}</p>;
}

function branchName(branch: UnlandedBranch): ReactNode {
  return (
    <code key={`${branch.repo} ${branch.name}`} className="text-foreground/85 font-mono text-xs">
      {branch.name}
    </code>
  );
}
