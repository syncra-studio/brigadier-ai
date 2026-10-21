import { useContext } from "react";

import { OpenFileContext } from "@/components/conversation";
import { revealPath } from "@/ipc/client";

/** The file's name, underlined, opening the file: in its tab when it is the session's, else in Finder. */
export function FileLink({ path, name }: { path: string; name: string }) {
  const openFile = useContext(OpenFileContext);
  return (
    <button
      type="button"
      data-slot="edited-file-link"
      title={path}
      onClick={(event) => {
        // The name opens the file; the rest of the row opens its diff.
        event.stopPropagation();
        if (!openFile(path, null)) revealPath(path).catch(() => {});
      }}
      onKeyDown={(event) => event.stopPropagation()}
      className="text-row-object hover:text-foreground focus-visible:text-foreground underline decoration-current/40 underline-offset-2"
    >
      {name}
    </button>
  );
}
