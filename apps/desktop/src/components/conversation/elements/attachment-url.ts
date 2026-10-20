import { createContext, useContext, useEffect, useState } from "react";

import type { AttachmentRef } from "@/ipc/generated";

/**
 * Where attachment bytes come from, provided by the app: the daemon's blob store, and the
 * stored reference behind a composer attachment that has no local file (one pulled back from
 * the queue).
 */
export type AttachmentReader = {
  read: (ref: AttachmentRef) => Promise<Blob>;
  composerRef: (attachmentId: string) => AttachmentRef | undefined;
};

export const AttachmentReaderContext = createContext<AttachmentReader | null>(null);

/** A tile's bytes: a file not stored yet, or a stored reference. */
export type AttachmentSource = { file?: File | undefined; ref?: AttachmentRef | undefined };

/** Object URLs of stored attachments by content hash, least recently used first. */
const storedUrls = new Map<string, Promise<string>>();
const STORED_URLS = 32;

function storedUrl(ref: AttachmentRef, read: AttachmentReader["read"]): Promise<string> {
  const cached = storedUrls.get(ref.id);
  if (cached) {
    storedUrls.delete(ref.id);
    storedUrls.set(ref.id, cached);
    return cached;
  }
  const url = read(ref).then((blob) => URL.createObjectURL(blob));
  url.catch(() => storedUrls.delete(ref.id));
  storedUrls.set(ref.id, url);
  const oldest = storedUrls.keys().next().value;
  if (storedUrls.size > STORED_URLS && oldest !== undefined) {
    void storedUrls.get(oldest)?.then(URL.revokeObjectURL, () => undefined);
    storedUrls.delete(oldest);
  }
  return url;
}

/** An object URL for the attachment's bytes, once there is one. */
export function useAttachmentUrl(source: AttachmentSource): string | null {
  return useAttachment(source).url;
}

/** An attachment's URL as `useAttachmentUrl` gives it, and whether reading it failed. */
export function useAttachment({ file, ref }: AttachmentSource): { url: string | null; failed: boolean } {
  const reader = useContext(AttachmentReaderContext);
  const [state, setState] = useState<{ url: string | null; failed: boolean }>({ url: null, failed: false });
  useEffect(() => {
    // A local file's URL is this tile's own; a stored one is shared through the cache.
    const local = file ? URL.createObjectURL(file) : null;
    const next = local ? Promise.resolve(local) : ref && reader ? storedUrl(ref, reader.read) : null;
    if (!next) return undefined;
    let live = true;
    next.then(
      (loaded) => live && setState({ url: loaded, failed: false }),
      () => live && setState({ url: null, failed: true }),
    );
    return () => {
      live = false;
      if (local) URL.revokeObjectURL(local);
    };
  }, [file, ref, reader]);
  return state;
}

