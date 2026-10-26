import { useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { VisualsContext, receiveFrameMessage } from "./bridge";
import { IDIOMORPH_JS } from "./idiomorph";
import { extractCompleteStyles, processPartialHtml } from "./partial-html";

/** The final document gets a fresh frame, so preview state cannot leak into its scripts. */
export function VisualFrame({ code, writing }: { code: string; writing: boolean }) {
  return <Frame key={writing ? "preview" : `final:${code}`} code={code} writing={writing} />;
}

function Frame({ code, writing }: { code: string; writing: boolean }) {
  const visuals = useContext(VisualsContext);
  useLayoutEffect(() => {
    // Keep the same navigation boundary in dev builds, whose page comes from Vite.
    if (document.head.querySelector('[data-visual-frame-policy]')) return;
    const policy = document.createElement("meta");
    policy.httpEquiv = "Content-Security-Policy";
    policy.content = navigator.userAgent.includes("Windows")
      ? "frame-src http://brigadier-ui.localhost"
      : "frame-src brigadier-ui:";
    policy.dataset.visualFramePolicy = "";
    document.head.appendChild(policy);
  }, []);
  const frame = useRef<HTMLIFrameElement>(null);
  const port = useRef<MessagePort | null>(null);
  const [ready, setReady] = useState(false);
  const [height, setHeight] = useState(50);
  const content = useRef(code);
  useEffect(() => { content.current = code; }, [code]);
  const latest = useRef(visuals);
  useEffect(() => { latest.current = visuals; }, [visuals]);
  const start = () => {
    port.current?.close();
    setReady(false);
    const channel = new MessageChannel();
    port.current = channel.port1;
    channel.port1.addEventListener("message", ({ data }) => {
      const message = receiveFrameMessage(channel.port1, port.current, data);
      if (!message) return;
      if (message.type === "ready") setReady(true);
      if (message.type === "resize") setHeight(message.height);
      if (message.type === "sendPrompt" && !latest.current?.running) latest.current?.prefill(message.text);
      if (message.type === "openLink") latest.current?.openLink(message.url);
    });
    channel.port1.start();
    frame.current?.contentWindow?.postMessage("brigadier-visual", "*", [channel.port2]);
  };
  useEffect(() => () => { port.current?.close(); port.current = null; }, []);
  useEffect(() => {
    if (!ready) return;
    const render = () => {
      const html = content.current;
      const style = getComputedStyle(document.documentElement);
      const theme = ["--foreground", "--background", "--border", "--secondary", "--muted-foreground", "--link", "--font-sans", "--text-sm", "--radius-capsule", "--spacing"].map((name) => `${name}:${style.getPropertyValue(name)};`).join("");
      port.current?.postMessage({
        type: "render", final: !writing, theme: `:root{${theme}--spacing-2:calc(var(--spacing) * 2);--spacing-3:calc(var(--spacing) * 3);}`,
        html: writing ? `${extractCompleteStyles(html)}${processPartialHtml(html)}` : html,
        morph: writing ? IDIOMORPH_JS : undefined,
      });
    };
    if (!writing) { render(); return; }
    render();
    const timer = setInterval(render, 1000);
    return () => clearInterval(timer);
  }, [writing, ready]);
  return <iframe ref={frame} title={writing ? "Drawing…" : "Interactive visual"} sandbox="allow-scripts" src={typeof navigator !== "undefined" && navigator.userAgent.includes("Windows") ? "http://brigadier-ui.localhost/frame" : "brigadier-ui://localhost/frame"} onLoad={start} className="my-3 w-full rounded-2xl border border-border bg-background" style={{ height }} />;
}
