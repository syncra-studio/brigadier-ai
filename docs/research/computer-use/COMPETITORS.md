# Computer-use competitive analysis (as of 2026-10-26)

Goal: make Brigadier's computer use the best, fastest, most reliable and cheapest, and "cheap and not really context filling" (ledger L1).

**Method and honesty note.** Compiled from web searches and page fetches on 2026-10-26. Several pages were summarized by a fetch tool rather than read verbatim; many numbers come from secondary blogs or vendor posts. Every claim carries a URL. Tags: **[V]** vendor/primary claim, **[S]** secondary or aggregator source, **[I]** independent measurement, **[?]** could not verify / sources conflict. Nothing here was benchmarked by us. Hands-on benchmarking of our own stack is still needed before we promise numbers.

---

## 1. Landscape summary

| Product | Perception | Actions | Background (no focus steal) | Headline reliability | Token/cost signal |
|---|---|---|---|---|---|
| Anthropic computer tool | Screenshot (+ `zoom` crop) | Pixel coords, 17 member tools, batching | n/a (host-defined) | OSWorld-Verified ~85% (Fable 5, provider-reported) [S] | ~1,000–1,800 tokens/screenshot |
| Claude Cowork / Code Desktop | Screenshot, connectors first | Screen fallback | Not stated | none published | slower than connectors [V] |
| Claude in Chrome | Not found [?] | Browser | n/a | prompt-injection ASR 11.2% (Aug 2025) | n/a |
| OpenAI Codex computer use (macOS) | AX tree + screenshots (per reviewer) | Own cursor per thread, AX actions | **Yes** (parallel cursors) | GPT-5.5 78.7% OSWorld-Verified [S] | no numbers published |
| OpenAI CUA/Operator/ChatGPT agent | Screenshot | Coords | cloud browser | 38.1% OSWorld, 58.1% WebArena (Jan 2025) [S] | n/a |
| Google Gemini 2.5 Computer Use | Screenshot | 13 actions, browser | cloud/browser | Online-Mind2Web ~69–70%, ~225 s [V] | n/a |
| trycua cua-driver | AX tree + window screenshots, optional perception ext. | MCP tools, element tokens | **Yes** (SkyLight-based) | no benchmarks published | n/a |
| Peekaboo | `see` → snapshot id + element ids | click/type/set-value/menu | Yes (targeted) | none | none |
| Vercel agent-browser | A11y snapshot w/ refs (`@e2`), delta | CLI, `batch` | headless browser | none | 82–93% less context vs Playwright MCP |
| Playwright MCP / CLI | A11y snapshot | refs | headless | none | ~equal now after fix |
| Chrome DevTools MCP | A11y snapshot + screenshot | 26–29 tools, `--slim`=3 | n/a | none | tool defs 4.5–18.5K tokens |
| browser-use | DOM+screenshot | Python agent | n/a | BU Bench V1 63–78% | $0.17/solved task (internal) |
| Stagehand | DOM/AX | act/observe/extract | n/a | none | cache hits = 0 LLM tokens |
| Simular Agent S3 | Screenshot + grounding | compositional | n/a | 62.6% → 69.9% (bBoN) OSWorld | many parallel rollouts |
| UI-TARS | Screenshot, end-to-end model | coords | n/a | 1.5: 42.5% OSWorld, 61.6 ScreenSpot-Pro | small local models |
| Cursor Computer Use | Screenshots + AX (helper app) | click/type | not stated | none | none |

---

## 2. Per-competitor detail

### 2.1 Anthropic

**Computer use tool (API).** Current tool is `computer_toolset_20260801` (GA; required for Claude 5.5+); prior `computer_20251124` needs a beta header. It has 17 member tools including `screenshot`, `zoom` (region → full-resolution crop), clicks, drag, `type`, `key`, `scroll`, `wait`. Claude may return **several actions per turn**; they run in order and stop at first failure, remaining ones get `is_error`. Docs advise attaching a screenshot to the last result of a batch to save a round trip. [V] https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool
- Perception/limits: 2,576 px long edge / 4,784 visual tokens for newest models (tokens = ⌈w/28⌉×⌈h/28⌉); 1,568 px / ~1.15 MP earlier. Recommended 1024×768 or 1280×720 (web 1280×800); avoid >1080p. Each screenshot "roughly 1,000–1,800 input tokens". >20 images in a request tightens per-side limit to 2,000 px. [V] same URL.
- Latency/cost advice from the docs: prompt-cache after system+tools; prune old screenshots in batches (e.g. keep last 3, prune every 25 turns) so the cache prefix stays stable; `disable_parallel_tool_use` if the loop can't batch. [V] same URL.
- Reliability: Opus 4.6 system card reports 72.7% OSWorld-Verified (avg of 5 runs) [V] https://www-cdn.anthropic.com/6a5fa276ac68b9aeb0c8b6af5fa36326e0e166dd.pdf. Opus 5.5 "81.8% on OSWorld 2.0" is a partial-credit figure from Anthropic's report; another board shows Opus 5 (max) at 31.4% on 2.0 full completion [S][?] https://aiweekly.co/alerts/anthropic-ships-claude-opus-55-at-20-lower-token-prices , https://www.benchleader.com/benchmarks/osworld_2 . Opus 5.5 price $4/$20 per MTok, cache reads $0.20 [S] (same aiweekly URL).
- Per-tool schema overhead: **not stated** in the fetched page [?].

**Claude Cowork / Claude Code Desktop computer use.** Research preview Mar 2026 (macOS), Windows added Apr 2026; falls back to screen operation when no connector fits; "slower than connectors", struggles with complex multi-step workflows. [V] https://claude.com/blog/dispatch-and-computer-use , https://support.claude.com/en/articles/14128542-let-claude-use-your-computer-in-cowork , https://x.com/claudeai/status/2039836891508261106. Whether it uses background input: not stated [?].

**Claude in Chrome.** Aug 2025 pilot: 123 adversarial cases / 29 scenarios; attack success 23.6% without mitigations, 11.2% with; computer use ASR 19.4% [V/S] https://claude.com/blog/claude-for-chrome , https://www.theregister.com/2025/08/26/anthropic_claude_chrome_warnings/ . Internals (DOM vs a11y tree vs screenshot) not found [?]. Newer numbers may exist.

### 2.2 OpenAI

- **Operator/CUA (Jan 2025):** 38.1% OSWorld, 58.1% WebArena, 87% WebVoyager [S] https://www.convergence-now.com/artificial-intelligence/test-scores-of-chatgpts-all-new-computer-using-agent-operator-might-blow-your-minds-119000/ . **ChatGPT agent:** WebArena 65.4%, BrowseComp 68.9% [S] https://www.neowin.net/news/openai-introduces-chatgpt-agent-that-can-complete-tasks-using-its-own-computer/ .
- **Sky acquisition (2025-10-23):** Mac-native interface joined OpenAI. [V] https://openai.com/index/openai-acquires-software-applications-incorporated/
- **Codex computer use (macOS, 2026-04-16), based on Sky.** "Sees, clicks, types with its own cursor, runs in the background without taking over your computer." [V] https://x.com/OpenAI/status/2044827932145897652 . Plugin in the Codex desktop app; needs Screen Recording + Accessibility + per-app approval; macOS only; not in EEA/UK/CH; not available via `codex exec`; cannot automate terminal apps; two agents on one app conflict; locked Mac fails silently. [S] https://codex.danielvaughan.com/2026/04/17/codex-app-computer-use-macos-background-gui-automation/ . 
- Hands-on (MacStories): reads each app's **AX tree** rather than mainly screenshots; tree is verbose, sometimes ~20 levels deep; screenshots/coordinate clicks/AppleScript as fallbacks; parallel virtual cursors; faster than the earlier Sky (which ran on Claude); Fast mode / Cerebras-hosted GPT-5.3-Codex-Spark "dramatically faster"; still slower than a skilled human. [I, single tester] https://www.macstories.net/notes/openais-new-codex-app-has-the-best-computer-use-feature-ive-ever-tested/
- Claim that it uses private SkyLight APIs: unverified [?] (one third-party guide only).
- Latency per step, tokens per step: **not published** [?].
- OSWorld-Verified: GPT-5.5 78.7%, GPT-5.4 75% [S] https://llmlearner.com/rankings/osworld-verified .

### 2.3 Google

- **Gemini 2.5 Computer Use (2025-10):** 13 actions, browser-focused (not OS-level); Google chart: Online-Mind2Web ≥70% at ~225 s latency on Browserbase harness, "lowest latency" among compared. Other summaries give 69.0% Online-Mind2Web, 88.9% WebVoyager, 69.7% AndroidWorld. [V/S] https://blog.google/innovation-and-ai/models-and-research/google-deepmind/gemini-computer-use-model/ , https://siliconangle.com/2025/10/07/googles-gemini-2-5-computer-use-model-can-navigate-web-like-human/ , https://www.marktechpost.com/2025/10/10/what-are-computer-use-agents-from-web-to-os-a-technical-explainer/
- **Project Mariner:** one wiki says it was folded into Gemini Agent / Chrome auto browse on 2026-05-04 [?, single low-quality source] https://aiwiki.ai/wiki/project_mariner . Gemini 3.x built-in computer use: unconfirmed [?].
- Note ~225 s per task is the best *browser* number Google chose to publicise; it's a useful reference for "fast".

### 2.4 trycua/cua and cua-driver (our upstream)

- README: background driver, MCP over stdio, no focus stealing; macOS/Windows/Linux (X11, Sway, GNOME, KDE); in-process runtimes for Python/TS via UniFFI/C ABI; permission modes `standard`/`bounded`/`unrestricted`. Optional separately-installed `cua-perception` extension parses a retained screenshot into text+icon regions; region-derived pixel clicks need a one-use `capture_id`; its icon detector is AGPL-3.0 (licence trap). No benchmarks or token claims on the page. [V] https://github.com/trycua/cua/tree/main/libs/cua-driver
- v0.36.0 theme: macOS background reliability (background typing into LibreOffice, background drag/menu commands, reading pages on other Spaces, no focus bounce, no Chrome raise); first read of a background Chrome window now includes page content; `health_report` windowserver check; `launch_app` resolves to real window; autoreleased objects drained on capture workers. No token/snapshot/perf changes. [V] https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.36.0
- **After v0.36 (releases page shows only nightlies, newest `v0.38.1-nightly.20261026`; no stable after 0.36 visible [?]):** a changelog spanning from v0.30.5 reports: element tokens now resolve against a snapshot store **invalidated on read**; token tool schemas constrained; documented pixel budget and "image-blind verification" paths; explicit background-drag refusal on macOS; `list_apps` lists every windowed instance of a bundle; `list_windows` omits AX-less helper windows; `verify_state` reads label-less display text; "settled the front app before foreground hotkey checks"; many Windows/Linux fixes (UIA timeouts as warnings, Hyprland, X11 modifiers); 10-26 nightly mostly dead-code removal. [V, via fetch summary; the 25th-nightly tag page itself 404'd so read via releases index] https://github.com/trycua/cua/releases
- Other upstream signals: Cua Driver open-sourced Apr 2026 [V] https://x.com/trycua/status/2047383200348221632 ; "jev-use" (Cua Driver + typesafeai Jev, "fast computer use is solved", dev preview) – page returned HTTP 402, **content unverified** [?] https://x.com/trycua/status/2100649543079502213 ; "Computer History" preview tools `history_status/history_query` in README.
- Internals write-up of 0.26.1 (pinning window target; separating action facts from postconditions): https://codenote.net/en/posts/cua-driver-computer-use-internals-code-reading-v0-26-1/ [S, not read in full].

### 2.5 Peekaboo (steipete)

`see` returns a structured UI map with opaque element IDs and a snapshot id; later `click/set-value/...` reference it without recapture; Agent/MCP policy requires a fresh exact non-dialog snapshot for background-only; semantic actions (menu click) preferred; targeted input delivered in the background when process resolves; raw chords need exact window selector or foreground consent; latest 4.9.7; `peekaboo agent "<task>"` built-in loop. No perf numbers. [V] https://github.com/steipete/Peekaboo (redirected to openclaw/Peekaboo) ; aggregator: https://www.typingmind.com/mcp/peekaboo-macos-screen-capture [S]. MCP tool count not stated [?].

### 2.6 macOS-use / mcp-server-macos-use

Swift MCP server on the AX API (MacosUseSDK); each action tool returns the refreshed accessibility tree; separate refresh tool; traversal options include `diff`, visibility, animation, delay. What `diff` returns is **unverified** [?]. [S] https://mcp.so/servers/mcp-server-macos-use?tab=tools , https://glama.ai/mcp/servers/mcacvucf1s

### 2.7 Simular Agent S3

62.6% OSWorld alone (100 steps), 69.9% with Behavior Best-of-N (parallel rollouts + judge); WindowsAgentArena 50.2→56.6, AndroidWorld 68.1→71.6; paper arXiv 2510.02250 (Oct 2025). One third-party guide conflicts (66%) [?]. Cost implication: bBoN multiplies compute per task. [V/S] https://digirise.ai/chaen-ai-lab/simular-ai-agent-s3/ , https://habr.com/ru/post/953246 , https://www.verdent.ai/guides/agent-s-vs-agent-s2-vs-agent-s3

### 2.8 UI-TARS (ByteDance)

End-to-end vision model. UI-TARS-1.5: OSWorld 42.5 (7B: 27.5), ScreenSpot-Pro 61.6 (7B: 49.6). UI-TARS-2: OSWorld 47.5% (step budget unverified). Relevant as a *local grounding* option. [S] https://lilting.ch/en/articles/ui-tars-gui-agent , https://codesota.com/model/ui-tars-2

### 2.9 Perplexity Comet, Manus

- Comet: Chromium browser with agent; indirect prompt injection (Aug 2025) and screenshot-hidden-text injection (Oct 2025) reported by Brave; patch status disputed. Perception method: **not found** [?]. [S] https://www.theregister.com/2025/08/20/perplexity_comet_browser_prompt_injection/ , https://decrypt.co/336763/perplexity-comet-flaw-exposed-user-data-attackers-brave-reports
- Manus: **nothing verifiable found** [?].

### 2.10 browser-use

BU Bench V1 (100 tasks): Browser Use Cloud bu-ultra 78.0%, OSS + ChatBrowserUse-2 63.3%, claude-opus-4-6 62.0%; bu-ultra ~14 tasks/hour, fewest steps though slower per step; bu-2-0 priced $0.60/$0.06 cached/$3.50 per MTok; "$0.17 per solved task" on Internal Bench Hard (82%) – self-reported. [V] https://browser-use.com/posts/ai-browser-agent-benchmark , https://browser-use.com/changelog/27-1-2026 , https://browser-use.com/benchmarks/agents . Opus 4.8 74% / Gemini 3.6 Flash 68% [S, secondhand] https://piunikaweb.com/2026/07/22/gemini-3-6-flash-browser-automation-benchmark/

### 2.11 Stagehand (Browserbase)

`act/observe/extract`; v3 server-side cache on Browserbase: repeated `act` with same inputs "instantly, without consuming LLM tokens"; local file cache via `cacheDir`; docs estimate ~1–3 s uncached vs ~150–300 ms cached [V, estimate not measurement]. v4 extends to observe/extract; cache keys exclude model. https://docs.stagehand.dev/v3/best-practices/caching.md , https://docs.stagehand.dev/v4/best-practices/caching , https://docs.stagehand.dev/v2/best-practices/speed-optimization

### 2.12 Playwright MCP / CLI

Old numbers: ~114K tokens/task via MCP vs 27K via CLI [S] https://morphllm.com/playwright-mcp . Re-tests on newer versions: MCP now writes snapshots to disk and returns a short path; within ~1% [I-ish single task] and Checkly 48–50K (MCP) vs 45–48K (CLI) tokens on a shop-cart flow [I] https://dev.to/aswani25/playwright-cli-vs-playwright-mcp-which-should-you-use-with-claude-code-1olh , https://www.checklyhq.com/blog/mcp-vs-cli-token-efficiency/ . Lesson: snapshot-to-file + on-demand read erased most of the gap.

### 2.13 Chrome DevTools MCP

26–29 tools; tool-definition cost 4,475 tokens (tiktoken, 29 tools; `take_snapshot` 167) per PolicyLayer vs ~18.6K for 26 tools in an older v0.12.1 benchmark; one real page `take_snapshot` ≈ 10,220 tokens; `--slim` = 3 tools (navigate, evaluate, screenshot). [S] https://policylayer.com/token-cost/io-github-chromedevtools-chrome-devtools-mcp , https://www.heyuan110.com/posts/ai/2026-07-21-claude-code-screenshot-mcp-frontend-debugging/ . Figures conflict by method/version [?].

### 2.14 Vercel agent-browser

Native Rust CLI. `snapshot` = a11y tree with refs (`@e2`); `-i` interactive only, `-c` compact, `-d` depth, `-s` selector scope; **refs survive across snapshots for surviving elements**, re-resolve only if exactly one role+name match; overlay-blocked clicks report the covering element; `snapshot --delta` returns state once then `unchanged`/incremental updates; `diff snapshot`/`diff screenshot`; `screenshot --if-changed`; `--max-output` cap; `batch` with `--bail`; `read` fetches markdown without launching Chrome. [V] https://github.com/vercel-labs/agent-browser . Token claims: Vercel "93% less context vs Playwright MCP" (vendor; compares full tree dumps to ref output); Pulumi test: 82% less across six tests (~5.5K vs ~31K chars) [I]; 200–400 tokens/snapshot typical is an estimate, Bright Data measured ~1,400 vs 7–8K on a page [S]. https://www.pulumi.com/blog/self-verifying-ai-agents-vercels-agent-browser-in-the-ralph-wiggum-loop/ , https://brightdata.com/blog/ai/browser-api-vs-vercel-agent-browser , https://paddo.dev/blog/agent-browser-context-efficiency/ . Trade-off: "token-cheap, time-expensive" (more round trips) per one review.

### 2.15 Competing agent apps

- **Cursor:** Computer Use announced 2026-10-23 (3 days ago): agents drive desktop apps (e.g. iOS Simulator); on macOS needs Accessibility + Screen Recording, granted to a **separate helper app** "Cursor Computer Use"; Cloud Agents/self-hosted workers already had it (macOS+Linux). No perf numbers. [V/S] https://x.com/cursor_ai/status/2113661506634305834 , https://cursor.com/docs/cloud-agent/self-hosted/computer-use , https://dev.to/gabrielanhaia/cursors-agent-can-now-click-through-your-desktop-apps-grant-access-to-the-helper-only-1og3
- **Conductor, Superset, Orca, t3code, bb:** no computer-use feature found. Orca has a per-worktree Chromium design mode; Superset lists a built-in browser and MCP server/SDK. Sources are Superset's own comparison pages (biased) [S][?] https://superset.sh/compare/orca-vs-conductor , https://superset.sh/compare/best-agent-development-environment . t3code and bb: nothing substantive found [?].

---

## 3. What makes computer use good

Evidence-backed techniques, with numbers. Quality tag on each.

1. **Compact text snapshots with stable refs instead of screenshots/full trees.** agent-browser: 82% fewer chars than Playwright MCP in a six-test independent run [I]; vendor claims 93% [V]. Codex reportedly reads the AX tree first and screenshots as fallback, and was faster than prior Sky [I, one tester]. Beware: raw AX trees can be ~20 levels deep and are themselves expensive, so compact + scope them (`-i`, `-c`, depth, selector). 
2. **Snapshot deltas / return `unchanged`.** `snapshot --delta`, `diff snapshot`, `screenshot --if-changed` (agent-browser) [V]; mcp-server-macos-use has a `diff` option [?]. No published token delta numbers [?] — we must measure.
3. **Snapshot to file/handle, return a short pointer + summary.** Playwright MCP's change erased a ~4x token gap vs CLI (114K→~equal) [S/I]. cua-driver's "element tokens resolved against a snapshot store invalidated on read" is the same idea on our upstream [V].
4. **Small tool surface and progressive disclosure.** Tool Search Tool: ~55K → ~8.7K tokens of definitions (−85%); MCP eval accuracy 49→74% (Opus 4), 79.5→88.1% (Opus 4.5) [V via secondary] https://anthropic.com/engineering/advanced-tool-use . Code-execution-with-MCP: 150K→2K in Anthropic's example [S, unverified against original] https://research.aimultiple.com/code-execution-with-mcp . Chrome DevTools MCP full set 4.5–18.5K tokens vs `--slim` 3 tools [S]. CLI-shaped tools (no schema) avoid the cost entirely [S].
5. **Batched multi-step actions.** Anthropic's tool natively batches with stop-on-first-failure and a screenshot on the last result [V]; agent-browser `batch --bail` [V]. Fewer LLM round trips is the main latency lever, since the model call dominates per-step time (inference; no vendor publishes split [?]).
6. **Right-sized and zoomable screenshots.** Anthropic recommends 1024×768–1280×800, avoid >1080p, ~1,000–1,800 tokens each, `zoom` for full-resolution crops [V]. ScreenSpot-Pro: iterative crop lifted OS-Atlas-7B 18.9→48.1% [V paper] https://arxiv.org/abs/2504.07981 ; but a simple two-pass zoom added only ~1.3 pts in one repo [S] https://github.com/deskmind-ai/eyes . Prune old screenshots in batches (keep last 3, prune every 25 turns) so cache prefix stays valid [V].
7. **Prompt caching with a stable prefix.** Cache reads $0.20/MTok vs $4 input (Opus 5.5) = 20× cheaper [S]. Order: system → tools → history; never rewrite earlier turns each step.
8. **Background input (no focus theft) + parallel cursors.** Codex and cua-driver and Peekaboo all converge here; it is what makes agents usable alongside the user [V/S]. Costs: Electron drag refusals, per-app quirks (cua 0.36 notes) [V].
9. **Action verification / postconditions.** Pixel clicks gated by one-use `capture_id` (cua) and `verify_state` (cua nightly); Peekaboo requires fresh snapshot for background-only; agent-browser reports the covering element on blocked clicks [V]. Pairs with returning state only on request. No quantified success-rate gain published [?].
10. **Semantic actions before coordinates.** Peekaboo prefers `menu click`, `set-value`; Codex prefers dedicated plugins over GUI [V/S]. Skip pixel grounding whenever an AX action or connector exists (Anthropic Cowork also prefers connectors; "slower than connectors") [V].
11. **Cache/replay learned trajectories.** Stagehand cache hits cost 0 LLM tokens, docs estimate 1–3 s → 150–300 ms [V estimate]. SkillDroid (mobile): replay 100% success over 79 rounds, 2.4× faster than LLM execution, 49% fewer LLM calls overall, 85.3% success (+23 pts vs stateless) [paper abstract via secondary; full tables unverified] https://arxiv.org/pdf/2604.14872 . Not yet shown on desktop macOS [?].
12. **Best-of-N / judge for hard tasks (expensive).** Agent S3 62.6→69.9% OSWorld at several× compute [V]; not a cost-reduction technique.
13. **Injection hygiene.** Screenshot text and DOM are attacker-controlled: Claude in Chrome 23.6%→11.2% ASR; Comet screenshot-text injection [V/S]. Matters for reliability claims.

---

## 4. Top 10 for Brigadier, ranked by expected win

(Expected win = tokens + latency + success, for a macOS background-input driver on cua-driver. All estimates below are hypotheses until we measure.)

1. **Return state only on request; actions return a tiny ack + postcondition, not a tree/screenshot.** Biggest context saver (each full AX tree can be thousands of tokens; Chrome `take_snapshot` ≈ 10K [S]). Evidence: Playwright MCP's fix [S/I]; cua snapshot store [V]; Anthropic advice to attach one screenshot only to the last action of a batch [V].
2. **Compact, scoped AX snapshots with short stable element refs** (interactive-only, depth cap, window/subtree scope, text-compressed). Evidence: agent-browser 82–93% [I/V]; Codex's 20-level trees as the anti-pattern [I].
3. **Snapshot deltas (`unchanged` / changed subtree) keyed on a snapshot id.** Evidence: agent-browser `--delta` [V]; macos-use `diff` [?]. Likely large on multi-step loops; must measure.
4. **Small tool set + deferred loading (tool search) or a CLI-shaped surface.** −85% definition tokens, accuracy +9–25 pts on MCP evals [V via S]; Chrome DevTools slim [S]. Cheap to do: fixed cost on every request.
5. **Batch multiple actions per turn with stop-on-failure; one verification read at the end.** Native in Anthropic tool; agent-browser `batch` [V]. Cuts model round trips, the dominant latency.
6. **Prompt-cache-friendly history: stable prefix, prune old screenshots/snapshots in blocks** (keep last 3, prune every 25 turns). 20× cheaper cache reads [V/S].
7. **Prefer semantic AX actions (press, set-value, menu) over coordinate clicks; screenshots only as fallback, downscaled to ≈1280×800 with `zoom` crops.** Evidence: Codex AX-first [I], Peekaboo [V], Anthropic sizing/zoom [V], ScreenSpot-Pro crop gains [V paper].
8. **Post-action verification with failure hints** (what covered the target, whether focus/window changed; `capture_id`-style staleness guards). Evidence: agent-browser overlay report, cua `capture_id`/`verify_state` [V]. Raises reliability; no published success-rate numbers [?].
9. **Trajectory/skill cache: record successful runs, replay deterministically with ref re-resolution, fall back to LLM on mismatch.** Evidence: Stagehand cache (0 tokens) [V], SkillDroid 2.4× / −49% LLM calls [paper, mobile] — highest ceiling for repeated tasks, highest build cost.
10. **Background, non-focus-stealing input and per-thread cursors as the default, with explicit refusal codes** (already our upstream's strength; Codex made it the headline) [V/S]. Differentiator for "doesn't disturb the user", not a token saver.

**Not recommended now:** best-of-N rollouts (compute ×N), local UI-TARS grounding (AGPL/licence and quality gap, 27.5–47.5% OSWorld), cloud-browser caches that don't apply to native apps.

## 5. Gaps / to verify next
- No competitor publishes per-step latency or tokens/step for desktop use; we need our own benchmark (steps, tokens, wall-clock) on a fixed macOS task set vs cua-driver baseline.
- Confirm with real tool listings: Anthropic computer tool per-tool schema tokens; what `diff` in mcp-server-macos-use returns; the "jev-use" claim (HTTP 402 page).
- Claude in Chrome / Comet / Manus perception internals: not found.
- t3code, bb, Conductor, Superset, Orca computer use: nothing found; vendor comparison pages are biased.
- OSWorld numbers differ wildly by source/variant (Verified vs 2.0, partial credit, harness): treat leaderboard values from aggregators as indicative only. https://llm-stats.com/benchmarks/osworld-verified , https://presenc.ai/research/osworld-computer-use-leaderboard-2026
