/**
 * Syntax highlighting for the thread's code blocks and file tabs, with shiki's core and its
 * WebAssembly regex engine: in WebKit the JavaScript one took ~1.5 s to colour a first file and
 * ~120 ms each one after, this one ~0.2 s and ~15 ms. Loaded on demand, each grammar too, so the
 * first paint never waits for it, and warmed at idle (`warmHighlight`); only the common
 * languages below ship with the app.
 */
import { createHighlighterCore, type HighlighterCore, type LanguageRegistration } from "shiki/core";
import { createOnigurumaEngine } from "shiki/engine/oniguruma";

import type { Token } from "@/lib/languages";

export { HIGHLIGHT_CHARS, languageOf, type Token } from "@/lib/languages";

type Grammar = () => Promise<{ default: LanguageRegistration[] }>;

const GRAMMARS: Record<string, Grammar> = {
  bash: () => import("shiki/langs/bash.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  dart: () => import("shiki/langs/dart.mjs"),
  dockerfile: () => import("shiki/langs/dockerfile.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  graphql: () => import("shiki/langs/graphql.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  ini: () => import("shiki/langs/ini.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  kotlin: () => import("shiki/langs/kotlin.mjs"),
  lua: () => import("shiki/langs/lua.mjs"),
  make: () => import("shiki/langs/make.mjs"),
  php: () => import("shiki/langs/php.mjs"),
  powershell: () => import("shiki/langs/powershell.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  ruby: () => import("shiki/langs/ruby.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  swift: () => import("shiki/langs/swift.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  vue: () => import("shiki/langs/vue.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
};

const ALIASES: Record<string, string> = {
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  console: "bash",
  "c++": "cpp",
  cs: "csharp",
  docker: "dockerfile",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  ts: "typescript",
  mts: "typescript",
  py: "python",
  rb: "ruby",
  rs: "rust",
  kt: "kotlin",
  yml: "yaml",
  makefile: "make",
  ps1: "powershell",
  svg: "xml",
};

let highlighter: Promise<HighlighterCore> | null = null;

function core(): Promise<HighlighterCore> {
  highlighter ??= createHighlighterCore({
    themes: [import("shiki/themes/github-dark-default.mjs")],
    langs: [],
    engine: createOnigurumaEngine(import("shiki/wasm")),
  });
  return highlighter;
}

/** shiki's FontStyle.Italic bit. */
const ITALIC = 1;

/** The lines of `code` as coloured tokens, or null for a language without a grammar here. */
export async function highlight(code: string, language: string): Promise<Token[][] | null> {
  const key = language.toLowerCase();
  const lang = ALIASES[key] ?? key;
  const grammar = GRAMMARS[lang];
  if (!grammar) return null;
  const shiki = await core();
  if (!shiki.getLoadedLanguages().includes(lang)) await shiki.loadLanguage(grammar());
  return shiki
    .codeToTokensBase(code, { lang, theme: "github-dark-default" })
    .map((line) =>
      line.map((token) => ({
        content: token.content,
        color: token.color,
        italic: ((token.fontStyle ?? 0) & ITALIC) !== 0,
      })),
    );
}

/** The languages most code here is in: warmed at idle, so their files show colours with their text. */
export const COMMON_LANGUAGES = ["typescript", "tsx", "javascript", "json", "rust", "python", "css", "html", "bash", "toml", "yaml"] as const;

/**
 * A few lines of TypeScript with the constructs most files use. The regex engine compiles each
 * of a grammar's patterns the first time a line needs it: coloured at idle, these leave a first
 * file little to compile (~130 ms → ~50 ms for a 300-line one).
 */
export const WARM_SAMPLE = `import { type FC, useEffect } from "react";
// A line comment
/** A doc comment with {@link Thing}. */
export interface Props<T extends object = {}> { readonly id: string; items?: T[]; onPick: (item: T, index: number) => void }
type Pair = [left: number, right: string] | null;
enum Kind { One = 1, Two = "two" }
export class Store<T> extends Base implements Thing {
  private static count = 0;
  #secret = 0x1f;
  constructor(public readonly name: string) { super(); }
  async load(path: string): Promise<T | undefined> {
    const found = await fetch(\`/api/\${encodeURIComponent(path)}?q=\${1 + 2.5e3}\`);
    if (!found.ok) throw new Error("not found: " + path);
    return (await found.json()) as T;
  }
  get size(): number { return this.items?.length ?? 0; }
}
export const Card: FC<Props<{ a: 1 }>> = ({ id, items = [], onPick }) => {
  useEffect(() => { const timer = setTimeout(() => void 0, 10); return () => clearTimeout(timer); }, []);
  const words = items.filter((item, n) => n % 2 === 0 && /^[a-z]+$/i.test(String(item))).map(({ a }) => a);
  return (
    <div data-slot="card" className={cn("flex", id && "gap-1")} onClick={() => onPick(items[0]!, 0)}>
      {words.length > 0 ? <span title={\`\${words.length} words\`}>{words.join(", ")}</span> : null}
    </div>
  );
};
for (let i = 0; i < 3; i++) { switch (typeof i) { case "number": break; default: continue; } }
export default Card satisfies unknown;
`;

/** `WARM_SAMPLE` in its top-level statements: each colours apart, so warming never holds the page long. */
export const WARM_PIECES = WARM_SAMPLE.split(/\n(?=[^\s}])/);

/** Hands the page back for a moment, so input between two pieces of warming isn't held. */
export const yieldToPage = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** Loads the highlighter and each common grammar ahead of the first file; TypeScript's patterns compile on a sample. */
export async function warmHighlight(): Promise<void> {
  for (const language of COMMON_LANGUAGES) {
    for (const piece of language === "typescript" || language === "tsx" ? WARM_PIECES : ["x"]) {
      await highlight(piece, language);
      await yieldToPage();
    }
  }
}
