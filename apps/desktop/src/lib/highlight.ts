/**
 * Syntax highlighting for the thread's code blocks, with shiki's core and its JavaScript
 * regex engine (no WebAssembly). Loaded on demand, each grammar too, so the first paint never
 * waits for it; only the common languages below ship with the app.
 */
import { createHighlighterCore, type HighlighterCore, type LanguageRegistration } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

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
    engine: createJavaScriptRegexEngine(),
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
