/**
 * A long Settings page and a short one in the page frame every Settings page uses: the edges
 * fade what scrolls under them as the conversation's do (`scroll-fade.tsx`). The frame is
 * FRAME_PX high, not the window's height: a window's view differs by browser (Chrome on Linux
 * draws 437px of an 800×600 window, its headless shell 600px), and the short page (500px) must
 * fit with room to spare on every one.
 *
 * ?drive=1 scrolls the long page to its top, its middle and its end, reading at each which edge
 * fades content and the fades' look, then the short page's, to #settings-fade-result
 * (SettingsFadeRender.test.ts reads it). Without it the page stays as it opens, for
 * screenshots; ?short=1 shows the short page.
 */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsPage, SettingsRow, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { SettingsCard } from "@/components/shared";

const params = new URLSearchParams(location.search);
/** The page frame's height: the short page fits it with 220px to spare, the long one scrolls. */
const FRAME_PX = 720;

function Page({ sections }: { sections: number }) {
  return (
    <SettingsPage title="Routing" description="Which agent and model take each kind of work.">
      {Array.from({ length: sections }, (_section, s) => (
        <SettingsSection key={s} title={`Section ${s + 1}`}>
          <SettingsCard>
            {Array.from({ length: 4 }, (_row, r) => (
              <SettingsRow key={r} label={`Setting ${s + 1}.${r + 1}`} description="What this setting changes, in a line.">
                <SettingsSwitch label={`Setting ${s + 1}.${r + 1}`} checked={r % 2 === 0} onCheckedChange={() => {}} />
              </SettingsRow>
            ))}
          </SettingsCard>
        </SettingsSection>
      ))}
    </SettingsPage>
  );
}

const $ = (selector: string) => document.querySelector<HTMLElement>(selector);
const frame = () => $('[data-slot="settings-page"]')!;
const scroller = () => frame().firstElementChild as HTMLElement;
const fade = (edge: string) => $(`[data-slot="scroll-fade"][data-edge="${edge}"]`);
const round = (n: number) => Math.round(n * 10) / 10;

/** Whether an edge's fade lies over any of the page's content, and so shows. */
function shows(edge: string): boolean | null {
  const over = fade(edge)?.getBoundingClientRect();
  if (!over) return null;
  const view = scroller().getBoundingClientRect();
  const content = [...frame().querySelectorAll("h1, h2, p, [data-slot='settings-card']")];
  return content.some((element) => {
    const box = element.getBoundingClientRect();
    const top = Math.max(box.top, over.top, view.top);
    const bottom = Math.min(box.bottom, over.bottom, view.bottom);
    return bottom - top > 0.5;
  });
}

/** Which edges fade content at this scroll. */
const edges = () => ({ top: shows("top"), bottom: shows("bottom") });

/** How a fade looks and where it lies. */
function look(edge: string) {
  const element = fade(edge)!;
  const style = getComputedStyle(element);
  const box = element.getBoundingClientRect();
  const view = scroller().getBoundingClientRect();
  return {
    height: round(box.height),
    image: style.backgroundImage,
    from: round(edge === "top" ? box.top - view.top : view.bottom - box.bottom),
    width: round(box.width - view.width),
    scrolls: scroller().contains(element),
    position: style.position,
    pointerEvents: style.pointerEvents,
  };
}

function Fixture() {
  const [short, setShort] = useState(params.has("short"));
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const frames = () => new Promise((resolve) => setTimeout(resolve, 200));
    void (async () => {
      await frames();
      const view = scroller();
      const max = view.scrollHeight - view.clientHeight;
      seen.scrollable = max;
      seen.background = getComputedStyle(frame().parentElement!).backgroundColor;
      // Transparent, as the browser writes it in a gradient.
      const probe = document.createElement("div");
      probe.style.backgroundColor = "transparent";
      document.body.append(probe);
      seen.clear = getComputedStyle(probe).backgroundColor;
      probe.remove();
      seen.atStart = edges();
      view.scrollTop = Math.round(max / 2);
      await frames();
      seen.midway = edges();
      seen.top = look("top");
      seen.bottom = look("bottom");
      view.scrollTop = max;
      await frames();
      seen.atEnd = edges();
      setShort(true);
      await frames();
      seen.short = { scrollable: scroller().scrollHeight - scroller().clientHeight, ...edges() };
      setResult(JSON.stringify(seen));
    })();
  }, []);
  return (
    <>
      <div className="bg-background text-foreground w-full" style={{ height: FRAME_PX }}>
        <Page key={String(short)} sections={short ? 1 : 8} />
      </div>
      {result && <pre id="settings-fade-result">{result}</pre>}
    </>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
