/** Standalone image-preview measurements; native=1 keeps real IPC for the dev-app check. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { saveCopy } from "@/ipc/client";
import { useToasts } from "@/state/toasts";

const query = new URLSearchParams(location.search);
const native = query.has("native");
const canvas = document.createElement("canvas");
canvas.width = 800;
canvas.height = 500;
const context = canvas.getContext("2d")!;
context.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--foreground");
context.fillRect(0, 0, 800, 500);
context.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--background");
context.fillRect(100, 100, 300, 200);
const url = canvas.toDataURL("image/png");
const sourceBytes = Uint8Array.from(atob(url.split(",")[1]!), (character) => character.charCodeAt(0));
const source = { file: new File([sourceBytes], "preview.png", { type: "image/png" }) };
const images = [{ name: "preview", url, source }, { name: "large.png", url: (() => { canvas.width = 2000; canvas.height = 1400; return canvas.toDataURL("image/jpeg"); })() }];
const calls: unknown[] = [];
if (!native) mockIPC((command, payload) => { if (command === "save_copy") calls.push(payload); return null; });
const wait = (ms = 80) => new Promise((resolve) => setTimeout(resolve, ms));
const one = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const button = (label: string) => one<HTMLButtonElement>(`[aria-label="${label}"]`);
const menuItem = () => Array.from(document.querySelectorAll<HTMLElement>("[role=menuitem]")).find((item) => item.textContent === "Copy image")!;
const scale = () => one("[data-slot=image-preview-scale]").textContent;
const key = async (value: string, extra: KeyboardEventInit = {}) => { one("[data-slot=image-preview]").dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true, ...extra })); await wait(); };
const report = (value: unknown) => { const pre = document.createElement("pre"); pre.id = "lightbox-result"; pre.textContent = JSON.stringify(value); document.body.append(pre); };

async function nativeCopies() {
  const bytes = sourceBytes.buffer;
  const first = await saveCopy({ name: "20261015-0226-rebuild-wow-w33lb-native.png", bytes: new Uint8Array(bytes), ask: false });
  const second = await saveCopy({ name: "20261015-0226-rebuild-wow-w33lb-native.png", path: first!, ask: false });
  const sha256 = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), (byte) => byte.toString(16).padStart(2, "0")).join("");
  const result = { first, second, sha256 };
  report(result);
  if (query.has("auto")) await saveCopy({ name: "20261015-0226-rebuild-wow-w33lb-result.json", bytes: new TextEncoder().encode(JSON.stringify(result)), ask: false });
}

const opening = query.get("drive") === "open";
createRoot(one("#root")).render(<main className="p-10">
  {/* `drive=open&source=1`: a composer attachment, its set's entry known only by its file. */}
  <ImagePreview name="preview" url={url} gallery={{ images: opening && query.has("source") ? [{ name: "preview", source }, images[1]!] : images, index: 0 }}><button type="button" data-slot="open-preview">{opening ? <img src={url} alt="preview" className="h-24 w-40 object-cover" /> : "Open image preview"}</button></ImagePreview>
  {native && <>
    <button type="button" onClick={() => { void nativeCopies(); }}>Save native copies</button>
    <a href={url} download="20261015-0226-rebuild-wow-w33lb-source.png">Source PNG</a>
  </>}
</main>);

async function measure() {
  await wait(100);
  one<HTMLButtonElement>("[data-slot=open-preview]").click();
  await wait(500);
  const view = one("[data-slot=image-preview]");
  const bounds = view.getBoundingClientRect();
  const geometry = (label: string) => {
    const node = button(label); const box = node.getBoundingClientRect(); const style = getComputedStyle(node);
    return { width: parseFloat(style.width), height: parseFloat(style.height), radius: style.borderRadius, top: box.top - bounds.top, right: bounds.right - box.right, left: box.left - bounds.left };
  };
  const out: Record<string, unknown> = {
    aria: view.getAttribute("aria-labelledby") ? document.getElementById(view.getAttribute("aria-labelledby")!)?.textContent : null,
    labels: Array.from(view.querySelectorAll("button")).map((node) => node.getAttribute("aria-label")),
    download: geometry("Download image"), close: geometry("Close image preview"), previous: geometry("Previous image"), next: geometry("Next image"), minus: geometry("Zoom out image"), plus: geometry("Zoom in image"),
    counterBelow: one("[data-slot=image-preview-count]").getBoundingClientRect().top - one("[data-slot=image-preview-zoom]").getBoundingClientRect().bottom,
    initial: scale(),
  };
  const input = document.createElement("input");
  view.append(input); input.focus();
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "+", bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await wait();
  out.typing = { open: !!document.querySelector("[data-slot=image-preview]"), scale: scale(), count: one("[data-slot=image-preview-count]").textContent };
  input.remove();
  button("Close image preview").focus();
  await key("ArrowLeft");
  out.first = { count: one("[data-slot=image-preview-count]").textContent, disabled: button("Previous image").disabled };
  await key("ArrowRight", { metaKey: true });
  out.modifiedArrow = one("[data-slot=image-preview-count]").textContent;
  await key("+", { shiftKey: true }); const first = scale(); await key("+"); out.ramp = [first, scale()];
  for (let i = 0; i < 20; i++) await key("-");
  await key("-"); out.minimum = scale();
  await key("0");
  await key("+"); await key("+"); await key("+"); await key("+"); await key("+");
  const frame = one("[data-slot=image-preview-frame]");
  const picture = one<HTMLImageElement>("[data-slot=image-preview] img");
  const box = frame.getBoundingClientRect();
  const pointer = { x: box.left + box.width * 0.57, y: box.top + box.height * 0.53 };
  const before = picture.getBoundingClientRect();
  const sourcePoint = { x: (pointer.x - before.left) / before.width, y: (pointer.y - before.top) / before.height };
  frame.dispatchEvent(new WheelEvent("wheel", { bubbles: true, cancelable: true, metaKey: true, deltaY: -40, clientX: pointer.x, clientY: pointer.y }));
  await wait();
  const after = picture.getBoundingClientRect();
  out.anchorError = { x: Math.abs(after.left + sourcePoint.x * after.width - pointer.x), y: Math.abs(after.top + sourcePoint.y * after.height - pointer.y) };
  // Synthetic pointer events exercise the same handlers while capture uses a real active mouse.
  const capture = frame.setPointerCapture.bind(frame);
  const release = frame.releasePointerCapture.bind(frame);
  frame.setPointerCapture = () => {};
  frame.releasePointerCapture = () => {};
  const oldScroll = { x: frame.scrollLeft, y: frame.scrollTop };
  frame.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerId: 7, button: 0, clientX: pointer.x, clientY: pointer.y }));
  frame.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, pointerId: 7, clientX: pointer.x - 45, clientY: pointer.y - 32 }));
  frame.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, pointerId: 7, clientX: pointer.x - 45, clientY: pointer.y - 32 }));
  await wait();
  out.pan = { x: frame.scrollLeft - oldScroll.x, y: frame.scrollTop - oldScroll.y };
  frame.setPointerCapture = capture; frame.releasePointerCapture = release;
  button("Download image").click();
  await wait(150);
  out.saved = calls[0];
  out.expectedBytes = Array.from(new Uint8Array(await (await fetch(url)).arrayBuffer()));
  out.toastsAfterCancel = useToasts.getState().toasts.map((entry) => entry.text);
  await key("ArrowRight");
  const fitted = scale();
  button("Zoom in image").click(); await wait(); const above = scale();
  button("Zoom out image").click(); await wait();
  out.inserted = { fit: fitted, above, below: scale() };
  let copied: { type: string; signature: number[] } | null = null;
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { write: async (items: ClipboardItem[]) => {
    const png = await items[0]!.getType("image/png");
    copied = { type: png.type, signature: Array.from(new Uint8Array(await png.arrayBuffer()).slice(0, 8)) };
  } } });
  one("[data-slot=image-preview] img").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 400, clientY: 300, button: 2 }));
  await wait(); menuItem().click(); await wait(200);
  out.copied = copied;
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { write: () => Promise.reject(new Error("Clipboard unavailable")) } });
  one("[data-slot=image-preview] img").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 400, clientY: 300, button: 2 }));
  await wait(); menuItem().click(); await wait(150);
  out.copyFailure = useToasts.getState().toasts.map((entry) => entry.text);
  button("Close image preview").click(); await wait(250);
  out.closed = !document.querySelector("[data-slot=image-preview]");
  report(out);
}
if (native) {
  useToasts.subscribe((state) => {
    let pre = document.getElementById("native-toasts");
    if (!pre) { pre = document.createElement("pre"); pre.id = "native-toasts"; document.body.append(pre); }
    pre.textContent = JSON.stringify(state.toasts.map((entry) => entry.text));
  });
  window.addEventListener("unhandledrejection", (event) => {
    const error = { error: String(event.reason) };
    report(error);
    if (query.has("auto")) void saveCopy({ name: "20261015-0226-rebuild-wow-w33lb-error.json", bytes: new TextEncoder().encode(JSON.stringify(error)), ask: false });
  });
}
/**
 * `drive=open` (with motion): the viewer grown out of a thumbnail, sampled on every frame until it
 * settles: once it shows whole it never fades again, and its image keeps one size.
 */
async function measureOpen() {
  await wait(300);
  const samples: { opacity: number; overlay: number; width: number; height: number }[] = [];
  let sampling = true;
  const sample = () => {
    const view = document.querySelector<HTMLElement>("[data-slot=image-preview]");
    const overlay = view?.previousElementSibling as HTMLElement | null;
    const picture = view?.querySelector("img");
    if (view) {
      const box = picture?.getBoundingClientRect();
      samples.push({
        opacity: Number(getComputedStyle(view).opacity),
        overlay: overlay ? Number(getComputedStyle(overlay).opacity) : 1,
        width: Math.round(box?.width ?? 0),
        height: Math.round(box?.height ?? 0),
      });
    }
    if (sampling) requestAnimationFrame(sample);
  };
  requestAnimationFrame(sample);
  one<HTMLButtonElement>("[data-slot=open-preview]").click();
  await wait(1200);
  sampling = false;
  const whole = samples.findIndex((entry) => entry.opacity >= 0.999 && entry.overlay >= 0.999 && entry.width > 0);
  const after = whole < 0 ? [] : samples.slice(whole);
  report({
    frames: samples.length,
    shownWhole: whole >= 0,
    // A dip after it showed whole is the flash.
    dips: after.filter((entry) => entry.opacity < 0.999 || entry.overlay < 0.999).length,
    // Frames of the open viewer with no picture in it.
    empty: samples.filter((entry) => entry.width === 0).length,
    sizes: [...new Set(after.filter((entry) => entry.width > 0).map((entry) => `${entry.width}x${entry.height}`))],
  });
}

if (!native && opening) void measureOpen().catch((error: unknown) => report({ error: String(error), stack: error instanceof Error ? error.stack : null }));
else if (!native && !query.has("stop")) void measure().catch((error: unknown) => report({ error: String(error), stack: error instanceof Error ? error.stack : null }));

if (native && query.has("auto")) void nativeCopies();
