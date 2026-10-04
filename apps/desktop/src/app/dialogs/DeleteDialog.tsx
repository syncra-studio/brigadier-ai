import { useState } from "react";

import { CheckboxRow, ErrorLine, errorText, RadioChoice } from "@/app/dialogs/fields";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Conversation } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";
import { deleteConversation } from "@/state/actions";
import { compactDatabase } from "@/state/storage";
import { toast } from "@/state/toasts";

type BranchChoice = "keep" | "delete";

/** Deletes a session or chat for good, asking what to do with what it leaves behind. */
export function DeleteDialog({
  conversation,
  onOpenChange,
}: {
  /** The conversation to delete; the dialog is open while it is set. */
  conversation: Conversation | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={conversation !== null} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        {conversation && (
          <DeleteForm
            key={conversation.id}
            conversation={conversation}
            onOpenChange={onOpenChange}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function DeleteForm({
  conversation,
  onOpenChange,
}: {
  conversation: Conversation;
  onOpenChange: (open: boolean) => void;
}) {
  const session = conversation.kind === "session";
  const [branches, setBranches] = useState<BranchChoice>("keep");
  const [forgetBrain, setForgetBrain] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      const compactable = await deleteConversation(
        conversation.id,
        session && branches === "delete",
        forgetBrain,
      );
      onOpenChange(false);
      if (compactable !== null) {
        toast(`Deleted. Compact the database to give back ${formatBytes(compactable)}.`, {
          actions: [{ label: "Compact", run: () => void compactDatabase() }],
        });
      }
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4">
      <DialogHeader>
        <DialogTitle>Delete {session ? "session" : "chat"}?</DialogTitle>
        <DialogDescription>
          “{conversation.title}” and its transcript are removed for good. This can't be undone;
          archive it instead to keep it restorable.
        </DialogDescription>
      </DialogHeader>
      {session && (
        <>
          <div className="grid gap-2">
            <p className="text-muted-foreground text-xs">Unmerged branches</p>
            <RadioChoice<BranchChoice>
              label="Unmerged branches"
              value={branches}
              onChange={setBranches}
              options={[
                {
                  value: "keep",
                  label: "Keep unmerged branches",
                  hint: "Its session, task and overnight run branches stay in the repository.",
                },
                {
                  value: "delete",
                  label: "Delete unmerged branches",
                  hint: "Work that never landed is lost.",
                },
              ]}
            />
          </div>
          <CheckboxRow
            label="Forget what the Brain learned from this session"
            note="Brain notes that came only from this session are removed too."
            checked={forgetBrain}
            onCheckedChange={setForgetBrain}
          />
        </>
      )}
      <ErrorLine error={error} />
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          type="button"
          variant="destructive"
          disabled={busy}
          onClick={() => void confirm()}
        >
          Delete
        </Button>
      </DialogFooter>
    </div>
  );
}
