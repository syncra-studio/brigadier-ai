// DevTools' panes: a <webview> per tab for its front end, in a layer of their own beside the pages, so the tab layer's
// children stay exactly the pages. Like a page, a pane shows through a hole the app cuts (src/app/side-panel/BrowserDevTools.tsx).
import type { BrowserBounds } from "@/ipc/generated";

import { agentPage, type Webview } from "./tabLayer";

const panes = new Map<string, Webview>();
let layer: HTMLDivElement | null = null;

function ensureLayer(): HTMLDivElement {
  if (layer) return layer;
  const style = document.createElement("style");
  style.textContent = `
    #brigadier-devtools-layer { position: fixed; inset: 0; z-index: 0; overflow: hidden; contain: strict; pointer-events: none; }
    #brigadier-devtools-layer webview { position: absolute; display: inline-flex; border: 0; background: #fff; pointer-events: auto; }
    #brigadier-tab-layer[data-locked] ~ #brigadier-devtools-layer webview { pointer-events: none; }
  `;
  document.head.append(style);
  layer = document.createElement("div");
  layer.id = "brigadier-devtools-layer";
  document.getElementById("brigadier-tab-layer")?.after(layer);
  return layer;
}

export function placeDevtools(id: string, bounds: BrowserBounds | null): void {
  const pane = panes.get(id);
  if (!pane) return;
  pane.style.visibility = bounds ? "visible" : "hidden";
  if (!bounds) return;
  pane.style.left = `${bounds.x}px`;
  pane.style.top = `${bounds.y}px`;
  pane.style.width = `${bounds.width}px`;
  pane.style.height = `${bounds.height}px`;
}

export async function openDevtools(id: string, bounds: BrowserBounds): Promise<void> {
  if (panes.get(id)?.isConnected) return placeDevtools(id, bounds);
  const { guest } = agentPage(id);
  const pane = document.createElement("webview") as Webview;
  pane.setAttribute("partition", "brigadier-devtools");
  pane.dataset.devtoolsFor = id;
  panes.set(id, pane);
  placeDevtools(id, bounds);
  await new Promise<void>((resolve) => {
    pane.addEventListener("dom-ready", () => resolve(), { once: true });
    pane.setAttribute("src", "about:blank");
    ensureLayer().append(pane);
  });
  await window.brigadierShell.devtools("open", { guest, front: pane.getWebContentsId() });
}

/** The pane goes with DevTools: a front end can't be used twice. */
export function dropDevtools(id: string): void {
  panes.get(id)?.remove();
  panes.delete(id);
}

export async function closeDevtools(id: string): Promise<void> {
  const { guest } = agentPage(id);
  await window.brigadierShell.devtools("close", { guest });
  dropDevtools(id);
}
