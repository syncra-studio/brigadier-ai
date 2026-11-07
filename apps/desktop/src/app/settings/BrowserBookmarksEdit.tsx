import { useId, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { Button, ErrorLine, Input } from "@/components/controls";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/overlays";
import { type Bookmark, bookmarkAddress, type BookmarkEdit, FOLDER_SEPARATOR, folderLabel } from "@/state/browserBookmarks";

/** Adds a bookmark, or edits `bookmark`. `onSave` does the work; a failure stays in the dialog. */
export function BrowserBookmarksEdit({
  open,
  bookmark,
  onOpenChange,
  onSave,
}: {
  open: boolean;
  bookmark: Bookmark | null;
  onOpenChange: (open: boolean) => void;
  onSave: (edit: BookmarkEdit) => Promise<void>;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{bookmark ? "Edit bookmark" : "Add bookmark"}</DialogTitle>
          <DialogDescription className="sr-only">The bookmark's name, address and folder.</DialogDescription>
        </DialogHeader>
        {open && <EditForm bookmark={bookmark} onCancel={() => onOpenChange(false)} onSave={onSave} />}
      </DialogContent>
    </Dialog>
  );
}

function EditForm({
  bookmark,
  onCancel,
  onSave,
}: {
  bookmark: Bookmark | null;
  onCancel: () => void;
  onSave: (edit: BookmarkEdit) => Promise<void>;
}) {
  const ids = useId();
  const [title, setTitle] = useState(bookmark?.title ?? "");
  const [url, setUrl] = useState(bookmark?.url ?? "");
  const [folder, setFolder] = useState(bookmark ? folderLabel(bookmark.folders) : "Bookmarks");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const address = bookmarkAddress(url);
  const submit = () => {
    if (!address) return;
    const folders = folder.split(FOLDER_SEPARATOR.trim()).map((part) => part.trim()).filter(Boolean);
    setBusy(true);
    onSave({ url: address, title: title.trim(), folders: folders.join() === "Bookmarks" ? [] : folders }).catch(
      (cause: unknown) => {
        setError(errorText(cause));
        setBusy(false);
      },
    );
  };
  const fields = [
    { id: `${ids}-title`, label: "Name", value: title, set: setTitle, hint: "Rust docs" },
    { id: `${ids}-url`, label: "Address", value: url, set: setUrl, hint: "https://doc.rust-lang.org" },
    { id: `${ids}-folder`, label: "Folder", value: folder, set: setFolder, hint: "Bookmarks bar › Work" },
  ];
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      {fields.map((field) => (
        <label key={field.id} htmlFor={field.id} className="flex flex-col gap-1">
          <span className="text-label">{field.label}</span>
          <Input
            id={field.id}
            value={field.value}
            placeholder={field.hint}
            spellCheck={false}
            onChange={(event) => field.set(event.target.value)}
          />
        </label>
      ))}
      <ErrorLine error={url && !address ? "That isn't a web address." : error} />
      <DialogFooter>
        <Button type="button" variant="ghost" disabled={busy} onClick={onCancel}>Cancel</Button>
        <Button type="submit" disabled={busy || !address}>{bookmark ? "Save" : "Add bookmark"}</Button>
      </DialogFooter>
    </form>
  );
}
