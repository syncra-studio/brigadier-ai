# Third-party notices

- `apps/desktop/src/components/ui/`, `apps/desktop/src/components/assistant-ui/`, `apps/desktop/src/hooks/use-copy-to-clipboard.ts` and the token values in `apps/desktop/src/styles/tokens.css` are adapted from [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT, Copyright (c) 2025 AgentbaseAI Inc.), radix flavor, commit `3ca2fb35619661e9941d5782f4beb08c1f83d5f5`. The `error-state` and `web-search` elements in `apps/desktop/src/components/assistant-ui/elements/` are adapted from its elements registry at commit `0bdf05066fcb83a64a07d542dde4fea0e8f99b76` (MIT, Copyright (c) 2026 AgentbaseAI Inc.).
- The diffs in the thread and the Review tab are drawn by [@pierre/diffs](https://github.com/pierrecomputer/pierre) 1.5.2 with its [@pierre/theme](https://github.com/pierrecomputer/pierre) 2.0.0 colours (both Apache-2.0, Copyright 2025 Pierre Computer Company), used unchanged as npm dependencies.
- `crates/index/queries/csharp.scm` adapts the tags query from [tree-sitter-c-sharp](https://github.com/tree-sitter/tree-sitter-c-sharp) 0.23.5 (MIT).
- `crates/index/queries/ecmascript-exports.scm`, `ecmascript-constants.scm`, `typescript.scm`, `javascript.scm` and `jsx.scm` follow the doc-comment patterns of the tags query from [tree-sitter-javascript](https://github.com/tree-sitter/tree-sitter-javascript) 0.25.0 (MIT); the index also loads that query and the one from [tree-sitter-typescript](https://github.com/tree-sitter/tree-sitter-typescript) 0.23.2 (MIT) as the crates ship them.
- `crates/index/queries/swift.scm` adapts the tags query from [tree-sitter-swift](https://github.com/alex-pinkus/tree-sitter-swift) 0.7.3 (MIT).
- The Brain's embedding model, [minishlab/potion-retrieval-32M](https://huggingface.co/minishlab/potion-retrieval-32M) (MIT), revision `6fc8051fab2a1e0ee76689cf08c853792ac285e7`, is downloaded at runtime into the data folder, not shipped. `crates/brain/src/embed.rs` computes its embeddings the way [Model2Vec](https://github.com/MinishLab/model2vec) (MIT) does; no code is taken from it.
- The code rules for implement and merge workers in `crates/core/src/manager/prompts.rs` (`WORKER_CODE_RULES`) follow, partly in close wording, the rules of [ponytail](https://github.com/DietrichGebert/ponytail) (MIT, Copyright (c) 2026 DietrichGebert), commit `e3ba2aa`.
- The glyphs in `apps/desktop/src/components/icons/app-glyphs/codicons.tsx` are the SVG paths of icons from [Codicons](https://github.com/microsoft/vscode-codicons) (Copyright (c) Microsoft Corporation), licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); the paths are unchanged, wrapped as React components, except for the locally drawn SourceControlGlyph.
- The file-type glyphs in `apps/desktop/src/components/icons/file-types/fileIcons.tsx` are a subset of the icons of [Seti UI](https://github.com/jesseweed/seti-ui) (MIT, Copyright (c) 2014 Jesse Weed); their paths are kept, with the fills replaced by the theme's file colours and wrapped as React components.
- `helpers/computer-use/` is forked from [cmux-cua](https://github.com/manaflow-ai/cmux-cua) (MIT, Copyright (c) 2025 Cua AI, Inc.), tag `cmux-cua-v0.8.12`, commit `e8a8dbfd45a5d050058e7340f97ffe6f157c3128`; its README lists the changes.
- `helpers/computer-use/upstream/` is a copy of the macOS backend of [Cua Driver](https://github.com/trycua/cua) (MIT, Copyright (c) 2025 Cua AI, Inc.), tag `cua-driver-rs-v0.35.2`, commit `ad694f82aaaa12e67257e4510837fc1f6cfc9484`, changed only by the hooks in `helpers/computer-use/patches/`; `upstream/UPSTREAM.md` records the release. It contains code from [yabai](https://github.com/koekeishiya/yabai) (MIT, Copyright (c) 2019 Åsmund Vikane) and [trope-cua](https://github.com/voctory/trope-cua) (MIT, Copyright (c) 2026 Victor Vannara), and compiles in [QuickJS-ng](https://github.com/quickjs-ng/quickjs) and [rquickjs](https://github.com/DelSkayn/rquickjs) (both MIT); `upstream/THIRD_PARTY_NOTICES.md` lists each. The MIT permission notice below applies to all of them.
- `apps/desktop/src/components/icons/app-glyphs/help-icon.tsx` contains the Help SVG from [Apps SDK UI](https://github.com/openai/apps-sdk-ui), commit `0f00143c7a639906f1621fe58e1b6be7b5bea46d` (MIT, Copyright 2025 OpenAI). Its geometry is unchanged, wrapped as a local React icon with caller-controlled size and colour.

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

## Computer-use helper permission notice

Copyright (c) 2025 Cua AI, Inc.

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

## assistant-ui elements permission notice

The Activities section's task card (`apps/desktop/src/components/assistant-ui/elements/task-card.tsx`) is ported from the [assistant-ui registry](https://r.assistant-ui.com/) source [task-card](https://r.assistant-ui.com/elements-task-card.json), fetched on 2026-10-17; the port maps design tokens and icon imports, and it puts the meta under the label. The `timeline`, `job-progress`, `mcp-server-panel` and `schedule-card` elements are ported from the elements in the [assistant-ui repository](https://github.com/assistant-ui/assistant-ui) (`packages/ui/src/components/react/assistant-ui/elements/`; `timeline` and `job-progress` on 2026-10-05, `mcp-server-panel` and `schedule-card` at commit `727a02dc3fe5286ded479cb792f48acbff336cd7`, 2026-10-19): Brigadier's tokens and icons, no card frame of their own, one-line rows for the servers, a server's sign-in said rather than offered, a job's bar cut into its stages, and a schedule without its pause switch.

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

## Apps SDK UI permission notice

Copyright 2025 OpenAI

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the “Software”), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Inline visuals permission notices

The partial HTML reader, frame lifecycle, bridge checks, table rendering and drawing guide in
`apps/desktop/src/components/conversation/visuals/`, `apps/desktop/src-tauri/src/visuals_frame.html`
and `crates/core/src/manager/visuals/GUIDE.md` contain adapted MIT-licensed code and documentation
from commit `8391a94c1c3939c679e40ddd7bfc08a9ef690e70`.

The MIT License

Copyright (c) Atai Barkai

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

`apps/desktop/src/components/conversation/visuals/idiomorph.ts` contains Idiomorph v0.3.0
(https://github.com/bigskysoftware/idiomorph), with this permission notice:

BSD 2-Clause License

Copyright (c) 2022, Big Sky Software
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## Chart renderer permission notice

Charts use `recharts` 3.10.1 as an npm dependency.

The MIT License (MIT)

Copyright (c) 2015-present recharts

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
