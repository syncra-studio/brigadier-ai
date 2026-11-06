// Fixture pages for section 1's right-click menu and popups.
//   /guests/menu            a link, an image, selected text and a filled field to right-click
//   /guests/opener?child=U  window.open with features on U, a target=_blank link, window.open with none;
//                           its log is __fixture.log and on the page
//   /guests/links           mailto:, tel:, file: and another scheme, in the tab and target=_blank, and in a
//                           cross-site frame; a button that tries twice on one press
//   /guests/child           says hello to its opener, shows the answer, and answers back: the round trip
const head = (title, color) => `<!doctype html><meta charset="utf-8"><title>${title}</title>
<style>html,body{margin:0;height:100%}body{background:#${color};color:#fff;font:16px system-ui;padding:16px}
a{color:#fff}button{font:16px system-ui;display:block;margin:8px 0;width:360px;height:40px}pre{font:13px ui-monospace,monospace;white-space:pre-wrap}</style>`;

const IMAGE = "data:image/svg+xml," + encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="160" height="90"><rect width="160" height="90" fill="#d9a441"/></svg>`);

const menu = () => `${head("menu fixture", "2d4a6c")}
<p><a id="link" href="/page/336699/linked">a link to right-click</a></p>
<p><img id="image" src="${IMAGE}" alt="image"></p>
<p id="text">Some text to select and right-click.</p>
<p><input id="field" value="text in a field" style="width:300px;height:30px;font-size:16px"></p>
<div id="plain" style="height:200px">the page itself</div>`;

const opener = (child) => `${head("opener fixture", "3b2d6c")}
<button id="features">window.open with features</button>
<p><a id="blank" href="/page/336699/from-blank" target="_blank">a target=_blank link</a></p>
<button id="nofeatures">window.open with no features</button>
<pre id="log"></pre>
<script>
window.__fixture={log:[],handle:null,returned:{}};
const say=(line)=>{__fixture.log.push(line);document.getElementById("log").textContent=__fixture.log.join("\\n")};
const child=${JSON.stringify(child)};
document.getElementById("features").onclick=()=>{const w=window.open(child,"signin","width=480,height=420");__fixture.handle=w;__fixture.returned.features=Boolean(w);say("window.open with features returned "+(w?"a window":"null"))};
document.getElementById("nofeatures").onclick=()=>{const w=window.open("/page/226644/from-open");__fixture.returned.none=Boolean(w);say("window.open with no features returned "+(w?"a window":"null"))};
addEventListener("message",(event)=>{say("from "+event.origin+": "+event.data);if(event.data==="hello from the popup"){event.source.postMessage("token for the popup","*");say("answered it")}});
</script>`;

const child = () => `${head("popup fixture", "1f5c46")}
<div>window.opener: <b id="opener"></b></div><pre id="log"></pre>
<script>
window.__fixture={log:[],hasOpener:Boolean(window.opener)};
document.getElementById("opener").textContent=window.opener?"kept":"null";
const say=(line)=>{__fixture.log.push(line);document.getElementById("log").textContent=__fixture.log.join("\\n")};
addEventListener("message",(event)=>{say("from "+event.origin+": "+event.data);event.source.postMessage("thanks, got the token","*")});
if(window.opener){window.opener.postMessage("hello from the popup","*");say("said hello")}
</script>`;

const links = (origin) => `${head("links fixture", "5c3a1f")}
<style>a{display:block;margin:6px 0}iframe{border:0;width:360px;height:40px}</style>
<a id="mail" href="mailto:someone@example.invalid">mailto in this tab</a>
<a id="mail-blank" href="mailto:blank@example.invalid" target="_blank">mailto target=_blank</a>
<a id="tel" href="tel:+15550100">tel in this tab</a>
<a id="tel-blank" href="tel:+15550101" target="_blank">tel target=_blank</a>
<a id="file" href="file:///etc/hosts">file in this tab</a>
<a id="file-blank" href="file:///etc/hosts" target="_blank">file target=_blank</a>
<a id="other" href="slack://open">another scheme in this tab</a>
<a id="other-blank" href="slack://open" target="_blank">another scheme target=_blank</a>
<button id="twice">one press, two mailtos</button>
<iframe id="frame" src="${origin}/guests/link-frame"></iframe>
<script>document.getElementById("twice").onclick=()=>{window.open("mailto:first@example.invalid");window.open("mailto:second@example.invalid")}</script>`;

const linkFrame = () => `${head("link frame", "444444")}<style>body{padding:0;margin:0}</style><a id="mail" href="mailto:frame@example.invalid">mailto in a cross-site frame</a>`;

/** Answers `/guests/…`. */
export function guestFixture(parts, url, response) {
  response.setHeader("content-type", "text/html");
  if (parts[1] === "menu") response.end(menu());
  else if (parts[1] === "opener") response.end(opener(url.searchParams.get("child") ?? "/guests/child"));
  else if (parts[1] === "child") response.end(child());
  else if (parts[1] === "links") response.end(links(url.searchParams.get("frame") ?? ""));
  else if (parts[1] === "link-frame") response.end(linkFrame());
  else { response.statusCode = 404; response.end("no"); }
}
