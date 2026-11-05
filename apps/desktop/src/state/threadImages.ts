import { useEffect, useState } from "react";

import { request } from "@/ipc/client";
import { cliEntry, itemIdOf, readSessionImage, useWorkerThreads } from "@/state/cliThread";

/**
 * Images the thread looked at or made, read from the daemon once each, as data URLs its rows'
 * thumbnails and the image viewer show: an image its item carries in the CLI's session file from
 * that file (`readSessionFileImage`), one on disk at the path its item names (`readThreadImage`).
 */

/** The images kept at once; the oldest read goes first. */
const KEPT = 64;

const read = new Map<string, Promise<string>>();

/** An item of a worker's own transcript the app has read: those tiles ask under the session. */
function workerEntry(itemId: string) {
  for (const view of Object.values(useWorkerThreads.getState().views)) {
    const entry = view.entries.find((each) => itemIdOf(each.event) === itemId);
    if (entry) return entry;
  }
  return undefined;
}

export function loadThreadImage(conversationId: string, itemId: string): Promise<string> {
  const key = `${conversationId}|${itemId}`;
  const known = read.get(key);
  if (known) return known;
  const image = (cliEntry(conversationId, itemId) ?? workerEntry(itemId))?.images?.[0];
  const promise = image
    ? readSessionImage(conversationId, image)
    : request({ method: "readThreadImage", conversationId, itemId }).then(({ data, mime }) => `data:${mime};base64,${data}`);
  // A failed read is tried again the next time the image is shown.
  promise.catch(() => read.delete(key));
  read.set(key, promise);
  for (const old of read.keys()) {
    if (read.size <= KEPT) break;
    if (old !== key) read.delete(old);
  }
  return promise;
}

/** A thread image's data URL; `error` says why it can't be shown ("This image is no longer there"). */
export function useThreadImage(conversationId: string | null, itemId: string): { url: string | null; error: string | null } {
  const [state, setState] = useState<{ url: string | null; error: string | null; for: string }>({ url: null, error: null, for: "" });
  const key = `${conversationId}|${itemId}`;
  useEffect(() => {
    if (!conversationId) return;
    let current = true;
    loadThreadImage(conversationId, itemId).then(
      (url) => current && setState({ url, error: null, for: key }),
      (cause: unknown) => current && setState({ url: null, error: cause instanceof Error ? cause.message : String(cause), for: key }),
    );
    return () => {
      current = false;
    };
  }, [conversationId, itemId, key]);
  return state.for === key ? state : { url: null, error: null };
}

/** Several thread images' data URLs, in order; an image not read yet (or gone) is `null`, with why. */
export function useThreadImages(conversationId: string | null, itemIds: readonly string[]): { url: string | null; error: string | null }[] {
  const ids = itemIds.join(" ");
  const [state, setState] = useState<{ for: string; images: { url: string | null; error: string | null }[] }>({ for: "", images: [] });
  const key = `${conversationId}|${ids}`;
  useEffect(() => {
    if (!conversationId) return;
    let current = true;
    const wanted = ids ? ids.split(" ") : [];
    Promise.all(
      wanted.map((itemId) =>
        loadThreadImage(conversationId, itemId).then(
          (url) => ({ url, error: null }),
          (cause: unknown) => ({ url: null, error: cause instanceof Error ? cause.message : String(cause) }),
        ),
      ),
    ).then((images) => current && setState({ for: key, images }));
    return () => {
      current = false;
    };
  }, [conversationId, ids, key]);
  return state.for === key ? state.images : itemIds.map(() => ({ url: null, error: null }));
}

/** Local Markdown sources never go to the webview's img src. */
export function localImagePath(src: string): string | null {
  try {
    if (src.startsWith("file:")) {
      const url = new URL(src);
      if (url.hostname && url.hostname !== "localhost") return null;
      const path = decodeURIComponent(url.pathname);
      return /^\/[a-z]:\//i.test(path) ? path.slice(1) : path;
    }
    return src.startsWith("/") || /^[a-z]:[\\/]/i.test(src) ? decodeURIComponent(src) : null;
  } catch {
    return null;
  }
}

/** A local answer image, loaded through the daemon with the same bounded cache as tiles. */
export function useLocalImage(conversationId: string | null, path: string | null): { url: string | null; error: string | null } {
  const [state, setState] = useState<{ for: string; url: string | null; error: string | null }>({ for: "", url: null, error: null });
  const key = `local|${conversationId}|${path}`;
  useEffect(() => {
    if (!conversationId || !path) return;
    let current = true;
    let promise = read.get(key);
    if (!promise) {
      promise = request({ method: "readLocalImage", conversationId, path }).then(({ data, mime }) => `data:${mime};base64,${data}`);
      promise.catch(() => read.delete(key));
      read.set(key, promise);
      for (const old of read.keys()) {
        if (read.size <= KEPT) break;
        if (old !== key) read.delete(old);
      }
    }
    promise.then(
      (url) => current && setState({ for: key, url, error: null }),
      () => current && setState({ for: key, url: null, error: "Not available" }),
    );
    return () => { current = false; };
  }, [conversationId, path, key]);
  return state.for === key ? state : { url: null, error: null };
}
