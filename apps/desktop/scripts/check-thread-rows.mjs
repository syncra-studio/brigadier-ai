// Checks the thread's rows against the measured target (THREAD-PARITY-PLAN.md §2, §4.1–§4.4,
// §4.7), in headless Chromium on the recorded sessions, with every fold and group open:
// - every chevron (rows, work headers, the workers rail) is centred on its words' line within
//   0.5px, in Normal and in Compact;
// - in Normal, the header, row, gap, bubble and action bar sizes match the target within 1px.
// It prints what it measured. Run against Vite:
//   PLAYWRIGHT_MODULE=<module path> CHROME_BIN=<binary> node scripts/check-thread-rows.mjs <base-url>
import assert from "node:assert/strict";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const base = process.argv[2] || "http://127.0.0.1:1420";
const CENTRED_PX = 0.5;
const SIZE_PX = 1;

const PAGES = ["thread-session.html?session=grill&done=1", "thread-session.html?done=1", "thread-replica.html?summary=0&approvals=0&view=done"];

/** The target's sizes, in CSS px (§2). */
const TARGET = {
  headerLine: 21,
  headerToRule: 8,
  header: 30,
  ruleToFirst: 16,
  row: 21,
  rowGap: 4,
  itemGap: 16,
  chevron: 14,
  bubblePaddingBlock: 10,
  bubblePaddingInline: 16,
  bubbleRadius: 22,
  bubbleMaxShare: 0.7,
  actionButton: 26,
  actionGap: 2,
  answerToActions: 6,
  bubbleToHeader: 48,
};

const browser = await chromium.launch({ executablePath: process.env.CHROME_BIN, headless: true });
const failures = [];
try {
  for (const density of ["normal", "compact"]) {
    for (const path of PAGES) {
      const page = await browser.newPage({ viewport: { width: 1728, height: 1024 }, deviceScaleFactor: 2, colorScheme: "dark", reducedMotion: "reduce" });
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto(`${base}/fixtures/${path}&density=${density}`);
      await page.locator('[data-slot="request-work-header"]').first().waitFor();
      await page.evaluate(openEverything);
      await page.waitForTimeout(400);
      const measured = await page.evaluate(measure);
      const { chevrons } = measured;
      // To a hundredth of a pixel; null where the page has no such thing.
      const sizes = Object.fromEntries(
        Object.entries(measured.sizes).map(([key, value]) => [key, typeof value === "number" && !Number.isNaN(value) ? Math.round(value * 100) / 100 : null]),
      );
      const label = `${path}, ${density}`;
      const off = chevrons.filter((chevron) => Math.abs(chevron.dy) > CENTRED_PX);
      console.log(`${label}: ${chevrons.length} chevrons, ${off.length} off centre (largest ${Math.max(0, ...chevrons.map((chevron) => Math.abs(chevron.dy))).toFixed(2)}px)`);
      for (const chevron of off) failures.push(`${label}: "${chevron.text}" chevron ${chevron.dy.toFixed(2)}px off its line`);
      if (chevrons.length === 0) failures.push(`${label}: no chevrons found`);
      console.log(`  ${JSON.stringify(sizes)}`);
      if (density === "normal") {
        for (const [key, value] of Object.entries(sizes)) {
          const want = TARGET[key];
          if (want === undefined || value === null) continue;
          const tolerance = key === "bubbleMaxShare" ? 0.01 : SIZE_PX;
          if (Math.abs(value - want) > tolerance) failures.push(`${label}: ${key} is ${value}, the target ${want}`);
        }
      }
      assert.deepEqual(errors, [], `${label}: page errors`);
      await page.close();
    }
  }
} finally {
  await browser.close();
}
if (failures.length > 0) {
  console.error(`\n${failures.length} problem(s):\n${failures.join("\n")}`);
  process.exit(1);
}
console.log("\nEvery chevron is centred on its line, and the sizes match the target.");

/** Runs in the page: opens every turn's work, then every group in it, and the workers rail. */
async function openEverything() {
  for (const header of document.querySelectorAll('button[data-slot="request-work-header"][aria-expanded="false"]')) header.click();
  await new Promise((resolve) => setTimeout(resolve, 100));
  for (const group of document.querySelectorAll('[data-slot="work-group"] > button[aria-expanded="false"]')) group.click();
  document.querySelector('[data-slot="workers-strip"] button[aria-expanded="false"]')?.click();
}

/** Runs in the page: every chevron's offset from its words' line, and the sizes of §2. */
function measure() {
  // It runs in the page, serialized on its own: its helpers live inside it.
  // oxlint-disable-next-line unicorn/consistent-function-scoping
  const box = (element) => element.getBoundingClientRect();
  const centre = (element) => box(element).top + box(element).height / 2;
  const visible = (element) => box(element).height > 0 && box(element).width > 0;
  const chevrons = [...document.querySelectorAll("svg")]
    .filter((svg) => svg.matches('[class*="size-chevron"]') && visible(svg))
    .map((svg) => {
      const row = svg.closest('button, [role="button"]');
      // The words: the row's first piece of text that isn't an icon.
      const words = [...(row?.querySelectorAll("span") ?? [])].find((span) => span.textContent.trim() && !span.querySelector("svg") && visible(span));
      return { text: (row?.textContent ?? "").trim().slice(0, 40), dy: words ? centre(svg) - centre(words) : Number.NaN };
    });

  // From the first user bubble with a work header after it, to that header.
  const bubbleToHeader = () => {
    for (const bubble of document.querySelectorAll(".aui-user-message-content")) {
      const header = [...document.querySelectorAll('[data-slot="request-work-header"]')].find(
        (candidate) => bubble.compareDocumentPosition(candidate) & Node.DOCUMENT_POSITION_FOLLOWING,
      );
      if (header) return box(header).top - box(bubble).bottom;
    }
    return null;
  };
  const header = document.querySelector('button[data-slot="request-work-header"]');
  const label = header?.querySelector('[data-slot="request-work-label"]');
  const fold = header?.parentElement?.querySelector('[data-slot="request-fold"] [data-slot="request-work"]');
  const first = fold?.firstElementChild;
  const group = [...document.querySelectorAll('[data-slot="work-group"] [data-state="open"] .max-h-group-list')].find((list) => list.children.length > 2);
  const rows = group ? [...group.children].filter(visible) : [];
  const gaps = rows.slice(1).map((row, index) => box(row).top - box(rows[index]).bottom);
  const items = fold ? [...fold.children].filter(visible) : [];
  const itemGaps = items.slice(1).map((item, index) => box(item).top - box(items[index]).bottom);
  const row = document.querySelector('[data-slot="request-fold"] [data-slot="orchestrator-step"], [data-slot="request-fold"] [data-slot="work-group"] > button');
  const bubble = document.querySelector(".aui-user-message-content");
  const bubbleStyle = bubble ? getComputedStyle(bubble) : null;
  const column = bubble?.closest('[data-slot="aui_user-message-root"]');
  const wrapper = bubble?.parentElement;
  const bar = [...document.querySelectorAll('[data-slot="answer-actions"]')].find(visible);
  const actions = bar ? [...bar.querySelectorAll(".aui-button-icon")].filter(visible) : [];
  const answer = bar?.closest('[data-slot="aui_assistant-message-root"]')?.querySelector('[data-slot="request-body"]');
  const siblings = bar ? [...bar.parentElement.children].filter(visible) : [];
  const before = siblings[siblings.indexOf(bar) - 1];
  return {
    chevrons,
    sizes: {
      headerLine: label ? box(label).height : null,
      headerToRule: header && label ? box(header).bottom - 1 - box(label).bottom : null,
      header: header ? box(header).height : null,
      ruleToFirst: header && first ? box(first).top - box(header).bottom : null,
      row: row ? box(row).height : null,
      rowGap: gaps.length ? Math.max(...gaps) : null,
      itemGap: itemGaps.length ? Math.min(...itemGaps) : null,
      chevron: chevrons.length ? box(document.querySelector('[class*="size-chevron"]')).width : null,
      bubblePaddingBlock: bubbleStyle ? Number.parseFloat(bubbleStyle.paddingTop) : null,
      bubblePaddingInline: bubbleStyle ? Number.parseFloat(bubbleStyle.paddingLeft) : null,
      bubbleRadius: bubbleStyle ? Number.parseFloat(bubbleStyle.borderTopLeftRadius) : null,
      bubbleMaxShare: wrapper && column ? Number.parseFloat(getComputedStyle(wrapper).maxWidth) / 100 : null,
      actionButton: actions[0] ? box(actions[0]).height : null,
      actionGap: actions[1] ? box(actions[1]).left - box(actions[0]).right : null,
      bubbleToHeader: bubbleToHeader(),
      answerToActions: bar && before && answer ? box(bar).top - box(before).bottom : null,
    },
  };
}
