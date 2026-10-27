import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";

const html = readFileSync(new URL("../../../../src-tauri/src/visuals_frame.html", import.meta.url), "utf8");
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((match) => match[1]!);

/** A read-only native namespace with an inherited transport, as WebKit exposes it. */
function frame(lockedPeer = false) {
  const context = createContext({});
  runInContext(`
    window = globalThis;
    const calls = [];
    const handlers = {ipc:{postMessage:()=>calls.push('native IPC')}};
    class NativeNamespace {}
    Object.defineProperty(NativeNamespace.prototype, 'messageHandlers', {get:()=>handlers, configurable:true});
    Object.defineProperty(window, 'webkit', {value:new NativeNamespace(), configurable:false, writable:false});
    const network = () => calls.push('network');
    Object.defineProperty(window, 'RTCPeerConnection', {value:network, configurable:${!lockedPeer}, writable:${!lockedPeer}});
    webkitRTCPeerConnection = network;
    RTCIceGatherer = network;
    WebSocket = network;
    EventSource = network;
    fetch = network;
    class Navigator {}
    Navigator.prototype.sendBeacon = network;
    Navigator.prototype.serviceWorker = {};
    navigator = new Navigator();
    const events = [];
    document = {body:{textContent:''},getElementById:()=>({}),addEventListener:()=>{}};
    addEventListener = (type) => events.push(type);
    ResizeObserver = class {observe(){}};
  `, context);
  for (const script of scripts) runInContext(script, context);
  return context;
}

test("model code cannot restore network APIs or recover IPC through a prototype", () => {
  const context = frame();
  const result = runInContext(`(() => {
    const errors = [];
    for (const attack of [
      () => new RTCPeerConnection({iceServers:[{urls:'stun:127.0.0.1:9'}]}),
      () => new webkitRTCPeerConnection(), () => new RTCIceGatherer(),
      () => new WebSocket('ws://127.0.0.1:9'), () => new EventSource('http://127.0.0.1:9'),
      () => fetch('http://127.0.0.1:9'), () => navigator.sendBeacon('http://127.0.0.1:9'),
      () => webkit.messageHandlers.ipc.postMessage('{}'),
      () => Object.defineProperty(window,'RTCPeerConnection',{value:Function}),
      () => Object.defineProperty(webkit,'messageHandlers',{value:{}}),
      () => Object.getOwnPropertyDescriptor(Object.getPrototypeOf(webkit),'messageHandlers').get.call(webkit),
      () => Object.getPrototypeOf(navigator).sendBeacon('http://127.0.0.1:9'),
    ]) { try {attack();errors.push('worked');} catch(error) {errors.push(error.name);} }
    return {errors,calls,events,rtc:Object.getOwnPropertyDescriptor(window,'RTCPeerConnection').configurable,
      ipc:Object.getOwnPropertyDescriptor(webkit,'messageHandlers').configurable};
  })()`, context) as { errors: string[]; calls: string[]; events: string[]; rtc: boolean; ipc: boolean };
  assert.equal(result.errors.length, 12);
  assert.ok(result.errors.every((error) => error === "TypeError"), JSON.stringify(result));
  assert.equal(result.calls.length, 0);
  assert.ok(result.events.includes("message"), "ordinary frame rendering still starts");
  assert.equal(result.rtc, false);
  assert.equal(result.ipc, false);
});

test("a webview that cannot seal an API never receives model content", () => {
  const context = frame(true);
  assert.equal(runInContext("events.includes('message')", context), false);
  assert.equal(runInContext("document.body.textContent", context), "This visual cannot run in this webview.");
});

test("the frame response blocks nested frames, forms and network resources", () => {
  const source = readFileSync(new URL("../../../../src-tauri/src/main.rs", import.meta.url), "utf8");
  const csp = /\.header\("Content-Security-Policy", "([^"]+)"\)/.exec(source)![1]!;
  for (const directive of ["default-src 'none'", "frame-src 'none'", "child-src 'none'", "worker-src 'none'", "object-src 'none'", "connect-src 'none'", "form-action 'none'"]) {
    assert.ok(csp.includes(directive), directive);
  }
  assert.match(source, /\.header\("X-DNS-Prefetch-Control", "off"\)/);
});
