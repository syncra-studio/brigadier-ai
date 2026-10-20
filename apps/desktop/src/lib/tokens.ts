/**
 * Resolves a length token from styles/tokens.css to CSS pixels, for APIs that need numbers
 * (e.g. a virtualizer's row size).
 */
export function tokenPx(name: `--${string}`): number {
  // Rendered on the server (a test's static markup) there are no styles to read.
  if (typeof document === "undefined") return 0;
  const root = document.documentElement;
  const value = getComputedStyle(root).getPropertyValue(name).trim();
  const amount = Number.parseFloat(value);
  if (Number.isNaN(amount)) return 0;
  if (value.endsWith("rem")) {
    return amount * Number.parseFloat(getComputedStyle(root).fontSize);
  }
  return amount;
}

/**
 * Resolves a color token from styles/tokens.css (`--glyph-1`, `--background`) to `#rrggbbaa`,
 * for canvas and WebGL APIs that cannot read CSS colors themselves.
 */
export function tokenColor(name: `--${string}`): string {
  const probe = document.createElement("span");
  probe.style.color = `var(${name})`;
  document.body.append(probe);
  const resolved = getComputedStyle(probe).color;
  probe.remove();
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) throw new Error("no 2D canvas to resolve colors with");
  context.fillStyle = resolved;
  context.fillRect(0, 0, 1, 1);
  const channels = Array.from(context.getImageData(0, 0, 1, 1).data);
  return `#${channels.map((value) => value.toString(16).padStart(2, "0")).join("")}`;
}
