/**
 * Brigadier's mark drawn small (`smallCut`) and large, on the platform `?platform=` names
 * (macOS unless given). It writes which of its drawings show, each one's shapes, and what a
 * screen reader is given to #brand-glyph-result (BrandGlyphRender.test.ts reads it).
 */
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { BrigadierGlyph } from "@/components/icons";

document.documentElement.dataset.platform = new URLSearchParams(location.search).get("platform") ?? "macos";

/** The shapes the image shows: each one drawn, not inside a hidden group. */
function shown(svg: SVGSVGElement | null): string[] {
  if (!svg) return [];
  return [...svg.querySelectorAll("path, circle, rect")]
    .filter((shape) => {
      for (let at: Element | null = shape; at && at !== svg; at = at.parentElement)
        if (getComputedStyle(at).display === "none") return false;
      return true;
    })
    .map((shape) => shape.tagName);
}

function Fixture() {
  useEffect(() => {
    const small = document.querySelector<SVGSVGElement>("#small svg");
    const large = document.querySelector<SVGSVGElement>("#large svg");
    const count = (shapes: string[]) =>
      Object.fromEntries(["path", "circle", "rect"].map((tag) => [tag, shapes.filter((name) => name === tag).length]));
    const result = document.createElement("pre");
    result.id = "brand-glyph-result";
    result.textContent = JSON.stringify({
      small: count(shown(small)),
      large: count(shown(large)),
      // Both drawings are one image, hidden from screen readers (its button names it).
      images: document.querySelectorAll("#small svg").length,
      hidden: small?.getAttribute("aria-hidden"),
    });
    document.body.append(result);
    return () => result.remove();
  }, []);
  return (
    <>
      <div id="small">
        <BrigadierGlyph smallCut={20} className="size-5" />
      </div>
      <div id="large">
        <BrigadierGlyph className="size-20" />
      </div>
    </>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
