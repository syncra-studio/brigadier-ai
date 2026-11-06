// The local page the checks can read exactly. Its colour says which tab it is; a counter, its rAF and a
// stamp set on load show whether it's still the same document, alive, and painting.
import http from "node:http";

export const FIXTURE_PORT = Number(process.env.BRIGADIER_FIXTURE_PORT ?? 1432);

const page = (color, name) => `<!doctype html><meta charset="utf-8"><title>${name}</title>
<style>html,body{margin:0;height:100%}body{background:#${color};font:28px system-ui;color:#fff}
#band{position:fixed;left:0;right:0;bottom:0;height:60px;background:transparent}#tall{height:6000px}
a{color:#fff;display:block;padding:10px;width:240px}input{display:block;margin:10px;width:300px;height:30px;font-size:18px}</style>
<a id="link" href="#linked">link ${name}</a><input id="field" autocomplete="off"><div id="count">0</div><div id="tall"></div><div id="band"></div>
<script>
window.__fixture={name:${JSON.stringify(name)},loadedAt:Date.now(),timeOrigin:performance.timeOrigin,raf:0,ticks:0,visibility:[[Date.now(),document.visibilityState]],pointer:0,wheel:0,keys:0,clicks:0};
requestAnimationFrame(function f(){__fixture.raf++;requestAnimationFrame(f)});
setInterval(()=>{__fixture.ticks++;document.getElementById("count").textContent=__fixture.ticks},1000);
document.addEventListener("visibilitychange",()=>__fixture.visibility.push([Date.now(),document.visibilityState]));
addEventListener("pointermove",()=>__fixture.pointer++);addEventListener("wheel",()=>__fixture.wheel++);addEventListener("keydown",()=>__fixture.keys++);addEventListener("click",()=>__fixture.clicks++);
window.__paintNonce=(hex)=>{document.getElementById("band").style.background="#"+hex};
</script>`;

// Page colour to its very edges, a marker in each corner: what "the page fills its slot" reads in pixels.
const fill = (color) => `<!doctype html><meta charset="utf-8"><title>fill</title>
<style>html,body{margin:0;height:100%;overflow:hidden;background:#${color}}i{position:fixed;width:24px;height:24px}</style>
<i style="left:0;top:0;background:#f00"></i><i style="right:0;top:0;background:#0f0"></i><i style="left:0;bottom:0;background:#ff0"></i><i style="right:0;bottom:0;background:#0ff"></i>`;

// Nothing that runs: for idle and timing, where a counter or rAF loop would be measured instead of the app.
const still = (color, name) => `<!doctype html><meta charset="utf-8"><title>${name}</title><style>html,body{margin:0;height:100%;background:#${color}}</style>`;

// A new colour each time the tab comes back, painted by the page once it's told it's visible. A hidden
// renderer paints nothing, so what its old surface holds is the colour of the last showing: stale pixels
// can't look like this showing's.
const fresh = (first, second, name) => `<!doctype html><meta charset="utf-8"><title>${name}</title><style>html,body{margin:0;height:100%}</style><script>
window.__shows=0;const paint=()=>{document.documentElement.style.background=window.__shows%2?"#${second}":"#${first}"};paint();
document.addEventListener("visibilitychange",()=>{if(document.visibilityState==="visible"){window.__shows++;paint()}});</script>`;

// Each press or key flips the colour, and Event Timing records how long until the next paint.
const input = (a, b) => `<!doctype html><meta charset="utf-8"><title>input</title><style>html,body{margin:0;height:100%}</style><script>
let on=false;const flip=()=>{on=!on;document.documentElement.style.background=on?"#${b}":"#${a}"};flip();
addEventListener("pointerdown",flip);addEventListener("keydown",flip);
window.__timing=[];new PerformanceObserver((list)=>{for(const e of list.getEntries())__timing.push([e.name,e.duration])}).observe({type:"event",durationThreshold:16,buffered:true});</script>`;

export function startFixtures() {
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://x");
    const parts = url.pathname.split("/").filter(Boolean);
    response.setHeader("cache-control", "no-store");
    if (parts[0] === "page") {
      response.setHeader("content-type", "text/html");
      response.end(page(parts[1] ?? "336699", parts[2] ?? "page"));
    } else if (parts[0] === "fill") {
      response.setHeader("content-type", "text/html");
      response.end(fill(parts[1]));
    } else if (parts[0] === "still") {
      response.setHeader("content-type", "text/html");
      response.end(still(parts[1], parts[2] ?? "still"));
    } else if (parts[0] === "fresh") {
      response.setHeader("content-type", "text/html");
      response.end(fresh(parts[1], parts[2], parts[3] ?? "fresh"));
    } else if (parts[0] === "input") {
      response.setHeader("content-type", "text/html");
      response.end(input(parts[1], parts[2]));
    } else if (parts[0] === "sign-in") {
      // One session cookie and one persistent, as a real sign-in sets them.
      response.setHeader("set-cookie", ["sid=session-value; Path=/; HttpOnly; SameSite=Lax", "keep=persistent-value; Path=/; Max-Age=86400"]);
      response.setHeader("content-type", "text/html");
      response.end(`<!doctype html><title>signed in</title><body style="background:#225522;color:#fff">sent cookies: ${request.headers.cookie ?? "none"}</body>`);
    } else if (parts[0] === "whoami") {
      response.setHeader("content-type", "text/html");
      response.end(`<!doctype html><title>whoami</title><body id="b" style="background:#552222;color:#fff">${request.headers.cookie ?? "none"}</body>`);
    } else {
      response.statusCode = 404;
      response.end("no");
    }
  });
  return new Promise((resolve) => server.listen(FIXTURE_PORT, "127.0.0.1", () => resolve(server)));
}

// Alone, beside a running instance, when the pages change and the instance should stay up.
if (process.argv[1] === new URL(import.meta.url).pathname) await startFixtures();
