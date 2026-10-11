/** The part of `document` the mark reads and writes, so a test can pass its own. */
export interface VisibilityDocument {
  readonly hidden: boolean;
  readonly documentElement: { readonly dataset: DOMStringMap };
  addEventListener(type: "visibilitychange", listener: () => void): void;
}

/**
 * Marks the root `data-page-hidden` while nobody can see the page: the window is hidden or
 * minimized or, on macOS, other windows cover it all. WebKit runs no animation frames then, so a
 * dialog or menu closing would wait forever for its exit animation to end, and Radix would keep
 * it and its overlay on screen until the window came back. globals.css drops closing animations
 * under the mark, so they leave at once.
 */
export function trackPageHidden(doc: VisibilityDocument = document): void {
  const sync = () => {
    if (doc.hidden) doc.documentElement.dataset.pageHidden = "";
    else delete doc.documentElement.dataset.pageHidden;
  };
  sync();
  doc.addEventListener("visibilitychange", sync);
}
