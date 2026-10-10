import { useEffect, useState } from "react";

import { readStoredImage } from "@/state/actions";
import { markerNumber, type MockupNote } from "@/state/mockupNotes";
import { tokenColor, tokenPx } from "@/lib/tokens";

/** A stored screenshot ready to show: its object URL and its size in image pixels. */
export type LoadedImage = { url: string; width: number; height: number };

/** Each screenshot is read once a session, and kept while the app runs (a few per mockup). */
const loaded = new Map<string, Promise<LoadedImage>>();

export function loadImage(hash: string): Promise<LoadedImage> {
  let image = loaded.get(hash);
  if (!image) {
    image = readStoredImage(hash).then((blob) => new Promise<LoadedImage>((resolve, reject) => {
      const url = URL.createObjectURL(blob);
      const probe = new Image();
      probe.addEventListener("load", () => resolve({ url, width: probe.naturalWidth, height: probe.naturalHeight }));
      probe.addEventListener("error", () => { URL.revokeObjectURL(url); reject(new Error("not an image")); });
      probe.src = url;
    }));
    image.catch(() => loaded.delete(hash));
    loaded.set(hash, image);
  }
  return image;
}

export type ImageState = { status: "loading" } | { status: "error"; retry: () => void } | ({ status: "ready" } & LoadedImage);

/** A stored screenshot, loading, failed (with a retry) or ready. */
export function useStoredImage(hash: string | null): ImageState {
  const [attempt, setAttempt] = useState(0);
  // Each try has its own key, so a retry shows loading again until it settles.
  const key = hash === null ? null : `${attempt}:${hash}`;
  const [state, setState] = useState<{ key: string | null; image: ImageState }>({ key: null, image: { status: "loading" } });
  useEffect(() => {
    if (key === null) return;
    let live = true;
    loadImage(key.slice(key.indexOf(":") + 1)).then(
      (image) => { if (live) setState({ key, image: { status: "ready", ...image } }); },
      () => { if (live) setState({ key, image: { status: "error", retry: () => setAttempt((value) => value + 1) } }); },
    );
    return () => { live = false; };
  }, [key]);
  return state.key === key ? state.image : { status: "loading" };
}

/**
 * A screenshot with its notes' numbered markers drawn on it, as the marked file the notes
 * are sent with: the markers keep their on-screen shape, sized to the image.
 */
export async function markedScreenshot(image: LoadedImage, notes: readonly MockupNote[], all: readonly MockupNote[], name: string): Promise<File> {
  const canvas = document.createElement("canvas");
  canvas.width = image.width;
  canvas.height = image.height;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("no 2D canvas to mark the screenshot on");
  const picture = new Image();
  picture.src = image.url;
  await picture.decode();
  context.drawImage(picture, 0, 0);
  const size = Math.max(tokenPx("--spacing-mockup-marker"), image.width * 0.03);
  const fill = tokenColor("--marker");
  const ring = tokenColor("--foreground");
  for (const note of notes) {
    const x = note.x * image.width - size / 2;
    const y = note.y * image.height - size / 2;
    context.beginPath();
    context.roundRect(x, y, size, size, [size / 2, size / 2, size / 2, size / 10]);
    context.fillStyle = fill;
    context.fill();
    context.lineWidth = Math.max(1, size / 30);
    context.strokeStyle = ring;
    context.stroke();
    context.fillStyle = ring;
    context.font = `bold ${Math.round(size * 0.37)}px -apple-system, system-ui, sans-serif`;
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.fillText(String(markerNumber(all, note)), x + size / 2, y + size / 2);
  }
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("couldn't encode the marked screenshot");
  return new File([blob], name, { type: "image/png" });
}
