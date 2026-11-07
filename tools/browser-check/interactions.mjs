#!/usr/bin/env node
// Section 1's interactions, run in the real front end through its own DOM and state. Every input here is
// scripted (sendInputEvent into the page or the app), which proves the app and the guest respond, not
// that a real pointer reaches them: go.mjs is for that. Pixels are the OS's.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { dataDir, evidenceDir } from "./paths.mjs";
import { capture, framesOf, record } from "./instruments.mjs";
import { call } from "./bc.mjs";
import { FIXTURES, control, guestExec, host, layerState, openShell, selectSession, sleep } from "./lib.mjs";
import { distinctColors, near } from "./png.mjs";

const folder = path.join(evidenceDir, "interactions");
fs.mkdirSync(folder, { recursive: true });
const name = "ui";
const session = `session-${name}`;
const results = [];
const check = (id, passed, proof, detail = {}) => {
  results.push({ id, passed, proof, ...detail });
  console.log(`${passed ? "ok  " : "FAIL"} [${proof}] ${id} ${JSON.stringify(detail).slice(0, 220)}`);
};
const SCRIPTED = "scripted";
const OS = "scripted input, OS pixels";

const { windowId } = await openShell(name);
await selectSession(name, session);
await sleep(600);

const send = (event) => control("hostSendInput", { name, event });
const rectOf = (selector) => host(name, `(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)return null;const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}})()`);
const clickAt = async (x, y) => {
  const at = { x: Math.round(x), y: Math.round(y) };
  await send({ type: "mouseMove", ...at });
  await sleep(40);
  await send({ type: "mouseDown", ...at, button: "left", clickCount: 1 });
  await sleep(40);
  await send({ type: "mouseUp", ...at, button: "left", clickCount: 1 });
};
const click = async (selector) => {
  const rect = await rectOf(selector);
  if (!rect) throw new Error(`nothing matches ${selector}`);
  await clickAt(rect.x + rect.width / 2, rect.y + rect.height / 2);
};
const press = async (keyCode, modifiers = []) => {
  await send({ type: "keyDown", keyCode, modifiers });
  if (keyCode === "Enter") await send({ type: "char", keyCode: "\r", modifiers });
  await send({ type: "keyUp", keyCode, modifiers });
};
const activeTab = () => host(name, `(async()=>{const s=await import("/src/state/sessionTabs.ts");const t=s.sessionTabs(${JSON.stringify(session)});return {active:t.active,count:t.tabs.length,kinds:t.tabs.map(x=>x.kind)}})()`);
const shot = (label) => capture(windowId, label, folder);
const pageAt = (image, x = 700, y = 420) => image.at(x * 2, y * 2);
const guestOf = async (id) => (await layerState(name)).pages.find((page) => page.id === id);
const waitFor = async (what, test, ms = 8000) => {
  const start = Date.now();
  for (;;) {
    const value = await test();
    if (value) return value;
    if (Date.now() - start > ms) throw new Error(`timed out: ${what}`);
    await sleep(100);
  }
};
const typeAddress = async (url) => {
  await click('[data-pane="browser"][data-active] input[aria-label="Address"]');
  // The app selects the whole address a frame after focus: wait for it, as a person's typing would.
  await sleep(350);
  await host(name, `document.activeElement?.select?.();1`);
  await control("hostInsertText", { name, text: url });
  await sleep(100);
  await press("Enter");
};

// New Tab by shortcut, then the address bar: src/app/shell/SessionTabBar.tsx:490's New Tab becomes a page.
const before0 = await activeTab();
await press("t", ["meta"]);
await sleep(500);
const afterT = await activeTab();
check("cmd-T opens a New Tab", afterT.count === before0.count + 1 && afterT.kinds.includes("newTab"), SCRIPTED, afterT);
const newTabShot = await shot("01-new-tab");
check("New Tab shows the app's own layout, no page yet", (await layerState(name)).counters.created === 0, OS, { file: newTabShot.file });
await typeAddress(`${FIXTURES}/page/116611/first`);
const firstId = afterT.active;
const firstPage = await waitFor("first page", async () => { const page = await guestOf(firstId); return page?.wc ? page : null; });
await sleep(800);
const firstShot = await shot("02-first-page");
const green = pageAt(firstShot.image);
check("address bar opens the page in the same tab, in one fixed layer", firstPage.partition === "persist:browser-profile-default" && green[1] > green[0] && green[1] > green[2], OS, { green, partition: firstPage.partition, file: firstShot.file });

// Shortcut for a browser tab; a second page and a second profile's partition.
await host(name, `window.__brigadierTabLayer.setProfileForNewTabs("work");1`);
await press("b", ["meta", "shift"]);
await sleep(500);
const afterB = await activeTab();
check("cmd-shift-B opens another tab", afterB.count === afterT.count + 1, SCRIPTED, afterB);
await sleep(200);
await typeAddress(`${FIXTURES}/page/661111/second`);
const secondId = afterB.active;
const secondPage = await waitFor("second page", async () => { const page = await guestOf(secondId); return page?.wc ? page : null; });
await host(name, `window.__brigadierTabLayer.setProfileForNewTabs("default");1`);
await sleep(800);
check("a tab carries its profile: another partition", secondPage.partition === "persist:browser-profile-work", SCRIPTED, { partition: secondPage.partition });
await guestExec(firstPage.wc, `document.cookie="scope=first;path=/;max-age=999";1`);
const seenInSecond = await guestExec(secondPage.wc, "document.cookie");
check("profiles don't share cookies", !seenInSecond.includes("scope=first"), SCRIPTED, { seenInSecond });

// Switching by clicking the real tab strip.
const tabRect = await host(name, `(()=>{const e=[...document.querySelectorAll('[data-slot="session-tabs"] [role=tab]')].find(x=>x.textContent.includes("first"));if(!e)return null;const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}})()`);
await clickAt(tabRect.x + 40, tabRect.y + tabRect.height / 2);
await sleep(400);
const switched = await shot("03-switched");
const switchedPixel = pageAt(switched.image);
const stateA = await layerState(name);
check("click on a tab shows its page; the other is hidden, not moved", switchedPixel[1] > switchedPixel[0] && stateA.pages.find((p) => p.id === secondId).visibility === "hidden" && stateA.counters.removedFromLayer === 0, OS, { switchedPixel, file: switched.file });

// Back, forward, reload with the toolbar.
const toolbar = (label) => `[data-pane="browser"][data-active] [data-slot="browser-navigation"] button:nth-of-type(${label})`;
const urlOf = () => guestExec(firstPage.wc, "location.href");
await typeAddress(`${FIXTURES}/page/114466/first-b`);
await waitFor("second address", async () => (await urlOf()).includes("first-b"));
await sleep(300);
await click(toolbar(1));
await waitFor("back", async () => !(await urlOf()).includes("first-b"));
check("back button", true, SCRIPTED, { url: await urlOf() });
await sleep(200);
await click(toolbar(2));
await waitFor("forward", async () => (await urlOf()).includes("first-b"));
check("forward button", true, SCRIPTED, { url: await urlOf() });
await sleep(300);
const originBefore = await guestExec(firstPage.wc, "performance.timeOrigin");
await click(toolbar(3));
await waitFor("reload", async () => (await guestExec(firstPage.wc, "performance.timeOrigin")) !== originBefore);
check("reload button loads the document again", true, SCRIPTED);
await sleep(500);

// Typing, hover, cursor, scrolling inside the page.
const guestSend = (event) => control("guestSendInput", { id: firstPage.wc, event });
const field = await guestExec(firstPage.wc, `(()=>{const r=document.getElementById("field").getBoundingClientRect();return {x:r.x+20,y:r.y+10}})()`);
await guestSend({ type: "mouseMove", x: Math.round(field.x), y: Math.round(field.y) });
await sleep(60);
await guestSend({ type: "mouseDown", x: Math.round(field.x), y: Math.round(field.y), button: "left", clickCount: 1 });
await guestSend({ type: "mouseUp", x: Math.round(field.x), y: Math.round(field.y), button: "left", clickCount: 1 });
await sleep(120);
await control("guestInsertText", { id: firstPage.wc, text: "typed in page" });
await sleep(150);
const typed = await guestExec(firstPage.wc, `document.getElementById("field").value`);
check("typing into the page's field", typed === "typed in page", SCRIPTED, { typed });
const typedShot = await shot("04-typed");
check("what was typed is on screen", distinctColors(typedShot.image, { x: 600, y: 300, width: 700, height: 80 }, 3) > 3, OS, { file: typedShot.file });
const link = await guestExec(firstPage.wc, `(()=>{const r=document.getElementById("link").getBoundingClientRect();return {x:r.x+30,y:r.y+15}})()`);
await guestSend({ type: "mouseMove", x: Math.round(link.x), y: Math.round(link.y) });
await sleep(150);
await guestSend({ type: "mouseMove", x: Math.round(field.x), y: Math.round(field.y) });
await sleep(150);
const cursors = await control("guestCursors", { id: firstPage.wc });
check("hover reaches the page: it asks for hand and text cursors", cursors.includes("hand") && cursors.includes("text"), SCRIPTED, { cursors, limit: "the guest asked for them; whether macOS shows them needs go.mjs" });
const wheelBefore = await shot("05-before-wheel");
await guestSend({ type: "mouseWheel", x: 400, y: 300, deltaX: 0, deltaY: -500, canScroll: true });
await sleep(400);
const scrolled = await guestExec(firstPage.wc, "scrollY");
const wheelAfter = await shot("06-after-wheel");
check("scrolling the page moves it, on screen too", scrolled > 0 && distinctColors(wheelBefore.image, { x: 600, y: 200, width: 400, height: 150 }, 4) !== distinctColors(wheelAfter.image, { x: 600, y: 200, width: 400, height: 150 }, 4), OS, { scrolled });
await guestExec(firstPage.wc, "scrollTo(0,0);1");
await sleep(200);

// An HTML menu over the page: the real Browser options menu, drawn over it, taking clicks.
await guestExec(firstPage.wc, "__fixture.clicks=0;1");
// The ⋯ button: the toolbar's last one.
const clickOptions = async () => {
  const rect = await host(name, `(()=>{const all=document.querySelectorAll('[data-pane="browser"][data-active] [data-slot="browser-toolbar"] button');const r=all[all.length-1].getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}})()`);
  await clickAt(rect.x + rect.width / 2, rect.y + rect.height / 2);
};
const menuVideo = path.join(folder, "menu-over-page.mp4");
const menuRecording = record(windowId, 3, menuVideo);
await sleep(500);
await clickOptions();
const menu = await waitFor("the menu", async () => rectOfMenu());
async function rectOfMenu() { return rectOf("[role=menu]"); }
await sleep(250);
const menuShot = await shot("07-menu-over-page");
const insideMenu = menuShot.image.at(Math.round((menu.x + menu.width / 2) * 2), Math.round((menu.y + menu.height / 2) * 2));
const besidePage = pageAt(menuShot.image, Math.round(menu.x - 60), Math.round(menu.y + 300));
const stateNow = (await layerState(name)).pages.find((page) => page.id === firstId);
check("the menu draws over the live page", !near(insideMenu, besidePage, 30) && menu.y + menu.height > stateNow.rect.y + 40, OS, { insideMenu, besidePage, menu, file: menuShot.file });
await clickAt(menu.x - 200, menu.y + 300);
await sleep(300);
const closed = (await rectOfMenu()) === null;
const clicksInPage = await guestExec(firstPage.wc, "__fixture.clicks");
check("a click beside the menu closes it and doesn't reach the page", closed && clicksInPage === 0, SCRIPTED, { closed, clicksInPage });
await menuRecording;
// One row through the menu's middle: no frame while it's up or going may be white or empty in the page.
const menuFrames = await framesOf(menuVideo, { x: 1400, y: 500, width: 1000, height: 2 });
const whiteFrames = menuFrames.filter((frame) => { let white = 0; for (let x = 0; x < 1000; x += 7) if (frame[x * 3] > 240 && frame[x * 3 + 1] > 240 && frame[x * 3 + 2] > 240) white++; return white > 100; }).length;
check("recorded menu over the page: no flash of white in any frame", whiteFrames === 0 && menuFrames.length > 60, OS, { frames: menuFrames.length, whiteFrames, video: menuVideo });
await clickOptions();
await waitFor("the menu again", rectOfMenu);
await sleep(200);
const item = await host(name, `(()=>{const e=[...document.querySelectorAll('[role=menuitem]')].find(x=>/zoom in|find/i.test(x.textContent));if(!e)return null;const r=e.getBoundingClientRect();return {text:e.textContent,x:r.x+r.width/2,y:r.y+r.height/2}})()`);
if (item) {
  await clickAt(item.x, item.y);
  await sleep(300);
}
check("a menu item over the page takes the click", item !== null && (await rectOfMenu()) === null, SCRIPTED, { item: item?.text });
await press("Escape");

// The divider, recorded: the page must keep up, and no pointer event may fall into it.
await guestExec(firstPage.wc, "__fixture.pointer=0;__fixture.clicks=0;1");
const handle = await rectOf('[data-slot="sidebar-resize-handle"]');
const row = 500;
const video = path.join(folder, "divider-drag.mp4");
const recording = record(windowId, 3, video);
await sleep(700);
const startX = Math.round(handle.x + handle.width / 2);
const y = Math.round(handle.y + 300);
await send({ type: "mouseMove", x: startX, y });
await send({ type: "mouseDown", x: startX, y, button: "left", clickCount: 1 });
for (let step = 1; step <= 40; step++) {
  await send({ type: "mouseMove", x: startX + step * 4, y, modifiers: ["leftButtonDown"] });
  await sleep(16);
}
const during = await layerState(name);
for (let step = 40; step >= 0; step--) {
  await send({ type: "mouseMove", x: startX + step * 4, y, modifiers: ["leftButtonDown"] });
  await sleep(8);
}
await send({ type: "mouseUp", x: startX, y, button: "left", clickCount: 1 });
await recording;
const pointerSeen = await guestExec(firstPage.wc, "__fixture.pointer");
const movedPage = during.pages.find((page) => page.id === firstId);
check("during a divider drag the page gets no pointer events", pointerSeen === 0, SCRIPTED, { pointerSeen });
check("the page follows the divider", movedPage.rect.x > firstPage.rect.x + 40, SCRIPTED, { from: firstPage.rect.x, to: movedPage.rect.x });
// In each frame, one row of pixels: sidebar colours, then page colours, nothing else between them.
const frames = await framesOf(video, { x: 0, y: row * 2, width: 2560, height: 2 });
let gapFrames = 0;
const edges = [];
const pageColor = pageAt((await shot("08-after-divider")).image);
for (const frame of frames) {
  let firstPageX = -1;
  for (let x = 0; x < 2560; x++) {
    if (near([frame[x * 3], frame[x * 3 + 1], frame[x * 3 + 2]], pageColor, 30)) { firstPageX = x; break; }
  }
  if (firstPageX < 0) { gapFrames++; continue; }
  edges.push(firstPageX);
  const before = [frame[(firstPageX - 8) * 3], frame[(firstPageX - 8) * 3 + 1], frame[(firstPageX - 8) * 3 + 2]];
  if (before[0] > 230 && before[1] > 230 && before[2] > 230) gapFrames++;
}
check("recorded divider drag: the page edge moves with it, every frame has the page, no white gap", gapFrames === 0 && frames.length > 60 && Math.max(...edges) - Math.min(...edges) > 100, OS, { frames: frames.length, gapFrames, edgeRange: [Math.min(...edges), Math.max(...edges)], video });

// The page fills its slot, by OS pixels, whatever the window and panels do: every corner marker at the
// slot's own corners, page colour to the edge, nothing of the page outside it.
await press("t", ["meta"]);
await sleep(400);
const fillTabId = (await activeTab()).active;
await typeAddress(`${FIXTURES}/fill/336699`);
const fillPage = await waitFor("fill page", async () => { const page = await guestOf(fillTabId); return page?.wc ? page : null; });
await sleep(800);
const slotRect = () => rectOf('[data-pane="browser"][data-active] [data-slot="browser-page"]');
const INSET = 8;
const cornerPoints = (slot) => ({
  topLeft: [slot.x + INSET, slot.y + INSET],
  topRight: [slot.x + slot.width - INSET, slot.y + INSET],
  bottomLeft: [slot.x + INSET, slot.y + slot.height - INSET],
  bottomRight: [slot.x + slot.width - INSET, slot.y + slot.height - INSET],
});
let cornerColors = null;
const fillOk = async (label) => {
  await sleep(350);
  const slot = await slotRect();
  const { image, file } = await shot(`fill-${label}`);
  const read = ([x, y]) => image.at(Math.round(x * 2), Math.round(y * 2));
  const corners = Object.fromEntries(Object.entries(cornerPoints(slot)).map(([key, point]) => [key, read(point)]));
  if (!cornerColors) {
    cornerColors = corners;
    const [r, g, y, c] = [corners.topLeft, corners.topRight, corners.bottomLeft, corners.bottomRight];
    if (!(r[0] > 180 && r[1] < 90 && g[1] > 180 && g[0] < 130 && y[0] > 180 && y[1] > 180 && y[2] < 120 && c[1] > 180 && c[2] > 180 && c[0] < 130)) throw new Error(`fill markers aren't where they should be: ${JSON.stringify(corners)}`);
  }
  const matches = Object.keys(corners).every((key) => near(corners[key], cornerColors[key], 30));
  const pageColor = read([slot.x + slot.width / 2, slot.y + slot.height / 2]);
  // Just inside each edge between the markers is the page; just outside it isn't.
  const edgesIn = [[slot.x + slot.width / 2, slot.y + 2], [slot.x + 2, slot.y + slot.height / 2], [slot.x + slot.width - 2, slot.y + slot.height / 2], [slot.x + slot.width / 2, slot.y + slot.height - 2]].every((point) => near(read(point), pageColor, 30));
  const leftOut = !near(read([slot.x - 3, slot.y + slot.height / 2]), pageColor, 30);
  const guest = await guestExec(fillPage.wc, "({w:innerWidth,h:innerHeight})");
  const sized = Math.abs(guest.w - slot.width) <= 1 && Math.abs(guest.h - slot.height) <= 1;
  check(`the page fills its slot: ${label}`, matches && edgesIn && leftOut && sized, OS, { slot: [slot.x, slot.y, slot.width, slot.height], guest, matches, edgesIn, leftOut, file });
  return slot;
};
const fillStart = await fillOk("at rest");
const parked = (await control("windowInfo", { name })).bounds;
await control("setBounds", { name, bounds: { ...parked, width: 1000, height: 620 } });
const smaller = await fillOk("window made smaller");
await control("setBounds", { name, bounds: { ...parked, width: 1400, height: 900 } });
const larger = await fillOk("window made larger");
await control("setBounds", { name, bounds: parked });
await fillOk("window back");
check("the window's size changes really moved the slot", smaller.width < fillStart.width && larger.height > fillStart.height, SCRIPTED, { smaller: smaller.width, larger: larger.height });
await press("b", ["meta"]);
await sleep(450);
const noSidebar = await fillOk("sidebar collapsed");
await press("b", ["meta"]);
await sleep(450);
await fillOk("sidebar back");
await press("b", ["meta", "alt"]);
await sleep(450);
const withRight = await fillOk("right panel open");
await press("b", ["meta", "alt"]);
await sleep(450);
check("the panels really moved the slot", noSidebar.x < fillStart.x && withRight.width !== fillStart.width, SCRIPTED, { noSidebarX: noSidebar.x, rightPanelWidth: withRight.width, restWidth: fillStart.width });
// The divider, frame by frame: the markers stay at the slot's corners all through the drag.
const fillVideo = path.join(folder, "divider-fill.mp4");
const fillHandle = await rectOf('[data-slot="sidebar-resize-handle"]');
const fillRecording = record(windowId, 3, fillVideo);
await sleep(700);
const fx = Math.round(fillHandle.x + fillHandle.width / 2);
const fy = Math.round(fillHandle.y + 300);
await send({ type: "mouseMove", x: fx, y: fy });
await send({ type: "mouseDown", x: fx, y: fy, button: "left", clickCount: 1 });
for (let step = 1; step <= 40; step++) { await send({ type: "mouseMove", x: fx + step * 4, y: fy, modifiers: ["leftButtonDown"] }); await sleep(16); }
for (let step = 40; step >= 0; step--) { await send({ type: "mouseMove", x: fx + step * 4, y: fy, modifiers: ["leftButtonDown"] }); await sleep(8); }
await send({ type: "mouseUp", x: fx, y: fy, button: "left", clickCount: 1 });
await fillRecording;
const restSlot = await slotRect();
const rowOf = (y) => framesOf(fillVideo, { x: 0, y: Math.round(y * 2), width: 2560, height: 2 });
const topRows = await rowOf(restSlot.y + INSET);
const bottomRows = await rowOf(restSlot.y + restSlot.height - INSET);
const firstAt = (frame, color) => { for (let x = 0; x < 2560; x += 2) if (near([frame[x * 3], frame[x * 3 + 1], frame[x * 3 + 2]], color, 40)) return x; return -1; };
const lastAt = (frame, color) => { for (let x = 2558; x >= 0; x -= 2) if (near([frame[x * 3], frame[x * 3 + 1], frame[x * 3 + 2]], color, 40)) return x; return -1; };
// Two different facts. The slot is filled: up to the page's right edge there's page or marker colour, top
// and bottom. The content may trail: a right-anchored marker can sit short of the edge until the guest
// lays out at its new width, and how far and how often is reported, with a limit.
const pageColorAtRest = (await shot("fill-reference")).image.at(Math.round((restSlot.x + restSlot.width / 2) * 2), Math.round((restSlot.y + restSlot.height / 2) * 2));
const pixelAt = (frame, x) => [frame[x * 3], frame[x * 3 + 1], frame[x * 3 + 2]];
let gapFrames2 = 0;
let lagFrames = 0;
let worstLag = 0;
const lefts = [];
const rightEdge = Math.round((restSlot.x + restSlot.width) * 2);
topRows.forEach((frame, index) => {
  const left = firstAt(frame, cornerColors.topLeft);
  const bottomLeft = firstAt(bottomRows[index], cornerColors.bottomLeft);
  const right = lastAt(frame, cornerColors.topRight);
  const bottomRight = lastAt(bottomRows[index], cornerColors.bottomRight);
  const isPage = (pixel, marker) => near(pixel, pageColorAtRest, 40) || near(pixel, marker, 40);
  const edgeX = rightEdge - 6;
  const filled = left >= 0 && bottomLeft >= 0 && Math.abs(left - bottomLeft) <= 4 && isPage(pixelAt(frame, edgeX), cornerColors.topRight) && isPage(pixelAt(bottomRows[index], edgeX), cornerColors.bottomRight);
  if (!filled) gapFrames2++;
  else lefts.push(left);
  const lag = Math.max(rightEdge - 2 - right, rightEdge - 2 - bottomRight);
  if (lag > 2) lagFrames++;
  worstLag = Math.max(worstLag, lag);
});
check("the page fills its slot in every frame of a divider drag", gapFrames2 === 0 && topRows.length > 60 && Math.max(...lefts) - Math.min(...lefts) > 100, OS, { frames: topRows.length, gapFrames: gapFrames2, leftEdgeRange: [Math.min(...lefts), Math.max(...lefts)], video: fillVideo });
check("during that drag, right-anchored content trails the slot by at most 8 points", worstLag <= 16, OS, { framesTrailing: lagFrames, worstDevicePixels: worstLag, note: "a guest lays out at its new width a frame after its frame is resized" });
await fillOk("divider back");
const fillTab = await host(name, `(async()=>{const s=await import("/src/state/sessionTabs.ts");s.closeTab(${JSON.stringify(session)},${JSON.stringify(fillTabId)});s.selectTab(${JSON.stringify(session)},${JSON.stringify(firstId)});return true})()`);
await sleep(500);
void fillTab;

// Transitions that remount the app's subtree. Nothing may touch the page.
const identity = async () => ({
  wc: (await guestOf(firstId)).wc,
  timeOrigin: await guestExec(firstPage.wc, "performance.timeOrigin"),
  stamp: await guestExec(firstPage.wc, "window.__stamp ??= Math.random()"),
});
const stable = await identity();
const removedBefore = (await layerState(name)).counters.removedFromLayer;
await selectSession(name, `session-${name}-other`);
await sleep(500);
const inOther = await layerState(name);
const otherShot = await shot("09-other-session");
check("session switch hides the page, keeps it", inOther.pages.every((page) => page.visibility === "hidden"), OS, { file: otherShot.file });
await selectSession(name, session);
await sleep(600);
const backShot = await shot("10-back-to-session");
const backAgain = await identity();
check("back in the session: same guest, same document, no reload", JSON.stringify(backAgain) === JSON.stringify(stable) && (await layerState(name)).counters.removedFromLayer === removedBefore, OS, { backAgain, file: backShot.file });
await press("b", ["meta"]);
await sleep(450);
const collapsed = await shot("11-sidebar-collapsed");
const narrow = (await guestOf(firstId)).rect;
await press("b", ["meta"]);
await sleep(450);
const opened = await identity();
check("panel collapse moves the page, keeps it", JSON.stringify(opened) === JSON.stringify(stable) && narrow.x < movedPage.rect.x, OS, { x: narrow.x, file: collapsed.file });

// A page that can't load says so, rather than sit white as if the tab were blank.
await typeAddress("http://brigadier-check-no-such-host.invalid/");
const banner = await waitFor("the failure's banner", () => host(name, `(()=>{const pane=document.querySelector('[data-pane="browser"][data-active]');const m=/This can.t open here[^]{0,80}/.exec(pane?.textContent??"");return m?m[0]:null})()`));
const failedShot = await shot("12-failed-load");
check("a failed load says why", Boolean(banner), OS, { banner, file: failedShot.file });
await typeAddress(`${FIXTURES}/page/114466/first-b`);
await waitFor("back on a page", async () => (await urlOf()).includes("first-b"));
await sleep(400);

// Find and zoom, since they cost little.
await press("=", ["meta"]);
await sleep(300);
const zoomed = await host(name, `(async()=>(await import("/src/state/browsers.ts")).useBrowsers.getState().pages[${JSON.stringify(firstId)}].zoom)()`);
check("zoom in by shortcut", zoomed > 1, SCRIPTED, { zoomed });
await press("0", ["meta"]);
await sleep(200);
await press("f", ["meta"]);
await sleep(300);
const findBar = await host(name, `(()=>{const e=[...document.querySelectorAll("[data-pane=browser][data-active] input")].find(x=>x.getAttribute("aria-label")!=="Address");return e?e.getAttribute("aria-label")||e.placeholder||"input":null})()`);
check("find bar opens by shortcut", findBar !== null, SCRIPTED, { findBar });
await press("Escape");

// Sign-in cookie and open tabs across a clean quit and a crash. These relaunch the instance: leave them
// out with --no-restart when nothing they cover has changed.
const noRestart = process.argv.includes("--no-restart");
if (noRestart) {
  check("restart checks skipped by --no-restart: results stand from the last full run", true, "not run");
  fs.writeFileSync(path.join(folder, "results-no-restart.json"), JSON.stringify({ at: new Date().toISOString(), passed: results.every((r) => r.passed), results }, null, 1));
  console.log(results.every((r) => r.passed) ? "ALL PASSED" : "SOME FAILED");
  process.exit(0);
}
const tabsOf = () => host(name, `(async()=>{const s=await import("/src/state/sessionTabs.ts");return s.sessionTabs(${JSON.stringify(session)}).tabs.map(t=>({id:t.id,kind:t.kind,url:t.url}))})()`);
await typeAddress(`${FIXTURES}/sign-in`);
await waitFor("signed in", async () => (await urlOf()).includes("sign-in"));
await sleep(1800);
const cookies = await control("cookies", { partition: "persist:browser-profile-default" });
const sid = cookies.find((cookie) => cookie.name === "sid");
check("a session cookie gets an expiry as it's set", sid && sid.session === false && sid.httpOnly === true && sid.hostOnly === true, SCRIPTED, { sid });
const tabsBefore = await tabsOf();
const readDb = () => {
  const source = path.join(dataDir, "Partitions/browser-profile-default/Cookies");
  const copy = path.join(folder, "Cookies.copy");
  fs.copyFileSync(source, copy);
  const raw = fs.readFileSync(copy);
  const rows = execFileSync("sqlite3", [copy, "select name, hex(substr(encrypted_value,1,3)), length(value) from cookies where name in ('sid','keep')"]).toString().trim();
  return { rows, plaintextInFile: raw.includes(Buffer.from("session-value")) };
};
await control("flushCookies", { partition: "persist:browser-profile-default" });
const db = readDb();
check("cookies on disk are encrypted (fuse on; v10 prefix, empty plaintext column)", db.rows.includes("763130") && !db.plaintextInFile, SCRIPTED, { ...db, limit: "dev runs on the mock keychain: the real Brigadier-named item stays unproved until a signed build" });
await call({ op: "restart" }, 240000);
await openShell(name);
await sleep(1200);
const tabsAfter = await tabsOf();
check("open tabs come back after a clean quit", JSON.stringify(tabsAfter.map((t) => t.id)) === JSON.stringify(tabsBefore.map((t) => t.id)), SCRIPTED, { tabs: tabsAfter.length });
const page2 = await waitFor("page after restart", async () => { const page = (await layerState(name)).pages.find((each) => each.id === firstId); return page?.wc ? page : null; });
await guestExec(page2.wc, `location.href=${JSON.stringify(`${FIXTURES}/whoami`)};1`).catch(() => {});
await sleep(1200);
const who = await guestExec(page2.wc, "document.body.innerText");
check("the sign-in's session and persistent cookies survive a clean quit", who.includes("sid=session-value") && who.includes("keep=persistent-value"), SCRIPTED, { who });
await guestExec(page2.wc, `location.href=${JSON.stringify(`${FIXTURES}/sign-in`)};1`).catch(() => {});
await sleep(1000);
await guestExec(page2.wc, `document.cookie="afterCrash=1;path=/";1`);
await sleep(1600);
await call({ op: "restart", signal: "kill" }, 240000);
await openShell(name);
const page3 = await waitFor("page after crash", async () => { const page = (await layerState(name)).pages.find((each) => each.id === firstId); return page?.wc ? page : null; }, 15000);
await sleep(800);
const crashCookies = (await control("cookies", { partition: "persist:browser-profile-default" })).map((c) => c.name);
check("a session cookie survives a crash 1.6 s after it's set", crashCookies.includes("afterCrash") && crashCookies.includes("sid"), SCRIPTED, { crashCookies });
await guestExec(page3.wc, "1");

fs.writeFileSync(path.join(folder, "results.json"), JSON.stringify({ at: new Date().toISOString(), passed: results.every((r) => r.passed), results }, null, 1));
console.log(results.every((r) => r.passed) ? "ALL PASSED" : "SOME FAILED");
process.exit(0);
