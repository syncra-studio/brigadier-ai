# Third-party notices

- `apps/desktop/src/components/ui/`, `apps/desktop/src/components/assistant-ui/`, `apps/desktop/src/hooks/use-copy-to-clipboard.ts` and the token values in `apps/desktop/src/styles/tokens.css` are adapted from [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT, Copyright (c) 2025 AgentbaseAI Inc.), radix flavor, commit `3ca2fb35619661e9941d5782f4beb08c1f83d5f5`. The `error-state` and `web-search` elements in `apps/desktop/src/components/assistant-ui/elements/` are adapted from its elements registry at commit `0bdf05066fcb83a64a07d542dde4fea0e8f99b76` (MIT, Copyright (c) 2026 AgentbaseAI Inc.).
- `crates/index/queries/csharp.scm` adapts the tags query from [tree-sitter-c-sharp](https://github.com/tree-sitter/tree-sitter-c-sharp) 0.23.5 (MIT).
- `crates/index/queries/ecmascript-exports.scm`, `ecmascript-constants.scm`, `typescript.scm`, `javascript.scm` and `jsx.scm` follow the doc-comment patterns of the tags query from [tree-sitter-javascript](https://github.com/tree-sitter/tree-sitter-javascript) 0.25.0 (MIT); the index also loads that query and the one from [tree-sitter-typescript](https://github.com/tree-sitter/tree-sitter-typescript) 0.23.2 (MIT) as the crates ship them.
- `crates/index/queries/swift.scm` adapts the tags query from [tree-sitter-swift](https://github.com/alex-pinkus/tree-sitter-swift) 0.7.3 (MIT).
- The Brain's embedding model, [minishlab/potion-retrieval-32M](https://huggingface.co/minishlab/potion-retrieval-32M) (MIT), revision `6fc8051fab2a1e0ee76689cf08c853792ac285e7`, is downloaded at runtime into the data folder, not shipped. `crates/brain/src/embed.rs` computes its embeddings the way [Model2Vec](https://github.com/MinishLab/model2vec) (MIT) does; no code is taken from it.
- The code rules for implement and merge workers in `crates/core/src/manager/prompts.rs` (`WORKER_CODE_RULES`) follow, partly in close wording, the rules of [ponytail](https://github.com/DietrichGebert/ponytail) (MIT, Copyright (c) 2026 DietrichGebert), commit `e3ba2aa`.
- The glyphs in `apps/desktop/src/components/glyphs/codicons.tsx` are the SVG paths of icons from [Codicons](https://github.com/microsoft/vscode-codicons) (Copyright (c) Microsoft Corporation), licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); the paths are unchanged, wrapped as React components, except for the locally drawn SourceControlGlyph.
- The file-type glyphs in `apps/desktop/src/components/glyphs/fileIcons.tsx` are a subset of the icons of [Seti UI](https://github.com/jesseweed/seti-ui) (MIT, Copyright (c) 2014 Jesse Weed); their paths are kept, with the fills replaced by the theme's file colours and wrapped as React components.

## Seti UI permission notice

Copyright (c) 2014 Jesse Weed

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## assistant-ui elements permission notice

The `agent-status` and `background-inbox` elements are ported from the [assistant-ui registry](https://r.assistant-ui.com/) sources fetched on 2026-10-14: [agent-status](https://r.assistant-ui.com/elements-agent-status.json), [background-inbox](https://r.assistant-ui.com/elements-background-inbox.json). The port maps design tokens and icon imports and adds preview controls and a waiting inbox state.

MIT License

Copyright (c) 2026 AgentbaseAI Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
