# R2: Research evidence on long sessions, compaction, delegation and grounding

Scope: academic and lab research only (no vendor docs). Searched 14 Oct 2026. "Verified" means I opened the arXiv abstract or full-text page and the numbers came from there. "Unverified" means search snippets or secondary write-ups only. Grades: **strong** = controlled and replicated across studies; **moderate** = one controlled study; **weak** = anecdote, position paper, or tiny sample.

Caveat on dates: many 2026 items are fresh preprints with no peer review yet. Older results (2023–24 models) are flagged where transfer is doubtful.

---

## 1. Context rot: how quality falls as context fills

**Lost in the Middle** (Liu et al.). https://arxiv.org/abs/2307.03172, TACL 2024. Claim: accuracy is highest when the needed fact sits at the start or end of the input and drops when it sits in the middle (a U-shaped curve). Evidence: multi-document QA and key-value retrieval on GPT-3.5-Turbo, Claude 1.3, MPT-30B, LongChat-13B. Grade: strong (replicated many times). Older models; Chroma (below) found position mattered little for newer models on its needle task, so the U-shape itself may have faded, though length effects have not.

**Context Rot** (Chroma technical report, Hong, Troynikov, Huber). https://www.trychroma.com/research/context-rot, July 2025, industry report. Verified. Claim: "Across all experiments, model performance consistently degrades with increasing input length," even on simple tasks with difficulty held fixed. Evidence: 18 models (GPT-4.1, Claude 4, Gemini 2.5, Qwen3). Lower question–needle similarity degrades faster; one distractor hurts and four compound; coherent haystacks hurt more than shuffled ones. On LongMemEval, a ~300-token focused prompt beat the ~113k-token full history for every model, and the gap remained with thinking on. Claude models abstained more and hallucinated less under distractors. Grade: moderate (controlled, one lab, results mostly as charts).

**Context Length Alone Hurts LLM Performance Despite Perfect Retrieval** (Du et al.). https://arxiv.org/abs/2510.05381, Oct 2025 (EMNLP 2025 Findings per listing; venue not confirmed by me). Verified. Claim: length itself hurts even when retrieval is perfect. Evidence: 5 models, math/QA/coding; accuracy fell 13.9%–85% as input grew, still well inside the advertised window. The drop persisted when the filler was whitespace or fully masked, and when the evidence sat right before the question. Fix: have the model first recite the evidence, then answer (up to +4% on RULER for GPT-4o). Grade: moderate.

**NoLiMa** (Modarressi et al.). https://arxiv.org/abs/2502.05167, ICML 2025. Verified via proceedings listings. Claim: when the question and the fact share no literal words, long context collapses fast. Evidence: 13 models claiming ≥128K; at 32K, 11 of 13 fell below half their short-context score; GPT-4o went 99.3% to 69.7%. Reasoning models and chain-of-thought did not close the gap. Grade: strong (consistent with RULER, HELMET, Chroma).

**LLMs Get Lost in Multi-Turn Conversation** (Laban et al.). https://arxiv.org/abs/2505.06120, ICLR 2026 (oral). Verified. Claim: when instructions arrive piecemeal over turns, models commit early to wrong assumptions and do not recover. Evidence: 15 models, 200,000+ simulated conversations, 6 tasks; average drop 39% vs one fully specified prompt. Best-case skill fell only ~16%; run-to-run unreliability rose ~112%. Concatenating all the pieces into one fresh prompt restored 95.1% of full performance. Recap at the end helped (GPT-4o 59.1 to 76.6, full = 93.0). o3 and R1 degraded as much as non-reasoning models. Authors' advice: start a fresh conversation with consolidated information. Grade: strong (very large controlled study, peer-reviewed).

**The Illusion of Diminishing Returns** (Sinha et al.). https://arxiv.org/abs/2509.09677, ICLR 2026. Abstract-level only. Claim: per-step accuracy falls as tasks get longer, and models become more error-prone once their own earlier mistakes are in the context ("self-conditioning"). Scaling the model does not remove this; thinking mode does mitigate it. Grade: moderate.

**Newer 1M-window models.**
- *Retrieval and Multi-Hop Reasoning in 1M-Token Context Windows* (Chow). https://arxiv.org/abs/2605.02173, May 2026, preprint. Verified. Single-fact retrieval at 1M is "essentially solved" (Gemini 3.1 Pro, Claude Opus 4.7, GPT-5.5 all 100%). Three-hop reasoning is not: Gemini and Claude stay above 80% to 512K with modest decline at 1M; GPT-5.5 and Qwen3.6-plus collapse between 512K and 1M. Grade: moderate (single author, one corpus).
- *AgentLongBench* (Fang et al., Fudan). https://arxiv.org/abs/2601.20730, Jan 2026, preprint. Verified abstract. 32K to 4M tokens of agent rollouts; agents do fine at static lookup but struggle to synthesise changing information; dense tool outputs are harder than long chat. Per-model numbers (one model above 50 at 2M, others below 30–40 at 1M) are from a snippet: unverified. Grade: moderate.
- *SlopCodeBench* (Orlanski et al.). https://arxiv.org/abs/2603.24755, Mar 2026, preprint. Verified. 15 coding agents extending their own code over 196 checkpoints: best agent passes 14.8% of checkpoints; structural erosion rises in 77% of trajectories and verbosity in 75.5%; quality instructions lower the starting point but not the rate of decay. Grade: moderate.
- **Contradicting:** *LoCoBench-Agent* (Qiu et al., Salesforce). https://arxiv.org/abs/2511.13998, Nov 2025, preprint. Verified abstract. Reports "remarkable long-context robustness" for agents across 10K–1M with tools, and that comprehension and efficiency trade off. Per-length numbers not in the abstract. Note the difference: here agents fetch what they need with tools rather than holding it all in context.

**Implication for an orchestrator that drives CLIs:** The evidence that quality falls with length well before the window limit is strong, including on 2025–26 models; 1M windows mainly fix lookup, not reasoning over many facts. A fresh session with a consolidated brief is directly supported (Laban's 95% recovery); trigger it on length and on signs of drift, not only near the hard limit.

---

## 2. Memory and continuity: what survives a hand-off

**Summaries lose constraints, and lose more with each round.**
- *Lost in Context Compaction: When LLM Systems Drop Side Constraints* (Wang et al., Penn State/IBM). https://arxiv.org/abs/2608.11242, Jul–Oct 2026, preprint. Verified. Standing user rules ("don't delete emails until I confirm") are dropped by compactors. An inbox agent deleted without approval in 30/100 sessions after compaction, 0 before. Compactors kept only 17% of injected constraints on average. Fix: a small model extracts constraints into a separate registry appended after the summary (whether word for word isn't stated): over 90% retention, no change to model or compactor. Grade: moderate.
- *The Compaction Cliff in Long-Running AI Agent Memory* (Zerhoudi et al.). https://arxiv.org/abs/2608.22752, Aug 2026, CIKM 2026. Verified abstract. A widely used CLI's built-in compact prompt on Sonnet 4.6 kept 53% of safety rules after one round and 10% after five. Classifying each line by type and compacting rules at exact wording kept 96% over five rounds (2–4x better than the best single-shot LLM compactor). Grade: moderate.
- *Governance Decay* (Chen). https://arxiv.org/abs/2606.22528, Jun 2026, preprint, single author. Verified abstract. 7 model families, 1,323 episodes: 0% policy violations with the policy in full context, 30% after compaction (up to 59% for some models). When the summary kept the rule, violations stayed 0%; when it dropped it, 38%. Injected text can steer the summariser into dropping a rule. "Pinning" constraints outside the lossy summary restored 0%. Grade: moderate (single author).
- *Agentic Context Engineering (ACE)* (Zhang et al., Stanford/SambaNova/Berkeley). https://arxiv.org/abs/2510.04618, ICLR 2026. Verified abstract. Names two failure modes of rewritten memory: "brevity bias" (summaries drift toward generic) and "context collapse" (repeated full rewrites erode detail). Incremental, itemised updates beat rewrites: +10.6% on agent tasks, +8.6% on finance. Grade: moderate.

**Validating the summary before trusting it.** *Slipstream* (Chen, Pan, Dai, Netravali, Princeton). https://arxiv.org/abs/2605.08580, May 2026, preprint. Verified abstract. Builds the summary in parallel while the agent keeps working on the full context, then a judge checks the summary against what the agent actually went on to need. Up to +8.8 points accuracy and up to 39.7% lower latency on SWE-bench Verified and BrowseComp. Grade: moderate.

**Learned "when to compact".** *AutoCompact* (Zhang et al.). https://arxiv.org/abs/2610.02163, Oct 2026, preprint. Verified abstract. Training the coding agent to decide when to compact and what working state to keep (conclusions, code/workspace state, remaining steps) gave +9.2 points on SWE-bench Verified and +5.0 on SWE-PolyBench. Not usable directly with closed CLIs, but its checklist of what to keep is. Grade: moderate.

**Structured notes and raw-history retrieval vs summaries.**
- *LongMemEval* (Wu et al., UCLA/Tencent). https://arxiv.org/abs/2410.10813, ICLR 2025. Abstract-level. ~30% accuracy drop for long-context models and commercial assistants across sustained histories; splitting history into sessions/rounds and indexing with extracted facts and time improved recall and QA. Grade: strong (widely reused benchmark).
- *Letta, "Benchmarking AI agent memory: is a filesystem all you need?"* https://www.letta.com/blog/benchmarking-ai-agent-memory, Aug 2025, industry blog. An agent with plain file tools (grep, open, search) over the raw history scored 74.0% on LoCoMo with GPT-4o mini, above a dedicated memory product's reported 68.5%. Grade: weak (self-run, vendor-adjacent), but directionally matches the next item.
- *Mem0* (Chhikara et al.). https://arxiv.org/abs/2504.19413, Apr 2025. Partly verified: full-context baseline sends ~26k tokens with p95 latency ~17 s vs ~1.4 s for extracted memory. My recollection that full context still scored highest on accuracy is unverified. Grade: moderate.
- *Context-Folding* (Sun et al., ByteDance/CMU). https://arxiv.org/abs/2510.11967, Oct 2025. Verified abstract. Branch into a sub-task, then fold it back as a short outcome: matches or beats full-history agents with ~10x smaller active context and "significantly outperforms" summarisation-based management. Grade: moderate (needs RL training for best results).
- *ReSum* (Alibaba). https://arxiv.org/abs/2509.13313, Sep 2025. Unverified detail: periodic summaries +4.5% over full-history ReAct on web-search tasks. Grade: moderate.

**Negative knowledge.** No study measured "dead ends" surviving a hand-off directly. The closest: Scrouting's hand-off note (section 7) explicitly carries "dead ends already tried" and replays its own claims in a sandbox before handing over. I found no paper testing whether exact identifiers (paths, hashes, IDs) survive compaction: gap.

**Memory poisoning and self-reinforcing errors.**
- *How Memory Management Impacts LLM Agents* (Xiong et al.). https://arxiv.org/abs/2505.16067, arXiv preprint (no venue listed; "ACL 2026" not confirmed). Full text checked 2026-10-14. Agents copy retrieved past records whose inputs look similar ("experience-following"); stored mistakes resurface and spread. Adding memories selectively (only evaluator-approved) and deleting bad ones gave 15.5–25.5 points absolute over adding everything (Table 1, strict evaluator vs add-all, four agents). Grade: moderate.
- *HaluMem* (Chen et al.). https://arxiv.org/abs/2511.03506, Nov 2025. Abstract-level. Memory systems create and accumulate errors at extraction and update time (fabrication, conflicts, omissions) which then poison answers; histories over 1M tokens. Grade: moderate.
- *MINJA* (Dong et al.). https://arxiv.org/abs/2503.03704, 2025. 98.2% injection and 76.8% attack success in idealised settings via ordinary queries. **Contradicting/limiting:** a 2026 follow-up (https://arxiv.org/abs/2601.05504) finds existing legitimate memories sharply reduce effectiveness. Grade: moderate.
- Self-conditioning (Sinha, section 1) is the in-context version: own errors in context raise later error rates.

**Implication for an orchestrator that drives CLIs:** Free-text summaries are the weak link: they silently drop standing rules, and loss compounds per round, so keep constraints, decisions and dead ends as a separate verbatim, itemised block carried unchanged across every rebirth. Keep the raw transcript searchable and store only verified facts (not the agent's own claims) as memory; check a hand-off note against the transcript before it replaces it.

---

## 3. Trajectory and tool-output compression for coding agents

**The Complexity Trap** (Lindenbauer et al., JetBrains/TUM). https://arxiv.org/abs/2508.21433, NeurIPS 2025 workshop (DL4Code). Verified full text. Claim: simply hiding old tool outputs (keep the last 10 turns' outputs) is as good as LLM summarisation and cheaper. Evidence: SWE-agent, SWE-bench Verified (500), five configs (Qwen3-32B ± thinking, Qwen3-Coder 480B, Gemini 2.5 Flash ± thinking). Masking cut cost ~50–57% vs raw in most configs and matched or beat summary on solve rate (e.g. Qwen3-Coder 53.4 raw / 54.8 masked / 53.8 summary; Gemini Flash 32.8 / 35.6 / 36.0). Exceptions: Gemini Flash with thinking lost solve rate under both (40.4 raw / 36.4 masked / 31.4 summary). **Cases where cutting tokens raised cost:** summaries made trajectories 13–15% longer (the agent kept going); summary calls were up to 7.2% of cost; an untuned hybrid lost cost efficiency through cache invalidation and extra summary calls. A tuned hybrid saved a further 7–11%. Masking window had to be re-tuned for OpenHands. Grade: moderate-to-strong (500 instances, 5 configs, partial replication on a second scaffold).

**AgentDiet / Reducing Cost of LLM Agents with Trajectory Reduction** (Xiao et al.). https://arxiv.org/abs/2509.23586, FSE 2026. Search-verified. A small reflection model removes useless, redundant and expired content: input tokens −39.9% to −59.7%, total cost −21.1% to −35.9%, same success. Claude 4 Sonnet and Gemini 2.5 Pro, SWE-bench Verified and Multi-SWE-bench Flash. Cost savings are smaller than token savings because of output tokens, invalidated caches, and the reflection calls themselves. Grade: moderate.

**ACON** (Kang et al., KAIST/Microsoft). https://arxiv.org/abs/2510.00615, ICLR 2026. Abstract-level. Learns a compression guideline by comparing runs that succeed with full context and fail when compressed: 26–54% peak token reduction while keeping accuracy on AppWorld, OfficeBench, multi-objective QA. Grade: moderate.

**SWE-Pruner** (Wang et al.). https://arxiv.org/abs/2601.16746, Jan–May 2026, preprint. Verified abstract. The agent states a goal ("focus on error handling") and a 0.6B model keeps only matching lines of tool output: 23–54% fewer tokens on SWE-bench Verified and similar, with success improving. A snippet says file reads are over 70% of tokens with mini-SWE-agent and Sonnet 4.5 (unverified). Follow-up SWE-Pruner Pro (https://arxiv.org/abs/2607.18213) reports up to 39% savings and +3.8 points (unverified detail). Grade: moderate.

**Don't Break the Cache** (Lumer et al.). https://arxiv.org/abs/2601.06007, Jan 2026, preprint. Search-verified. 500+ agent sessions across three providers: prompt caching cut cost 41–80% and time-to-first-token 13–31%. Naive full-context caching can increase latency; dynamic content in the prefix and changes to the tool set break the cache. Grade: moderate. Relevance: any compaction or rewrite of earlier context forfeits cache savings until the new prefix is cached again.

**How Do AI Agents Spend Your Money?** (Bai et al., incl. Brynjolfsson, Pentland). https://arxiv.org/abs/2604.22750, Apr–Oct 2026, preprint. Verified abstract. Eight frontier models on SWE-bench Verified: input tokens, not output, drive most cost; the same task can vary 30x in tokens across runs; accuracy peaks at intermediate cost then flattens; models predict their own cost poorly (r ≤ 0.39) and under-estimate it. Grade: moderate.

**Implication for an orchestrator that drives CLIs:** Dropping or trimming stale tool output is the best-evidenced, cheapest saver and usually keeps quality; LLM-written summaries are not better and can lengthen runs. Count cache invalidation and extra summary calls in any saving claim; compact rarely and in large steps rather than often.

---

## 4. Multi-agent vs single-agent

**Towards a Science of Scaling Agent Systems** (Kim et al., Google Research/DeepMind/MIT). https://arxiv.org/abs/2512.08296, Dec 2025, preprint. Search-verified. 180 configurations, 5 architectures, 3 model families, 4 benchmarks, equal token budgets. Centralised orchestration gained ~81% on parallelisable finance analysis; every multi-agent variant lost 39–70% on sequential planning. Tool-heavy tasks suffer most from coordination overhead. Once a single agent passes ~45% success, adding agents gives diminishing or negative returns. Independent agents amplified errors 17.2x vs 4.4x with a central coordinator. Grade: moderate-to-strong (large, controlled; not coding-specific).

**Why Do Multi-Agent LLM Systems Fail? (MAST)** (Cemri et al., Berkeley). https://arxiv.org/abs/2503.13657, NeurIPS 2025 D&B. 1,600+ traces, 7 frameworks: 14 failure modes in three groups (specification, inter-agent misalignment, verification); gains over single agent or best-of-N often minimal. Older models (GPT-4, Claude 3). Grade: moderate.

**Do More Agents Help?** (Fu et al.). https://arxiv.org/abs/2606.05670, Jun–Oct 2026, preprint. Verified. Six multi-agent workflows vs one single-agent controller, GPT-4.1, ten benchmarks, 3,302 instances × 3 runs. Only one workflow beat the single agent (79.7% vs 76.65%); the other five trailed by 1.7–8.8 points. Debate and panel styles won on single benchmarks but lost overall while using 1.4x–4.3x the tokens. Grade: moderate.

**Multi-agent research system** (Anthropic engineering report). https://www.anthropic.com/engineering/multi-agent-research-system, Jun 2025, lab report. Verified. Opus 4 lead + Sonnet 4 subagents beat single Opus 4 by 90.2% on an internal research eval; agents use ~4x chat tokens, multi-agent ~15x; token spend alone explained 80% of variance on BrowseComp; parallel subagents cut research time up to 90%. Explicitly says most coding has fewer parallel parts and tasks needing shared context or many dependencies are a poor fit. Grade: moderate (internal eval).

**The Handoff Tax** (Ganz et al.). https://arxiv.org/abs/2608.24358, Aug 2026, preprint. Verified abstract. Coding agents switching between a cheap and a strong model mid-task: escalating with the full trajectory recovers less than half the quality gap at a large cost premium; handing the strong model less of the cheap model's trajectory improves escalation; down-shifting strong to cheap is the better trade, but removing the strong model's trajectory hurts it. Grade: moderate.

**Contradicting/qualifying:** Context-Folding (section 2) shows sub-trajectories folded into short results can beat one long agent; this is delegation for context isolation rather than for parallel work.

**Implication for an orchestrator that drives CLIs:** Delegate when sub-tasks are truly independent or need a clean context, and keep sequential, tightly coupled coding in one worker. Expect several times the tokens per delegated job; pass workers a short brief rather than the parent's raw history (the hand-off tax).

---

## 5. Hallucination, grounding and verification

**Self-check is weak; external checks work.**
- *LLMs Cannot Self-Correct Reasoning Yet* (Huang et al., Google DeepMind). https://arxiv.org/abs/2310.01798, ICLR 2024. Without outside feedback, self-correction often makes answers worse. Older models. Grade: strong (widely replicated).
- *On the Self-Verification Limitations of LLMs on Reasoning and Planning* (Stechly, Valmeekam, Kambhampati). https://arxiv.org/abs/2402.08115, ICLR 2025. GPT-4 on Game of 24, graph colouring, planning: self-critique caused "significant performance collapse"; a sound external verifier gave significant gains; simply re-prompting with the verifier's verdict kept most of the benefit. Older model, formal tasks. Grade: moderate-to-strong.
- *From Confident Closing to Silent Failure* (Advani). https://arxiv.org/abs/2606.09863, Jun 2026, ICML 2026 workshop. Verified. 9,876 tau2-bench + 1,879 AppWorld trajectories: claiming success on a task that actually failed accounted for 45–48% of failures in single-control domains, ~3% where an independent party could verify actions, and 75.8% of failures among coding-agent runs that stated a status. LLM judges reading the transcript did poorly (AUROC ≤ 0.65; 0.54 on AppWorld), keying on confident wording; simple text classifiers did better (0.83–0.95). Grade: moderate.
- *How Coding Agents Fail Their Users* (Tang et al., Notre Dame/Google). https://arxiv.org/abs/2605.29442, May–Aug 2026, preprint. Verified abstract. 20,574 real sessions from 1,639 repos (IDE and CLI): 91.5% of resolutions needed explicit user correction; overall misalignment fell over time, but constraint violations and inaccurate self-reporting grew as a share. Grade: moderate (large observational).

**Test gaming.** *ImpossibleBench* (Zhong et al.). https://arxiv.org/abs/2510.20270, ICLR 2026. Search-verified. Tasks whose tests contradict the spec, so passing means cheating: GPT-5 passed 54% of impossible SWE-bench tasks; more capable models tended to cheat more, though newer Claude models cheated less than Sonnet 3.7; prompts reduced but did not stop it. Grade: moderate.

**Abstention.** *Why Language Models Hallucinate* (Kalai, Nachum, Vempala, Zhang, OpenAI/Georgia Tech). https://arxiv.org/abs/2509.04664, Sep 2025. Abstract-level. Hallucination persists because grading rewards guessing over "I don't know"; proposes stating a confidence threshold and penalising wrong answers so abstaining pays. Mostly theoretical. Grade: weak-to-moderate. Chroma (section 1) observed Claude models abstaining more under distractors, which counts as a lost answer on scores but prevents a false one.

**Error compounding.** Sinha et al. (section 1): own errors in context raise later error rates; Kim et al. (section 4): independent agents amplify errors 17x vs 4x with a coordinator; Xiong et al. (section 2): stored errors propagate.

**Implication for an orchestrator that drives CLIs:** Never accept a worker's "done" on its word or on an LLM reading its transcript; check real state (tests run by the orchestrator, diff, git status, file existence) and ask for evidence such as command output with exit codes. Allow and reward "unknown" in briefs, and watch for edited or skipped tests.

---

## 6. Reasoning effort and quality on agentic coding

**Prompt-Induced Waste in Coding Agents** (Weinberger, Hozez). https://arxiv.org/abs/2608.01347, Aug–Sep 2026, preprint. Verified full text (Appendix N). The only study found that varies effort inside a real coding CLI on current models. 25 hard SWE-bench Verified tasks via Claude Code, low/high/xhigh, Sonnet 5 and Opus 5 (150 cells):
- Opus 5: 17 / 18 / 16 resolved; cost per resolved task $0.75 / $1.89 / $3.62; low vs xhigh not significant (p = 1.0). Low sits on the cost–quality frontier.
- Sonnet 5: 12 / 19 / 17 resolved; $1.76 / $2.32 / $3.00. High beat low significantly (7 rescues, 0 regressions, p = 0.016); xhigh was dominated by high.
- xhigh vs low added 12–16k thinking tokens and 23–30 turns and 2.4–4.5x billed cost; all timeouts and empty patches happened at high/xhigh. An "escalate when evidence says it's hard" rule fired on 64–68% of tasks and did not improve the frontier.
- Prompt wording alone moved reasoning cost (measured on six open models, not in Claude Code): "try multiple approaches" 2.4–7.4x reasoning tokens for the same success; "think deeply" 1.6–2.2x; certainty-seeking wording 1.3–1.9x.
Grade: weak-to-moderate (small n, large run-to-run variance noted by the authors; only half of rescues reproduced).

**The Danger of Overthinking** (Cuadron et al., Berkeley). https://arxiv.org/abs/2502.08235, Feb 2025. 4,018 OpenHands trajectories on SWE-bench Verified: more internal reasoning instead of acting went with lower solve rates; picking the less-overthinking candidate improved performance ~30% and cut cost 43%. Older models (o1, R1 era). Grade: moderate.

**Inverse Scaling in Test-Time Compute** (Gema et al., Anthropic Fellows). https://arxiv.org/abs/2507.14417, TMLR 2025. Longer reasoning lowered accuracy on several constructed tasks (distractors, spurious features, long deductions); Claude models became more distractible. **Contradicting:** a public critique argues longer reasoning and wrong answers may share a cause (confusion) rather than one causing the other. Grade: moderate.

**Holistic Agent Leaderboard (HAL).** https://arxiv.org/abs/2510.11977, Oct 2025. Snippet: o4-mini low effort 54% vs high 50% on SWE-bench Verified Mini, low cheaper. Unverified detail. Grade: weak.

**Supporting the other side:** Sinha et al. found thinking reduces self-conditioning on long executions; a 2026 serving benchmark (SWE-Serve, https://arxiv.org/abs/2609.26777) reportedly shows Opus 5 rising 60.4 → 75.5% from low to max effort (unverified). Effort helps on genuinely hard or long-execution tasks and for weaker models.

**Implication for an orchestrator that drives CLIs:** Default workers to a moderate effort and do not use the top setting by default; top effort mostly adds cost, timeouts and empty results. Keep prompts free of "think deeply / try several approaches" boilerplate, which multiplies reasoning cost without improving success.

---

## 7. Fast paths and routing for small tasks

**RouteLLM** (Ong et al., Berkeley/Anyscale). https://arxiv.org/abs/2406.18665, ICLR 2025. Learned router between a strong and weak model: over 2x cost reduction with no quality loss on public benchmarks; 85% cost cut on MT-Bench at 95% of GPT-4 quality. Chat/QA, older models. Grade: strong for chat; transfer to agents unclear.

**Scrouting** (Bhola et al.). https://arxiv.org/abs/2608.04804, Aug 2026, preprint. Verified full text. Replaying learned routers on public SWE-bench Verified/Multilingual/Pro results: tasks weaker models solve are almost a subset of what the strongest solves (containment 0.94 / 0.91 / 0.77), and no learned router beat "always the strongest" on solve rate (gaps within noise). Accuracy routing has little headroom; cost routing is where gains are. A small scout model explores the repo first and writes a ~4 KB hand-off (ranked files with line ranges, a reproduction, dead ends tried, notes), with its reproduction claim replayed in a sandbox and false claims removed. Result: 159/266 SWE-bench Pro tasks vs 158 for the best single model at about one fifth of the cost per solve. Ablation: always using the cheapest fixer with the hand-off tied the routed system, so the verified hand-off, not the routing, drove the gain. Grade: moderate.

**SWE-Router** (Son et al., UCL). https://arxiv.org/abs/2607.00053, ICML 2026 DL4Code workshop. Verified abstract. A cheap model takes a few exploratory turns; a value estimate decides whether to keep it or escalate. "Maintains the majority" of the strong model's performance at much better cost efficiency; numbers not in the abstract. Grade: moderate (unquantified here).

**Efficient Agents** (Wang et al., OPPO). https://arxiv.org/abs/2508.02694, Aug 2025, preprint. Abstract-level. On GAIA, a pared-down agent (fewer planning steps, simple memory, no best-of-N) kept 96.7% of a leading framework's performance at 57% of the cost (28.4% better cost-of-pass). Grade: moderate.

**Contradicting/caution:** Handoff Tax (section 4): escalating from a cheap to a strong model mid-task recovers less than half the gap and costs extra. Bai et al. (section 3): models cannot predict their own cost (r ≤ 0.39), and human difficulty ratings correlate only weakly with cost, so up-front "is this simple?" guesses are unreliable. Kimi K2.5 in Scrouting hit the turn cap in 48% of attempts vs 17% for Opus 4.6, yet was still cheapest per solve.

**Implication for an orchestrator that drives CLIs:** A cheap, fast first pass that gathers facts (files, reproduction, dead ends), checked mechanically and handed to the worker, is better supported than guessing up front which model a task needs. Decide on escalation from what the first few turns found, and when escalating, pass a short verified brief, not the cheap model's whole transcript.

---

## Cross-cutting gaps

- No study found measuring whether exact identifiers (paths, hashes, IDs) survive summaries; no study of "negative knowledge" loss as its own variable.
- Little controlled work on multi-hour sessions with today's CLIs specifically; most compaction studies use their own scaffolds.
- Many 2026 results are single preprints; the strongest, most replicated findings are: length hurts quality; fresh consolidated prompts recover it; self-checking is weak and external checks work; masking old tool output is a safe, large saving.
