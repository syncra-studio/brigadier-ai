#!/usr/bin/env node
// The driver's command line. Talks to devd over its socket; devd owns the kept-alive instance.
//   bc up | down | wipe | status | restart | targets
//   bc control <op> [json]       a fixed operation in the main process (devControl.mjs)
//   bc host <window> <js>        evaluate in that window's page over the debugging pipe
import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { checkDir, socketPath } from "./paths.mjs";

export function call(message, timeoutMs = 120000) {
  return new Promise((resolve, reject) => {
    const socket = net.connect(socketPath);
    let data = "";
    const timer = setTimeout(() => { socket.destroy(); reject(new Error(`no answer in ${timeoutMs} ms`)); }, timeoutMs);
    socket.on("connect", () => socket.end(JSON.stringify(message)));
    socket.on("data", (chunk) => { data += chunk; });
    socket.on("end", () => {
      clearTimeout(timer);
      const reply = JSON.parse(data);
      if (reply.error) reject(new Error(reply.error));
      else resolve(reply.result);
    });
    socket.on("error", (error) => { clearTimeout(timer); reject(error); });
  });
}

async function up() {
  try {
    return await call({ op: "status" }, 3000);
  } catch {
    fs.mkdirSync(checkDir, { recursive: true });
    const log = fs.openSync(path.join(checkDir, "devd.log"), "a");
    const here = path.dirname(fileURLToPath(import.meta.url));
    spawn(process.execPath, [path.join(here, "devd.mjs")], { detached: true, stdio: ["ignore", log, log] }).unref();
    for (let tries = 0; tries < 240; tries++) {
      await new Promise((resolve) => setTimeout(resolve, 500));
      try {
        const status = await call({ op: "status" }, 2000);
        if (status.controlReady) return status;
      } catch { /* still starting */ }
    }
    throw new Error(`devd did not come up; see ${path.join(checkDir, "devd.log")}`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [command, first, second] = process.argv.slice(2);
  let result;
  switch (command) {
    case "up": result = await up(); break;
    case "down": result = await call({ op: "down" }); break;
    case "wipe": {
      // Only ever the scratch folder under checkDir, while nothing is running.
      fs.rmSync(path.join(checkDir, "data"), { recursive: true, force: true });
      result = true;
      break;
    }
    case "status": result = await call({ op: "status" }); break;
    case "restart": result = await call({ op: "restart", signal: first }, 240000); break;
    case "targets": result = await call({ op: "targets" }); break;
    case "control": result = await call({ op: "control", name: first, args: second ? JSON.parse(second) : {} }); break;
    case "host": result = await call({ op: "host", window: first, js: second }); break;
    default:
      console.error("usage: bc up|down|wipe|status|restart|targets|control <op> [json]|host <window> <js>");
      process.exit(64);
  }
  console.log(JSON.stringify(result, null, 1));
}
