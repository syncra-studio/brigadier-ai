import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { invoke, listen, nativeWindow, newChannel, onDragDrop, type DragDropPayload } from "@/ipc/native";

type Place = { window?: unknown };
const place = globalThis as Place;

afterEach(() => {
  delete place.window;
});

test("without Electron's preload a command reaches Tauri, channel included", async () => {
  const calls: unknown[][] = [];
  place.window = {
    __TAURI_INTERNALS__: {
      transformCallback: () => 7,
      invoke: async (...args: unknown[]) => {
        calls.push(args);
        return "from tauri";
      },
    },
  };
  const channel = newChannel<string>(() => {});
  assert.equal(await invoke("a_command", { channel }), "from tauri");
  const [command, args] = calls[0] as [string, { channel: unknown }];
  assert.equal(command, "a_command");
  assert.equal(args.channel, channel);
});

function fakeElectron() {
  const sent: { command: string; args: unknown }[] = [];
  const eventHandlers = new Map<string, (payload: unknown) => void>();
  let channelHandler: ((id: number, message: unknown) => void) | undefined;
  const page = new EventTarget();
  const native = {
    invoke: async (command: string, args: unknown) => {
      sent.push({ command, args });
      return "from electron";
    },
    listen: (event: string, handler: (payload: unknown) => void) => {
      eventHandlers.set(event, handler);
      return () => eventHandlers.delete(event);
    },
    onChannel: (handler: (id: number, message: unknown) => void) => {
      channelHandler = handler;
    },
    isFullscreen: async () => true,
    onResized: () => () => {},
    closeWindow: async () => {},
    pathForFile: (file: { path: string }) => file.path,
  };
  place.window = Object.assign(page, { brigadierNative: native });
  return { sent, eventHandlers, page, deliver: (id: number, message: unknown) => channelHandler?.(id, message) };
}

test("under Electron a channel crosses as its id and its messages come back to it", async () => {
  const electron = fakeElectron();
  const heard: string[] = [];
  const channel = newChannel<string>((message) => heard.push(message));
  assert.equal(await invoke("subscribe", { channel, other: 1 }), "from electron");
  const args = electron.sent[0]?.args as { channel: { channelId: number }; other: number };
  assert.equal(args.other, 1);
  electron.deliver(args.channel.channelId, "hello");
  electron.deliver(args.channel.channelId + 1000, "for nobody");
  assert.deepEqual(heard, ["hello"]);
});

test("under Electron an event carries its payload and can be stopped", async () => {
  const electron = fakeElectron();
  const heard: number[] = [];
  const stop = await listen<number>("flush-pages", ({ payload }) => heard.push(payload));
  electron.eventHandlers.get("flush-pages")?.(5);
  assert.deepEqual(heard, [5]);
  stop();
  assert.equal(electron.eventHandlers.has("flush-pages"), false);
  assert.equal(await nativeWindow.isFullscreen(), true);
});

function dragEvent(type: string, paths: string[], x = 3, y = 4) {
  const event = new Event(type, { cancelable: true });
  Object.assign(event, {
    clientX: x,
    clientY: y,
    dataTransfer: { types: ["Files"], files: paths.map((path) => ({ path })) },
  });
  return event;
}

test("under Electron a drag is told once across the elements it crosses, and a drop gives the paths", async () => {
  const electron = fakeElectron();
  // onDragDrop listens on the page's window, which the fake is.
  const heard: DragDropPayload[] = [];
  const stop = await onDragDrop((payload) => heard.push(payload));
  electron.page.dispatchEvent(dragEvent("dragenter", ["/a"]));
  electron.page.dispatchEvent(dragEvent("dragenter", ["/a"]));
  electron.page.dispatchEvent(dragEvent("dragleave", []));
  assert.deepEqual(heard.map((payload) => payload.type), ["enter"]);
  const drop = dragEvent("drop", ["/a"]);
  electron.page.dispatchEvent(drop);
  assert.equal(drop.defaultPrevented, true);
  assert.deepEqual(heard.at(-1), { type: "drop", paths: ["/a"], position: { x: 3, y: 4 } });
  electron.page.dispatchEvent(dragEvent("dragenter", ["/b"]));
  electron.page.dispatchEvent(dragEvent("dragleave", []));
  assert.equal(heard.at(-1)?.type, "leave");
  stop();
  const before = heard.length;
  electron.page.dispatchEvent(dragEvent("dragenter", ["/c"]));
  assert.equal(heard.length, before);
});
