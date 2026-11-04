/**
 * The icons of the web sites answers link to, asked of the daemon (which fetches them from each
 * site and caches them) once per site per run: a link shows a globe until its icon comes, and
 * keeps it when the site has none.
 */
import { useEffect, useState } from "react";

import { request } from "@/ipc/client";

const icons = new Map<string, Promise<string | null>>();

/** The HTTPS origin of a link, or `null` for any other link. */
export function httpsOrigin(href: string): string | null {
  try {
    const url = new URL(href);
    return url.protocol === "https:" ? url.origin : null;
  } catch {
    return null;
  }
}

function iconOf(origin: string): Promise<string | null> {
  let found = icons.get(origin);
  if (!found) {
    found = request({ method: "siteIcon", origin }).then(
      (answer) => answer.icon ?? null,
      () => null,
    );
    icons.set(origin, found);
  }
  return found;
}

/** A near-black mark on clear (`rgba`, its pixels; GitHub's), which the dark thread would hide. */
export function darkMark(rgba: ArrayLike<number>): boolean {
  let seen = false;
  for (let at = 0; at < rgba.length; at += 4) {
    if (rgba[at + 3]! < 40) continue;
    if (Math.max(rgba[at]!, rgba[at + 1]!, rgba[at + 2]!) > 60) return false;
    seen = true;
  }
  return seen;
}

/** Whether a loaded icon is a {@link darkMark}; one the browser won't let be read is left as it is. */
export function imageIsDarkMark(image: HTMLImageElement): boolean {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 32;
  const context = canvas.getContext("2d");
  if (!context) return false;
  try {
    context.drawImage(image, 0, 0, 32, 32);
    return darkMark(context.getImageData(0, 0, 32, 32).data);
  } catch {
    return false;
  }
}

/** The site's icon as a `data:` URL, once known; `null` meanwhile and when it has none. */
export function useSiteIcon(href: string | undefined): string | null {
  const origin = href ? httpsOrigin(href) : null;
  const [icon, setIcon] = useState<{ origin: string; url: string | null } | null>(null);
  useEffect(() => {
    if (!origin) return;
    let live = true;
    void iconOf(origin).then((url) => live && setIcon({ origin, url }));
    return () => {
      live = false;
    };
  }, [origin]);
  return icon && icon.origin === origin ? icon.url : null;
}
