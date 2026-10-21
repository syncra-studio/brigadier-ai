import { openLink as openWebLink } from "@/state/links";
import { toast } from "@/state/toasts";

/** Web links in `text`, without trailing punctuation. */
export function linksIn(text: string): string[] {
  return (text.match(/https?:\/\/[^\s<>()"'`]+/g) ?? []).map((url) =>
    url.replace(/[.,;:!?]+$/, ""),
  );
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

export function openLink(url: string): void {
  openWebLink(url, { agent: true }).catch((error: unknown) =>
    toast(error instanceof Error ? error.message : String(error), {
      tone: "error",
    }),
  );
}
