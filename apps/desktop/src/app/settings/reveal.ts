import { create } from "zustand";

/** How many frames a reveal waits for its page (which may load lazily) to show the setting. */
const FIND_FRAMES = 60;

/**
 * The setting Settings is bringing into view (a search result, or a page merged into another),
 * by its row's label.
 */
export const useReveal = create<{ label: string | null }>(() => ({ label: null }));

/** A setting's row, else a section named like it (the Usage page's sections). */
function findSetting(label: string): HTMLElement | undefined {
  const rows = document.querySelectorAll<HTMLElement>("[data-setting]");
  const row = [...rows].find((element) => element.dataset.setting === label);
  if (row) return row;
  return [...document.querySelectorAll<HTMLElement>("section")].find((section) => {
    const labelledBy = section.getAttribute("aria-labelledby");
    const name =
      section.getAttribute("aria-label") ??
      (labelledBy ? document.getElementById(labelledBy)?.textContent : null);
    return name?.trim() === label;
  });
}

/** Brings the setting labelled `label` into view once its page shows it. */
export function revealSetting(label: string): void {
  useReveal.setState({ label });
  let frames = 0;
  const reveal = () => {
    const target = findSetting(label);
    if (target) {
      target.scrollIntoView({ block: "center" });
      useReveal.setState({ label: null });
    } else if (++frames < FIND_FRAMES) requestAnimationFrame(reveal);
    else useReveal.setState({ label: null });
  };
  requestAnimationFrame(reveal);
}
