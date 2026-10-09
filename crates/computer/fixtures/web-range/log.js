// Posts one event to the fixture server's log, in the order it happened. The fields match the
// native fixture's log (id, ev, x, y, v), plus `trusted`: whether the browser marked the event as
// the user's own input.
const logStart = performance.timeOrigin;
let logChain = Promise.resolve();
function log(id, ev, extra = {}) {
  const line = { t: (logStart + performance.now()) / 1000, id, ev, page: location.pathname, ...extra };
  const body = JSON.stringify(line);
  logChain = logChain.then(() => fetch("/log", { method: "POST", body, keepalive: true }).catch(() => {}));
}
