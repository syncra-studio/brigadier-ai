# Building the Fastest, Most Precise Computer-Use Environment for LLM Agents (State of the Art, September 2026)

The fastest and most precise computer-use setup in 2026 does not rely on pure pixels. It is a layered hybrid. The agent uses code, APIs and CLIs first. For web pages it uses structured page state (DOM or accessibility refs). It clicks screenshot pixel coordinates only as the last resort, and when it does, it uses a correctly scaled screenshot and a zoom tool. All of this runs inside a snapshot-able VM or microVM with a fast, native capture path, and the harness batches actions, detects when the UI has settled and verifies results. Benchmark evidence points the same way. Top OSWorld systems now beat the 72.4% human baseline mainly through harness engineering (coding actions, batching, zoom, best-of-N selection), not better clicking alone. Profiling studies show that time goes mostly to large model calls and wasted steps, not screen capture.

## TL;DR

- **Architecture:** Use an API/code-first, GUI-last hybrid. A strong planner model (Claude, GPT-5.x, Gemini or Qwen class) drives a harness that offers bash/Python, DOM or accessibility-tree actions, and pixel actions with zoom. On the desktop, fuse the accessibility tree (UIA, AX or AT-SPI) with vision parsing. Systems built this way (CoAct-1, Agent S3, UFO2, UI-TARS-2) gain both success rate and speed.
- **Precision:**
  - The single biggest click-accuracy fix is sizing screenshots to the model's image limits yourself and mapping coordinates back exactly. For Claude, start at 1280x720 (1080p on Opus 4.7+). For OpenAI, use `detail: "original"` or downscale to about 1440x900. Gemini returns coordinates on a normalized 0–999 grid.
  - Add zoom or crop-and-refine for small targets. ScreenSpot-Pro results show that shrinking the search area is the most reliable grounding gain.
- **Speed:**
  - Cut model calls, not capture time. OSWorld-Human (WukLab) found that planning alone takes more than half, sometimes close to 75%, of task latency, and that even top agents take 1.4–2.7x more steps than necessary.
  - Batch several actions per turn, and verify them cheaply on the harness side (UIA/DOM checks) instead of with extra LLM calls.
  - Use prompt caching with batched screenshot pruning, and skip screenshots when DOM text is enough. Browser Use measured that each screenshot adds about 0.8 s of inference latency. Replace fixed sleeps with settle detection.

## Key Findings

1. **The frontier moved from "better clicking" to "better harnesses."**
   - OSWorld-Verified leaderboards (as of September 2026) are led by Qwen3.8 Max at 86.1%, followed by Claude Fable 5 and Claude Mythos 5 at about 85%. These are mostly self-reported. llm-stats counts 24 models with 0 independently verified results.
   - Independently verified harness results include Pointer at 83.6% with Claude Opus 4.7.
   - Simular's Agent S3 with Behavior Best-of-N (bBoN) was the first to pass the human baseline, at 72.6%.
   - Scaffolds matter as much as models. Agent S3 with GPT-5 moves from 65.6% to 69.9% just by switching from single-shot to best-of-10.
2. **Coding and API actions are the biggest efficiency lever.**
   - CoAct-1 (Salesforce) reached 60.76% on OSWorld with an average of 10.15 steps, versus 15 for agents like GTA-1.
   - Agent S3 replaced hierarchical planning with a coding agent. Compared with Agent S2, it got 13.8% better success, 52.3% fewer LLM calls and 62.4% less completion time.
   - Microsoft's UFO2 paper reports that native-API actions improve completion rate by over 8%.
3. **Latency is dominated by LLM calls and step count.**
   - OSWorld-Human profiled Agent S2 and GTA1. Planning took more than half, and up to about 75%, of task latency. Reflection (S2) took 33.6% and judging (GTA1) took 22.5%.
   - Later steps took up to 3x longer than early ones as context grew.
   - 23% of GTA1's errors came from poor visual grounding, sometimes costing up to 30 extra steps.
4. **Structured observations beat pixels on the web; vision is still needed on the desktop.**
   - Browser Use's self-reported "Speed Matters" benchmark gives 68 s average per Online-Mind2Web task (3 s per step with its strongest model), versus 330 s for OpenAI's Computer-Using Model, 285 s for Claude Sonnet 4.5 and 225 s for Gemini 2.5 CU. Browser Use gets this by navigating mainly through DOM text. It also measured that each screenshot adds about 0.8 s of inference latency while the image encoder processes it.
   - On desktops, accessibility trees miss custom-rendered widgets. That is why UFO2 fuses Windows UIA with OmniParser-v2 detection.
5. **Grounding precision comes from resolution discipline and search-area reduction.**
   - When ScreenSpot-Pro launched, the best specialized grounder scored only 18.9%. Planner-guided cascaded search (ScreenSeekeR) reached 48.1% with no extra training.
   - OmniParser v2 plus GPT-4o scores 39.6%, versus 0.8% for GPT-4o alone. GTA1-7B scores 50.1%.
   - Frontier general models now report ScreenSpot-Pro scores in the 0.75–0.85 range, for example Qwen3.8 Max at 0.845 on llm-stats (self-reported).
6. **Security is a first-class design constraint.** In "Mitigating the risk of prompt injections in browser use," Anthropic reports that Claude Opus 4.5 cut browser-use prompt-injection attack success to 1% against an adaptive Best-of-N attacker with 100 attempts per environment. The same post says a 1% rate "still represents meaningful risk" and that "no browser agent is immune," so isolation and human confirmation are still required.

## Details

### 1. How today's computer-use systems are built

| System | Observation | Action space | Notable design points | Known weaknesses |
|---|---|---|---|---|
| **Anthropic Claude computer use** (`computer_toolset_20260801`; earlier `computer_20251124`) | Screenshots you capture and return; `zoom` region images at full resolution | 17 member tools: screenshot, zoom, left/right/middle/double/triple click, drag, mouse down/up, mouse_move, cursor_position, scroll, type, key (with repeat), hold_key, wait | Batch actions (several `tool_use` blocks per turn, run in order, stop at first failure). Coordinates are always in screenshot pixel space. Reference setup is Docker with Xvfb, Mutter, Tint2 and xdotool. Built-in prompt-injection classifiers | Clicks miss if screenshots exceed image limits or scaling is wrong. Small targets (checkboxes, tray icons) are hard. The toolset definition costs about 4,500 input tokens |
| **OpenAI computer tool** (Responses API; GA successor to `computer-use-preview`/Operator CUA) | Screenshots with `detail: "original"` | `computer_call` with batched `actions[]` (click, double_click, drag, keypress, scroll, type, etc.) | OpenAI now recommends starting with a **code-execution harness** (the model writes Playwright/PyAutoGUI code) and treats the structured computer tool as the alternative. The Agents SDK supports `needsApproval` per action | Conversation state and environment state are separate. Resuming a response does not restore browser or login state |
| **Google Gemini computer use** (gemini-2.5-computer-use-preview; `gemini-3.5-flash` now recommended per Browserless docs) | Screenshots | Browser-centric function calls (click_at, type_text_at, scroll_document, navigate, go_back, drag_and_drop, key_combination, etc.) on a **normalized 0–999 grid** | Per-step `safety_decision` (allowed / require_confirmation / blocked). Gemini 3.x adds an `intent` field | Gemini 2.5 CU was "not yet optimized for desktop OS-level control" |
| **Microsoft UFO2** | Windows UIA tree fused with OmniParser-v2 vision detection | GUI actions plus native APIs (COM, xlwings, etc.) via MCP servers | HostAgent/AppAgent split. Speculative multi-action execution checked against UIA. Picture-in-Picture virtual desktop through RDP loopback | Windows-only. Complex setup |
| **OmniParser v2 / OmniTool** | Screenshot parsed into boxes, captions and IDs (YOLOv8 + Florence-2) | Element IDs, turned into coordinates | Makes any LLM a CUA. 0.6 s/frame on A100, 0.8 s on RTX 4090 | Adds a GPU hop. Parser errors spread downstream |
| **Agent S2 → S3 (Simular)** | Screenshots (plus OCR) | GUI actions plus coding agent | S3 dropped hierarchical planning and added bBoN (parallel rollouts, "behavior narratives," a comparative judge) | bBoN multiplies compute. Built for a single monitor |
| **UI-TARS / UI-TARS-2 (ByteDance)** | Pure screenshots (native agent) | Unified GUI actions plus GUI-SDK (terminal, file system) | End-to-end multi-turn RL. 47.5 OSWorld, 50.6 WindowsAgentArena, 73.3 AndroidWorld, 88.2 Online-Mind2Web | Open 7B-class models trail frontier APIs on long tasks |
| **browser-use** | Distilled DOM with indexed interactive elements; screenshots only when needed | Element-index actions | Fastest reported web agent loop, from sending fewer images | Web only. Needs anti-bot infrastructure in production |
| **Playwright MCP / Playwright CLI / Vercel agent-browser** | Accessibility snapshot with stable refs (e.g., `e5`) | Ref-based click/type | Deterministic and needs no vision model. The Playwright team measured about 114K tokens per task via MCP vs about 27K via the CLI, which keeps snapshots on disk | Snapshots grow large on dense pages. Refs go stale after re-renders |
| **trycua/Cua** | Screenshots plus OS drivers | Cross-OS Sandbox SDK (shell, mouse, keyboard, mobile gestures) | Lume runs macOS/Linux VMs through Apple Virtualization.Framework. Cua Driver runs macOS apps in the background without taking the cursor. cua-bench runs OSWorld and ScreenSpot | Fast-moving APIs. macOS VMs require Apple Silicon |
| **E2B Desktop** | Screenshots via Xvfb + Xfce; stream through x11vnc/noVNC | xdotool-style mouse and keyboard | Firecracker microVMs, with about 150 ms base sandbox start, memory-inclusive pause and resume | No `/dev/kvm` inside the sandbox. One stream at a time |

**What they have in common:** every production system runs the same basic loop (observe, then the model proposes one or more actions, then the harness executes and re-observes). They differ in three places: (a) what they observe, (b) how rich the action vocabulary is (pixels vs. elements vs. code), and (c) how much verification the harness does rather than the model.

### 2. Observation strategies: speed vs. precision

**Raw screenshots.** These are universal but expensive and lossy.
- **Claude:**
  - Opus 4.7 and later models accept up to 2576 px on the long edge and 4,784 visual tokens (⌈w/28⌉×⌈h/28⌉, about 3.75 MP). Earlier models accept 1568 px and about 1.15 MP.
  - A typical screenshot costs roughly 1,000–1,800 input tokens.
  - With more than 20 images in a request, every image in that request is held to a stricter per-side limit.
  - Under the toolset, the API does not downscale for you. Oversized images are rejected.
- **OpenAI:** use `detail: "original"` and avoid `high` or `low` for computer use. When downscaling, OpenAI sees strong results at 1440x900 and 1600x900.
- **Anthropic's own negative results:** splitting screenshots into tiles, overlaying coordinate grids and changing the resize algorithm gave no consistent improvement.
- **Anthropic's positive results:** pre-downscaling ("worth more than almost any other optimization"), keeping the aspect ratio, and placing the instruction text *before* the image.
- **Retina gotcha:** macOS captures at device pixel ratio 2. Halve the coordinates, or downscale by 2x first.

**Accessibility trees (desktop):**
- **Windows UI Automation** is the richest. It gives control types, enabled/visible state, and patterns such as Invoke, Value and Toggle.
- **macOS AX** needs the Accessibility (and Screen Recording) permission.
- **Linux AT-SPI** coverage varies by toolkit.

Accessibility trees give exact bounding boxes, text and states for free, and support harness-side validation. Their weakness is custom-drawn canvases (Electron edge cases, Figma, Blender, games, DAWs). That is why UFO2 fuses UIA with OmniParser and removes duplicates by IoU overlap.

**DOM/CDP (browser):** This is the best speed and precision path for web tasks.
- Accessibility snapshots with refs (Playwright MCP, agent-browser) and distilled indexed DOM (browser-use) let text-only models act deterministically.
- The cost is context size. Dense pages can produce very large snapshots. Snapshot once per page state, act on refs, and snapshot again only after changes.

**Set-of-Mark and parsing (OmniParser-style):** These draw numbered boxes so the model can say "click 17." They are useful for models with weak native grounding, or as a cross-check. The cost is about 0.6–0.8 s of GPU parsing per frame, plus added risk from parser errors. With frontier models trained natively on coordinates, SoM is now mostly a fallback, not the default.

**What the evidence says.** On the web, text/DOM-first with screenshots on demand wins on speed and usually on precision. On desktops, the evidence favors hybrids: accessibility tree plus vision, with zoom. Pure-vision native agents (UI-TARS-2, Claude, GPT) have become strong enough that you can treat vision as the universal fallback. Keep structured data for verification and for text extraction, which vision often misreads.

### 3. Action strategies

- **Pixel coordinates vs. element IDs.**
  - Element or ref actions are deterministic and resolution-independent, so use them whenever a structured tree exists.
  - Pixel actions work everywhere but need exact coordinate mapping:
    - Claude's coordinates are in the pixel space of the image you returned.
    - OpenAI's coordinates are in the space of the image you sent. Remap them if you downscaled.
    - Gemini uses a normalized 0–999 grid, so `px = x/1000 × width`.
  - Anthropic's failure table sums it up: consistent offset in one direction means a scaling or dimension bug. Near-misses mean the target is small, so use zoom. Clicking the wrong element means the instruction was ambiguous.
- **Keyboard over mouse.** Both Anthropic docs suggest keyboard shortcuts or tab navigation for dropdowns, scrollbars and tiny targets. Keyboard actions are also cheaper to batch, for example `key Tab repeat=4`.
- **Batching.**
  - Claude batch actions and OpenAI `actions[]` let one model turn carry click, type, Enter, then screenshot.
  - Anthropic also suggests the harness attach a screenshot to the last result of a batch, which saves a round trip.
  - UFO2's speculative multi-action approach predicts N actions, validates each against UIA (target still enabled and visible) and replans when a check fails. The UFO2 paper reports that this lowers inference cost by up to 51.5% without hurting reliability, which it also describes as up to a 51.5% reduction in average completion steps on complex tasks.
- **API/CLI/code first, GUI last.**
  - CoAct-1's Orchestrator sends file and data work to a Programmer agent (Python/Bash) and visual work to a GUI Operator.
  - UI-TARS-2's GUI-SDK adds terminal and file-system tools.
  - OpenAI's current guidance recommends a code-execution harness first.
  - This is the single largest step-count reducer.
- **Verification after actions.** Anthropic notes that Claude "sometimes assumes outcomes of its actions without explicitly checking." Verify with cheap signals first: DOM or accessibility state, window titles, file existence, exit codes. Fall back to a screenshot only when those are unavailable.

### 4. Grounding precision

**Specialized grounders.**
- **SeeClick, OS-Atlas, UGround, ShowUI, Aria-UI:** first-generation specialized grounders. On ScreenSpot-Pro's original full-screen evaluation, OS-Atlas-7B scored 18.9%, UGround-7B 16.5% and Aria-UI 11.3%.
- **UI-TARS-1.5 / 2:** the ByteDance models described in Section 1.
- **GTA1:** a 7B model scoring 50.1% on ScreenSpot-Pro, versus 34.5% for UGround-72B. It also uses test-time scaling over candidate actions.
- **MAI-UI:** the 32B model scores 73.9% on OSWorld-G Refine, rising to 75.0% with zoom-in.

Small targets and icon-only targets remain the weakest categories.

**Planner/grounder split.** A reasoning model decides *what* to do and a grounder decides *where*. This was the dominant open-source pattern in 2025 (Agent S2, Jedi-7B with o3, GTA1 with GPT-5 at 63.4% on OSWorld). By 2026, frontier models ground well on their own. Anthropic now suggests the split mainly for cost and latency: a reasoning orchestrator, with Sonnet or Haiku doing the mechanical clicking. Anthropic finds Sonnet 4.6 more mechanically precise than Opus 4.6, while Opus 4.7 roughly matches Sonnet's precision.

**Zoom / crop-and-refine.** ScreenSpot-Pro's central finding was that "strategically reducing the search area enhances accuracy." Claude's `zoom` implements this natively: it returns a region at full resolution, and coordinates stay in full-screenshot space. For your own grounder, use a two-pass approach. First predict a coarse region on the downscaled frame. Then re-ground inside a native-resolution crop and map back.

**Benchmark snapshot (September 2026; mostly self-reported, so read with care):**
- **OSWorld-Verified:**
  - Qwen3.8 Max 86.1%, Claude Fable 5 and Mythos 5 about 85% (aggregators).
  - Coasty claims 85.6% in-house and 82.81% verified.
  - Pointer reports 83.6% verified (Opus 4.7).
  - GPT-5.5 is reported at 78.7% (secondary source).
  - Human baseline: 72.36–72.4%.
- **OSWorld 2.0** (108 long-horizon tasks, about 318 tool calls each, median human time about 1.6 h):
  - Best binary completion is only 32.0% (Muse Spark 1.3), while partial scores pass 75%. Claude Fable 5.1 leads the tracked partial metric at 77.9%.
  - Simular reports 73% for its hosted Sai agent, but that figure is not comparable to OSWorld 1.0.
- **WindowsAgentArena:** Agent S3 50.2%, rising to 56.6% with bBoN. CoAct-1 52.5%. UI-TARS-2 50.6%.
- **AndroidWorld:** UI-TARS-2 73.3%. Agent S3 68.1%, rising to 71.6% with bBoN.
- **Online-Mind2Web:** UI-TARS-2 88.2%. Gemini 2.5 CU scored 65.7% on the Browserbase harness, with latency Google shows at about 225 s.
- **WebVoyager:** browser-use 89.1%.
- **What drove the gains:**
  - coding and SDK actions
  - multi-turn RL (UI-TARS-2)
  - best-of-N with a comparative judge (bBoN)
  - higher-resolution vision budgets (Opus 4.7, GPT-5.5)
  - zoom

**Caveat:** benchmark rows differ in step budget, OS image, tools and retries. Compare only matched setups.

### 5. Latency and speed engineering

**Where the time goes** (typical cloud-model loop; the per-step numbers are from Browser Use and OSWorld-Human, the rest is engineering judgment):

| Stage | Typical cost | Fix |
|---|---|---|
| Screen capture | ms (native APIs) to 30+ ms (naïve full-res CPU grabs) | DXGI Desktop Duplication or Windows.Graphics.Capture; ScreenCaptureKit; XShm/Xvfb framebuffer; scale on the GPU |
| Resize and encode | 5–50 ms | Downscale first. Use JPEG/WebP for photo-like screens and PNG for flat UIs. Encode the target resolution once |
| Upload | Depends on size | Smaller images. Keep the region close to the model endpoint |
| **Model inference** | **Seconds per step. Dominant.** Each screenshot adds about 0.8 s (Browser Use measurement) | Fewer images, prompt caching, smaller models for routine steps, lower thinking effort |
| Action execution | ms | Native input injection (XTEST, SendInput, CGEvent) or CDP |
| UI settle / waits | 0.5–5 s if fixed sleeps are used | Event- or diff-based settle detection |
| **Wasted steps** | 1.4–2.7x more steps than humans need | Batching, code actions, better grounding, loop detection |

**Concrete optimizations:**
1. **Capture fast and natively.**
   - Windows: DXGI Desktop Duplication gives zero-copy GPU frames. One open-source library reports that GPU-side scaling cuts readback to about 2 ms, versus about 36 ms on 3K displays.
   - macOS: ScreenCaptureKit (needs the Screen Recording permission).
   - Linux: read the Xvfb/X11 framebuffer via XShm. Avoid VNC round-trips for capture when you control the VM; use VNC only for human viewing.
2. **Send fewer and smaller images.**
   - Pick the smallest resolution that holds accuracy (720p for Claude 4.6-class models, 1080p for Opus 4.7+, 1440x900 for OpenAI).
   - Skip screenshots when DOM or accessibility text answers the question.
   - Use pixel diffing (hash or SSIM) to detect "nothing changed" and short-circuit re-observation. Send cropped changed regions through `zoom`-style images rather than full frames when the model supports it.
3. **Prompt caching done right.**
   - Place one cache breakpoint after system prompt plus tools, and up to three on recent tool results.
   - Prune old screenshots *in batches*, not every turn. Anthropic's default is to keep the last three and prune every 25 turns, so the cached prefix stays byte-identical. Anthropic notes that for Claude Fable 5.1 and Opus 5.5, client-side pruning should be avoided because it invalidates the cache.
   - Declare tools with stable ordering.
4. **Right-size models and thinking.**
   - Anthropic's measurements: on Claude 4.6 models, `medium` effort is the sweet spot, and `low` uses *fewer* output tokens than disabling thinking, because it causes fewer retries. `max` gives no accuracy gain on UI tasks.
   - On Opus 4.7, `high` is the default and `low` is for throughput. Opus 4.7 at low effort scores like Sonnet 4.6 at max, using about 1/10th the tokens.
   - Use Haiku-class or local models (UI-TARS, a GTA1 grounder) for routine clicks, and escalate to the frontier planner on anomalies.
5. **Batch and speculate.** Request multi-action batches and validate each step on the harness side (UFO2-style), replanning only when a check fails. OSWorld-Human's grouped-action trajectories show many steps can share one observation.
6. **Settle detection instead of sleeps.**
   - Browser: wait on network idle, the `load` event, a MutationObserver going quiet, or a Playwright locator's actionability checks.
   - Desktop: wait on UIA events (StructureChanged, WindowOpened), AX notifications, or two consecutive identical frames within a timeout.
   - Keep `wait` as the model's explicit escape hatch.
7. **Stream and pipeline.** Stream model output and start executing the first action of a batch as soon as its tool block completes, if your API exposes it. Capture the next screenshot while the model is still generating text.
8. **Parallelize across environments, not within one.** Run N sandboxes for N tasks, or for bBoN rollouts. Inside a single desktop, keep actions sequential.

### 6. Environment and infrastructure

**Isolation options:**
- **Direct host control** has the lowest latency and uses real user state, but it is dangerous and hijacks the cursor. It is acceptable only for supervised personal use. Cua Driver and UFO2's PiP desktop show ways to run agents alongside the user without stealing input.
- **Containers** (Docker with Xvfb, as in Anthropic's reference) are cheap and start fast, but offer weaker isolation and Linux-only GUIs.
- **MicroVMs** (Firecracker, as in E2B) give hardware-level isolation.
  - Base sandbox start is about 150 ms.
  - Pause captures memory and running processes, at about 4 s per GiB of RAM to pause and about 1 s to resume.
  - This is ideal for fast reset and "fork from a known state."
- **Full VMs** (KVM/QEMU, Hyper-V, VMware, Apple Virtualization.Framework) are required for Windows and macOS. They support snapshot and restore, which is what OSWorld uses for task setup.

**Linux:**
- Use **Xvfb or Xvnc on X11**. The whole automation ecosystem (xdotool, XTEST, AT-SPI) assumes it, and so does every major reference: Anthropic's demo, E2B and OSWorld.
- Wayland breaks this. xdotool's maintainer writes that "Wayland comes along and eliminates *everything* xdotool can do." Wayland input then requires the RemoteDesktop portal plus libei, which brings repeated permission prompts, or root-level uinput.
- If you must support Wayland hosts, run the agent in a nested X11/Xvfb session or a VM.

**macOS:**
- Use Apple Virtualization.Framework VMs (Lume/Cua, Tart) on Apple Silicon.
- Pre-grant Accessibility and Screen Recording (TCC) permissions in your golden image.
- Use ScreenCaptureKit for capture and CGEvent for input. Handle the 2x DPR scaling.

**Windows:**
- Use Hyper-V or KVM VMs (WindowsAgentArena, OmniTool) with UIA for structure.
- Use DXGI or Windows.Graphics.Capture for frames and SendInput for input.
- RDP loopback or a separate session isolates the agent from the user (UFO2's PiP).
- Windows Sandbox works for disposable single runs.

**Reset and reliability:** Build golden images with apps preinstalled, permissions granted, and popups such as first-run wizards, update prompts and cookie banners pre-dismissed. Snapshot after setup, and restore per task or per rollout. Warm pools hide boot time.

**Security:**
- Anthropic recommends a dedicated VM or container with minimal privileges, no sensitive credentials, a domain allowlist, and human confirmation for consequential actions (purchases, terms of service, cookies). OpenAI and Google give the same advice.
- Measured injection risk is real and model-dependent:
  - Anthropic's Claude for Chrome pilot saw 23.6% attack success without mitigations and 11.2% with them.
  - Opus 4.5 reached 1% against an adaptive attacker.
  - The Opus 4.6 system card, as reported secondhand, shows GUI computer-use attacks succeeding far more often with many attempts (78.6% at 200 attempts without safeguards, 57.1% with them).
- Treat all screen content as untrusted. Enforce permissions in the harness before *each* action in a batch, since a batch can complete a multi-step risky action within one turn. Keep secrets out of the model's view (inject credentials via the harness or a vault).

### 7. Reliability techniques

- **Self-verification with cheap checks first.** Prompt "after each step, take a screenshot and evaluate," but prefer deterministic harness checks: DOM values, UIA states, file hashes, exit codes. Reflection is expensive. It took 33.6% of Agent S2's time.
- **Task-state memory.** Keep a compact progress summary (browser-use's "memory" field, Agent S3's behavior narratives) instead of dozens of old screenshots. This keeps prompts small and stops per-step latency from climbing.
- **Loop and stall detection.** If the screen hash and action repeat for N steps, force a replan, a different modality (keyboard instead of mouse, code instead of GUI) or a snapshot rollback.
- **Popups and timing.** Pre-dismiss popups in golden images. Add harness-side popup detectors (UIA WindowOpened, DOM dialog events). Remember that some native dropdowns and system dialogs are not visible in a browser viewport screenshot; use keyboard, JavaScript or DOM manipulation instead.
- **Best-of-N and judges** for high-value, non-interactive tasks. bBoN's comparative judge beat independent judging (WebJudge). It needs resettable, parallel environments, and N rollouts cost roughly N times the compute.
- **Guardrails.**
  - Enforce max iterations and action allowlists or denylists.
  - Require confirmation for sensitive actions (Gemini `safety_decision`, OpenAI `needsApproval`).
  - Keep full transcript and screenshot logs with predicted clicks overlaid, which is Anthropic's recommended debugging practice.

## Recommendations: the 2026 reference architecture

### Core design (all targets)

```
┌──────────────── Control plane (trusted) ────────────────┐
│ Orchestrator/planner model (frontier; medium/high effort)│
│ Router: code/API ▸ structured (DOM/AX) ▸ pixels+zoom     │
│ Executor model for routine clicks (Sonnet/Haiku/local)   │
│ Policy engine: per-action allowlist, HITL confirmation   │
│ State: progress memory, screenshot ring buffer, caching  │
│ Verifier: DOM/AX/file checks, loop detector, judge (BoN) │
└───────────────▲──────────────────────────┬──────────────┘
                │ observations (small)     │ batched actions
┌───────────────┴──────────────────────────▼──────────────┐
│ Environment agent (inside sandbox)                       │
│ Fast capture + GPU downscale + diff/hash                 │
│ Structure: CDP/Playwright, UIA, AX, AT-SPI               │
│ Input: CDP / XTEST / SendInput / CGEvent                 │
│ Settle detector (events + frame stability)               │
│ Shell/Python runtime, app APIs/MCP servers               │
└──────────────────────────────────────────────────────────┘
      MicroVM/VM with golden snapshot, egress allowlist
```

Key choices:
- Run the harness *outside* the sandbox, as OpenAI's Agents SDK guidance also advises, so credentials, audit logs and approvals never share a compute boundary with model-directed execution.
- Put a thin, low-latency "environment agent" inside the sandbox that does capture, scaling, structure extraction, input and settle detection locally. The control plane then receives one compact observation per batch.

### (a) Browser-only agents

- **Stack:**
  - Playwright on Chromium over CDP, in a remote browser service (Browserbase, Steel, Browserless) or your own container pool.
  - Accessibility-snapshot refs or distilled DOM as the primary observation (Playwright CLI/agent-browser style, snapshots on disk, to avoid the roughly 4x MCP token overhead).
  - Screenshots only on demand or on uncertainty.
- **Model tools:** `navigate`, `click(ref)`, `fill(ref, text)`, `select(ref, value)`, `press(key)`, `extract(query)`, `eval_js` (gated), plus a vision fallback (the provider's computer tool, e.g., Claude's browser-use toolset or Gemini CU) for canvases and visual-only widgets.
- **Speed:** a stable cached prefix, batched ref actions, network-idle and locator-actionability waits instead of sleeps, and a small model for navigation steps.
- **Precision:** refs remove coordinate error. Re-snapshot after any DOM-changing action, since refs go stale. For vision fallback, set the viewport to match the screenshot and model limits exactly.

### (b) Full desktop agents

- **Environment by OS:**
  - **Linux (default for scale):** Firecracker or KVM microVMs with Xvfb, a lightweight WM (Xfce/Mutter), XShm capture, XTEST input and AT-SPI. Snapshot and resume for resets. Avoid Wayland.
  - **Windows:** Hyper-V or KVM VMs with UIA plus OmniParser-class vision fusion, DXGI capture, SendInput, and COM/PowerShell/MCP app APIs. Use an RDP-loopback session or separate VM so the agent never touches the user's desktop.
  - **macOS:** Apple Virtualization.Framework VMs (Lume/Cua) with pre-granted TCC permissions, ScreenCaptureKit, CGEvent and the AX API. Correct for 2x DPR.
- **Action router:** try, in order: shell/Python/AppleScript/PowerShell, then app API or MCP, then an accessibility-tree action (Invoke/Value patterns), then keyboard shortcut, then pixel click (with zoom for small targets).
- **Grounding:** use the frontier model's native coordinates, with zoom first. Optionally add a local specialized grounder (UI-TARS / GTA1 / MAI-UI class) on a GPU next to the VMs for cheap re-grounding. Use accessibility-tree bounding boxes to snap a predicted point to the nearest matching control.

### Step-by-step build plan

1. **Baseline loop (week 1).** Run Anthropic's reference container or the OpenAI sample app. Implement exact screenshot sizing and coordinate mapping, plus a debug overlay that draws predicted clicks. Anthropic's best-practices repo ships a localization playground for this.
2. **Fast environment agent.** Add native capture, GPU or SIMD downscale, frame hashing, and settle detection. Measure per-stage latency from the start.
3. **Structured channels.** Add CDP/Playwright refs for browsers and UIA/AX/AT-SPI extraction for desktops. Expose element-level actions and harness-side verifiers.
4. **Code and API tools.** Add a sandboxed shell/Python runtime and app-specific MCP servers. Route file and data work there, following the CoAct-1 pattern.
5. **Batching and speculation.** Enable multi-action batches. Validate each action against structure before executing, and stop at the first failure using the provider's halt semantics.
6. **Context and cost control.** Set cache breakpoints, prune screenshots in batches, keep a progress memory, tune effort per model, and route routine steps to smaller models.
7. **Infrastructure.** Build golden images per OS, snapshot and restore, warm pools, parallel sandboxes, egress allowlists and secret injection.
8. **Safety.** Build a per-action policy engine, human-in-the-loop for irreversible actions, provider injection classifiers (keep the official tool types to benefit from them), and audit logs.
9. **Evaluation.** Run OSWorld-Verified / WindowsAgentArena / your own task suite through cua-bench or the OSWorld harness. Track success *and* steps versus OSWorld-Human-style references, wall-clock time, and tokens per task.
10. **Scale-up options.** Add bBoN rollouts for high-value batch tasks. Fine-tune or RL a local executor on your logged trajectories.

## Caveats

- **Leaderboard numbers are contested.** Most 2026 OSWorld-Verified rows are vendor self-reports. llm-stats lists 0 verified out of 24. Setups vary (step budgets, retries, tools). Benchmark saturation above the human baseline says little about messy production conditions such as logins, bot detection and app updates. OSWorld 2.0's 32% best binary completion is a sober counterpoint.
- **Some figures are secondhand.** These include the GPT-5.4/5.5 OSWorld scores and GPT-5.5's 10.24 MP screenshot handling (Medium posts), the Playwright token benchmark (reported by third parties), the Opus 4.6 injection figures (VentureBeat reporting of the system card), and the DXGI timing (a single open-source project). Verify them before relying on them.
- **Vendor self-measurement.** Browser Use's speed comparisons, Simular's Sai claims and Coasty's scores are self-published.
- **Fast-moving APIs.** Anthropic moved from `computer_20251124` (single tool with `action` and display dimensions) to `computer_toolset_20260801` (17 member tools, no display parameters, no API-side downscaling). OpenAI moved from `computer-use-preview` to a GA tool with batched `actions[]` and now recommends code execution. Gemini's action names differ between 2.5 and 3.x. Pin versions.
- **Model-specific advice changes.** Resolution limits, effort recommendations and pruning guidance differ by model generation. For example, Anthropic advises *against* client-side pruning on Claude Fable 5.1 and Opus 5.5. Re-check the provider docs whenever you upgrade models.