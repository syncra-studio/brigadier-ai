/**
 * An answer's optional parts (Q86): a few Markdown shapes the thread draws as parts of its own,
 * on top of free Markdown. The model uses a shape only when its answer really has it; an answer
 * with none renders exactly as before. Every part is keyed on shape, never on words, so it works
 * in any language.
 *
 * The parts are one registry, [`PART_SHAPES`] here and its views in `AnswerParts.tsx` (the type
 * makes each shape have one): adding a part is one entry in each and one line in the prompt's
 * `answer_parts!`. A view may be our own component or an installed assistant-ui one: `parse` turns
 * the Markdown into the view's own props.
 */
import { type AnswerSource, linkMarkers, splitSources } from "@/app/conversation/answerSources";

/** Where a block sits: right after the opening paragraph, and whether inside a fold. */
export type BlockPlace = { afterBottomLine: boolean; folded: boolean };

/** One part's shape: whether a Markdown block has it, and the block as the view's props. */
export type PartShape<P> = {
  /**
   * Whether the shape alone lays the answer out. A shape any answer may have by chance (a
   * numbered list) only does in a Chat; in a session it takes an explicit marker as well.
   */
  explicit: boolean;
  shape: (block: string, place: BlockPlace) => boolean;
  /** The view's props, or null when the block is too far off the shape: it stays Markdown. */
  parse: (block: string) => P | null;
};

export type Step = { title: string; detail: string };
export type StepsProps = { steps: Step[] };
export type CautionProps = { title: string | null; items: string[] };
export type ChanceProps = { of: number; caption: string };
export type PickProps = { markdown: string };

/** The link a pick row's ★ becomes: the Markdown renderer draws it as the "My pick" badge. */
export const PICK_HREF = "brigadier-pick:";

const ORDERED = /^(\d+)[.)][ \t]+/;
const BULLET = /^[-*+][ \t]+/;
const CAUTION = /^>[ \t]*\[!(?:CAUTION|WARNING)\][ \t]*$/i;
const CHANCE = /^```chance[ \t]*\n[ \t]*(\d{1,3})\b[^\n]*?\b100\b[^\n]*\n([\s\S]*?)\n?```[ \t]*$/;
const STAR = /[ \t]*★[ \t]*(\**)[ \t]*$/;

/** A flat numbered list of 3 to 7 items from 1, each item one paragraph: nothing nested. */
function steps(block: string): Step[] | null {
  const items: string[] = [];
  for (const line of block.split("\n")) {
    const item = ORDERED.exec(line);
    if (item) {
      if (Number(item[1]) !== items.length + 1) return null;
      items.push(line.slice(item[0].length).trim());
    } else if (items.length === 0 || /^\s*(?:[-*+]|\d+[.)])[ \t]/.test(line) || /^\s*(?:```|>|\|)/.test(line)) {
      return null;
    } else if (line.trim()) {
      items[items.length - 1] += ` ${line.trim()}`;
    }
  }
  if (items.length < 3 || items.length > 7) return null;
  return items.map((item) => {
    // "Verb phrase. One sentence of detail.": the phrase is the step's title.
    const cut = item.search(/[.!?](?=[ \t]+\S)/);
    return cut < 0 ? { title: item, detail: "" } : { title: item.slice(0, cut + 1), detail: item.slice(cut + 1).trim() };
  });
}

function caution(block: string): CautionProps | null {
  const lines = block.split("\n");
  if (!CAUTION.test(lines[0] ?? "") || lines.some((line) => line.trim() && !line.trimStart().startsWith(">"))) return null;
  let title: string | null = null;
  const items: string[] = [];
  for (const line of lines.slice(1).map((each) => each.trimStart().replace(/^>[ \t]?/, ""))) {
    if (!line.trim()) continue;
    if (BULLET.test(line) || ORDERED.test(line)) items.push(line.replace(BULLET, "").replace(ORDERED, "").trim());
    else if (items.length > 0) items[items.length - 1] += ` ${line.trim()}`;
    else if (title === null) title = line.trim().replace(/^\*\*(.*)\*\*$/, "$1");
    else items.push(line.trim());
  }
  return items.length > 0 || title ? { title, items } : null;
}

function chance(block: string): ChanceProps | null {
  const match = CHANCE.exec(block);
  if (!match) return null;
  const of = Number(match[1]);
  return of <= 100 ? { of, caption: match[2]!.trim().replace(/\s*\n\s*/g, " ") } : null;
}

/** A table whose row's first cell ends in ★: that ★ becomes the pick's badge, the first only. */
function pick(block: string): PickProps | null {
  const lines = block.split("\n");
  const start = lines.findIndex((line) => line.trimStart().startsWith("|"));
  if (start < 0 || !/^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(lines[start + 1] ?? "") || !/-/.test(lines[start + 1] ?? "")) return null;
  if (lines.slice(start).some((line) => line.trim() && !line.trimStart().startsWith("|"))) return null;
  for (let index = start + 2; index < lines.length; index++) {
    const cells = lines[index]!.split("|");
    // `| A ★ | …` splits to ["", " A ★ ", …]: the first cell is the second piece.
    const first = cells[1];
    if (first === undefined || !STAR.test(first)) continue;
    cells[1] = `${first.replace(STAR, "$1")} [My pick for you](${PICK_HREF}) `;
    lines[index] = cells.join("|");
    return { markdown: lines.join("\n") };
  }
  return null;
}

/** Every part's shape, by kind; first match wins. `AnswerParts.tsx` holds a view for each. */
export const PART_SHAPES = {
  caution: { explicit: true, shape: (block) => CAUTION.test(block.split("\n")[0] ?? ""), parse: caution } satisfies PartShape<CautionProps>,
  chance: { explicit: true, shape: (block) => block.startsWith("```chance"), parse: chance } satisfies PartShape<ChanceProps>,
  pick: { explicit: true, shape: (block) => block.includes("|") && block.includes("★"), parse: pick } satisfies PartShape<PickProps>,
  steps: {
    explicit: false,
    shape: (block, place) => place.afterBottomLine && ORDERED.test(block),
    parse: (block) => {
      const each = steps(block);
      return each ? { steps: each } : null;
    },
  } satisfies PartShape<StepsProps>,
} as const;

export type PartKind = keyof typeof PART_SHAPES;
export type PartProps<K extends PartKind> = NonNullable<ReturnType<(typeof PART_SHAPES)[K]["parse"]>>;

/** A block of the answer: a part with its props, or plain Markdown. */
export type AnswerBlock = { [K in PartKind]: { kind: K; props: PartProps<K>; text: string } }[PartKind] | { kind: null; text: string };
export type AnswerFold = { title: string; blocks: AnswerBlock[] };

/** An answer laid out in parts; its Markdown has its source markers linked. */
export type AnswerLayout = {
  /** The opening paragraph, drawn as a card: only when parts or folds follow it. */
  bottomLine: string | null;
  blocks: AnswerBlock[];
  folds: AnswerFold[];
  askNext: string[];
  sources: AnswerSource[] | null;
};

/** Whose answer: a Chat's, a session's, or one that keeps today's look (it changed code, a run's report). */
export type AnswerMode = "chat" | "session" | "off";

/** The answer's Markdown blocks: paragraphs apart, a fence whole, a loose numbered list one block. */
export function blocksOf(text: string): string[] {
  const blocks: string[] = [];
  let current: string[] = [];
  let fenced = false;
  const end = () => {
    if (current.length > 0) blocks.push(current.join("\n"));
    current = [];
  };
  for (const line of text.split("\n")) {
    if (/^\s*```/.test(line)) fenced = !fenced;
    if (!fenced && !line.trim() && !/^\s*```/.test(line)) {
      end();
      continue;
    }
    current.push(line);
  }
  end();
  // A numbered list with blank lines between its items is still one list.
  return blocks.reduce<string[]>((merged, block) => {
    const last = merged[merged.length - 1];
    const lastItem = last?.split("\n").filter((line) => ORDERED.test(line)).at(-1);
    const next = ORDERED.exec(block);
    if (last && lastItem && ORDERED.test(last) && next && Number(next[1]) === Number(ORDERED.exec(lastItem)![1]) + 1) {
      merged[merged.length - 1] = `${last}\n${block}`;
    } else merged.push(block);
    return merged;
  }, []);
}

function classify(block: string, place: BlockPlace): AnswerBlock {
  for (const [kind, part] of Object.entries(PART_SHAPES) as [PartKind, PartShape<unknown>][]) {
    if (!part.shape(block, place)) continue;
    const props = part.parse(block);
    if (props) return { kind, props, text: block } as AnswerBlock;
  }
  return { kind: null, text: block };
}

const HEADING = /^#{2,3}[ \t]+(.+?)[ \t#]*$/;
const ASK_NEXT = /^#{2,4}[ \t]+Ask next:?[ \t]*$/im;

/** The `### Ask next` questions (at most 3), and the text without them. */
function askNextOf(text: string): { text: string; askNext: string[] } {
  const at = text.search(ASK_NEXT);
  if (at < 0) return { text, askNext: [] };
  const after = text.slice(at).split("\n").slice(1);
  const end = after.findIndex((line) => /^#{1,6}[ \t]/.test(line));
  const lines = end < 0 ? after : after.slice(0, end);
  const askNext = lines
    .filter((line) => line.trim())
    .map((line) => line.trim().replace(BULLET, "").replace(ORDERED, "").trim());
  // Only a list of short questions is "Ask next": anything else stays as written.
  if (askNext.length === 0 || lines.some((line) => line.trim() && !BULLET.test(line.trim()) && !ORDERED.test(line.trim()))) return { text, askNext: [] };
  return { text: `${text.slice(0, at).trimEnd()}${end < 0 ? "" : `\n\n${after.slice(end).join("\n")}`}`.trimEnd(), askNext: askNext.slice(0, 3) };
}

/**
 * The folds: after a line `---` that only `###` sections follow, one fold each. A `---` with
 * anything else after it, or none at all, gives none: the text stays as written.
 */
function foldsOf(text: string): { visible: string; sections: { title: string; body: string }[] } {
  const lines = text.split("\n");
  let fenced = false;
  for (let index = 0; index < lines.length; index++) {
    if (/^\s*```/.test(lines[index]!)) fenced = !fenced;
    // After a blank line: right under a paragraph's line, `---` underlines it as a heading.
    if (fenced || !/^[ \t]*-{3,}[ \t]*$/.test(lines[index]!) || (index > 0 && lines[index - 1]!.trim())) continue;
    const rest = lines.slice(index + 1);
    const first = rest.findIndex((line) => line.trim());
    const heading = first < 0 ? null : /^(#{2,3})[ \t]/.exec(rest[first]!);
    if (!heading) continue;
    const level = heading[1]!;
    const sections: { title: string; body: string[] }[] = [];
    let inFence = false;
    for (const line of rest.slice(first)) {
      if (/^\s*```/.test(line)) inFence = !inFence;
      const title = !inFence && line.startsWith(`${level} `) ? HEADING.exec(line)?.[1] : undefined;
      if (title) sections.push({ title: title.trim(), body: [] });
      else sections[sections.length - 1]!.body.push(line);
    }
    return {
      visible: lines.slice(0, index).join("\n").trimEnd(),
      sections: sections.map((section) => ({ title: section.title, body: section.body.join("\n").trim() })),
    };
  }
  return { visible: text, sections: [] };
}

const isParagraph = (block: string) => !/^\s*(?:#|[-*+][ \t]|\d+[.)][ \t]|>|\||```|[-*_]{3,}\s*$)/.test(block);

/**
 * The answer in parts, or null when it has none: it then renders as it always has. In a session
 * a numbered list alone never lays an answer out (a coding reply has those): it takes an
 * explicit part (`> [!CAUTION]`, ```` ```chance ````, a ★ row), folds after `---`, or Ask next.
 */
export function splitAnswer(text: string, mode: AnswerMode): AnswerLayout | null {
  if (mode === "off") return null;
  const sourced = splitSources(text);
  const asked = askNextOf(sourced?.head ?? text);
  const linked = (markdown: string) => (sourced ? linkMarkers(markdown, sourced.sources) : markdown);
  const { visible, sections } = foldsOf(asked.text);
  const raw = blocksOf(visible);
  const bottomLine = raw[0] !== undefined && isParagraph(raw[0]) ? raw[0] : null;
  const blocks = raw.slice(bottomLine === null ? 0 : 1).map((block, index) =>
    classify(linked(block), { afterBottomLine: bottomLine !== null && index === 0, folded: false }),
  );
  const folds = sections.map((section) => ({
    title: section.title,
    blocks: blocksOf(section.body).map((block) => classify(linked(block), { afterBottomLine: false, folded: true })),
  }));
  const parts = [...blocks, ...folds.flatMap((fold) => fold.blocks)].filter((block) => block.kind !== null);
  const explicit = parts.some((block) => PART_SHAPES[block.kind!].explicit) || folds.length > 0 || asked.askNext.length > 0;
  if (!explicit && (mode === "session" || parts.length === 0)) return null;
  return {
    bottomLine: bottomLine === null ? null : linked(bottomLine),
    blocks,
    folds,
    askNext: asked.askNext,
    sources: sourced?.sources ?? null,
  };
}

/** Whether the opening paragraph shows as a card: only when parts or folds follow it. */
export function bottomLineCard(layout: AnswerLayout): boolean {
  return layout.bottomLine !== null && (layout.folds.length > 0 || layout.blocks.some((block) => block.kind !== null));
}

/** A block as read aloud: a part's words, not its marks. */
function said(block: AnswerBlock): string {
  switch (block.kind) {
    case "caution":
      return [block.props.title, ...block.props.items].filter(Boolean).join("\n");
    case "chance":
      return `${block.props.of} out of 100: ${block.props.caption}`;
    case "steps":
      return block.props.steps.map((step, index) => `${index + 1}. ${step.title} ${step.detail}`.trim()).join("\n");
    case "pick":
      return block.props.markdown.replace(`[My pick for you](${PICK_HREF})`, "(my pick)");
    case null:
      return block.text;
  }
}

/**
 * What "Read aloud" says of a laid-out answer: the opening and every part in view, never a fold
 * (open or not), Sources or Ask next, then "More details are folded below." when there are folds.
 */
export function spokenAnswer(layout: AnswerLayout): string {
  const text = [layout.bottomLine ?? "", ...layout.blocks.map(said)].filter(Boolean).join("\n\n");
  // A source marker is a link to its source; read aloud, it is a number in the sentence.
  const words = text.replace(/[ \t]*\[(\d+)\]\(brigadier-cite:\d+\)/g, "");
  return layout.folds.length > 0 ? `${words}\n\nMore details are folded below.` : words;
}
