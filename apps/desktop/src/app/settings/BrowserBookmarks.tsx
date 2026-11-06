import { useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { BrowserBookmarksEdit } from "@/app/settings/BrowserBookmarksEdit";
import { BrowserBookmarksRow } from "@/app/settings/BrowserBookmarksRow";
import { BrowserPasswordsProfile, useBrowserProfilePick } from "@/app/settings/BrowserPasswordsProfile";
import { DayGroup } from "@/app/settings/DayGroup";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Button, ErrorLine } from "@/components/controls";
import { Plus } from "@/components/icons";
import { EmptyState, SettingsCard, SettingsSearch } from "@/components/shared";
import {
  addBookmark,
  type Bookmark,
  type BookmarkEdit,
  bookmarkFolders,
  browserBookmarks,
  removeBookmark,
  updateBookmark,
} from "@/state/browserBookmarks";

/** Settings › Browser › Bookmarks: one profile's bookmarks, by folder, imported ones among them. */
export function BrowserBookmarks({ profile: first, onBack }: { profile?: string | undefined; onBack: () => void }) {
  const { profiles, profile, setProfile } = useBrowserProfilePick(first);
  const [read, setRead] = useState<{ from: string; bookmarks: Bookmark[] } | null>(null);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Bookmark | "new" | null>(null);

  // What another profile's read left isn't shown: the list is empty until this one's comes.
  const bookmarks = read?.from === profile ? read.bookmarks : null;
  const setBookmarks = (change: (shown: Bookmark[]) => Bookmark[]) =>
    setRead((last) => last && { from: last.from, bookmarks: change(last.bookmarks) });

  useEffect(() => {
    if (!profile) return;
    browserBookmarks(profile).then(
      (found) => setRead({ from: profile, bookmarks: found }),
      (cause: unknown) => setError(errorText(cause)),
    );
  }, [profile]);

  const save = async (edit: BookmarkEdit) => {
    if (!profile || !editing) return;
    if (editing === "new") {
      const added = await addBookmark(profile, edit);
      setBookmarks((shown) => [...shown, added]);
    } else {
      await updateBookmark(profile, editing.id, edit);
      setBookmarks((shown) => shown.map((one) => (one.id === editing.id ? { ...one, ...edit } : one)));
    }
    setEditing(null);
  };

  const remove = (id: number) => {
    if (!profile) return;
    removeBookmark(profile, id)
      .then(() => setBookmarks((shown) => shown.filter((one) => one.id !== id)))
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  const folders = bookmarkFolders(bookmarks ?? [], query);
  return (
    <SettingsPage
      title="Bookmarks"
      description="Pages you keep in Brigadier's browser, and the ones you import from other browsers."
      back={{ label: "Browser", onBack }}
    >
      <div className="flex items-center gap-2">
        <SettingsSearch label="Search bookmarks" value={query} onChange={setQuery} />
        <BrowserPasswordsProfile profiles={profiles} profile={profile} onChange={setProfile} />
        <Button variant="soft" disabled={!profile} onClick={() => setEditing("new")}>
          <Plus />
          Add bookmark
        </Button>
      </div>
      <SettingsSection title="All bookmarks" aside={bookmarks ? String(bookmarks.length) : undefined}>
        <ErrorLine error={error} />
        {bookmarks && folders.length === 0 && (
          <SettingsCard>
            <EmptyState
              variant="card"
              title={query ? "No bookmarks match." : "Bookmarks you add or import show here."}
            />
          </SettingsCard>
        )}
        {folders.map((folder) => (
          <DayGroup key={folder.key} label={folder.label}>
            {folder.bookmarks.map((bookmark) => (
              <BrowserBookmarksRow
                key={bookmark.id}
                bookmark={bookmark}
                onEdit={() => setEditing(bookmark)}
                onRemove={() => remove(bookmark.id)}
              />
            ))}
          </DayGroup>
        ))}
      </SettingsSection>
      <BrowserBookmarksEdit
        open={editing !== null}
        bookmark={editing === "new" ? null : editing}
        onOpenChange={(open) => !open && setEditing(null)}
        onSave={save}
      />
    </SettingsPage>
  );
}
