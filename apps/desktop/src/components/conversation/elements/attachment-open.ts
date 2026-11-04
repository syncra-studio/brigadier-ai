import { openArtifact } from "@/ipc/client";
import launchTypes from "@/lib/launchTypes.json" with { type: "json" };
import type { AttachmentRef } from "@/ipc/generated";
import { toast } from "@/state/toasts";

/**
 * How an attachment opens, the same from a Sources row and a sent message's card.
 */

/**
 * What the default app would run instead of showing, or can't be read as text: apps, installers,
 * compiled scripts and the links that start something. A blob is copied without an execute bit,
 * but these launch without one or ask to. The shell refuses them too (`opening.rs`), from the same list.
 */
const LAUNCHES = new Set(launchTypes.launches);

/** Source text that runs when launched: the reader shows it and never runs it, whatever its type says (browsers often give none). */
const SCRIPTS = new Set(launchTypes.scripts);

function extensionOf(name: string): string {
  const bare = name.replace(/[.\s]+$/, "");
  const dot = bare.lastIndexOf(".");
  return dot < 0 ? "" : bare.slice(dot + 1).toLowerCase();
}

/** The extension of the copy the daemon writes for the default app: it keeps the name's first 80 characters, which can leave another extension last. */
function writtenExtension(name: string): string {
  return extensionOf([...name.replace(/^\.+/, "")].slice(0, 80).join(""));
}

/** What a click on an attachment does: the image viewer, the read-only reader, the default app, or it refuses. */
export function attachmentOpens(file: AttachmentRef): "viewer" | "reader" | "app" | "refused" {
  if (file.mime.startsWith("image/")) return "viewer";
  if (file.pasted) return "reader";
  const extension = extensionOf(file.name);
  const written = writtenExtension(file.name);
  if (LAUNCHES.has(extension) || LAUNCHES.has(written)) return "refused";
  if (SCRIPTS.has(extension) || file.mime.startsWith("text/") || file.mime === "application/json") return "reader";
  // Named otherwise, a copy cut down to a script's name still must not reach the default app.
  return SCRIPTS.has(written) ? "refused" : "app";
}

export function refuse(file: AttachmentRef): void {
  toast(`${file.name} wasn't opened: it could run a program on this Mac.`, { tone: "error" });
}

export function openInDefaultApp(file: AttachmentRef): void {
  openArtifact(file.id, file.name).catch((error: unknown) =>
    toast(error instanceof Error ? error.message : String(error), { tone: "error" }),
  );
}
