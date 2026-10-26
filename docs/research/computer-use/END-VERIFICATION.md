# Computer-use end verification

Reviewed `2b6b9d993..HEAD` on `brigadier/f59a482a/session`, including the lean
shim, native transports/backends, target guard, core host/placement, desktop
Settings, packaging, workflows and skill. Read `brigadier show f59a482a`, context
and PHASE1-PLAN first. Standing constraints: session L1, L10–L19; L13 supersedes
driver options and a benchmark. No benchmark or driver options added.

**Verification remains incomplete.** Fixes and passing automated checks below
do not establish reliability parity or native Windows/Linux desktop operation.
`OURS-AUDIT.md` was not edited by this worker.

## Remaining findings

1. **P1 — Windows update can replace an executing MCP shim image.**
   `crates/core/src/manager/computer_use.rs:390` uses a single placed executable;
   `:397` stops its daemon, then `:407` renames over the executable. `stop_helper`
   at `:429` does not end live `brigadier-cu mcp` processes. A Windows shim still
   running that image can prevent replacement, making subsequent placement fail
   while old agents remain connected. This is a code-path finding, not a native
   reproduction. Needs versioned placement and an update/reconnect protocol,
   with a native Windows regression; left unresolved (worker ledger L10).

2. **P1 — the broader same-user process boundary is still absent.**
   The new peer checks reject an unrelated program connecting directly, and
   clients check the server before disclosing a token. They compare executable
   paths, not signed code or Brigadier process ancestry
   (`brigadier-cu/src/peer_auth.rs:5`, `:19`). A same-user program can still run
   the placed helper itself; the helper reads `<home>/auth`
   (`brigadier-cu/src/serve.rs:1025`) and the non-macOS daemon treats accepted
   peers as host root (`:1919`). Core supplies no separate host capability.
   Therefore `brigadier-cu host request …` remains a route to host operations
   for a process that can launch that helper. If “another process” includes
   this case, authentication fails that requirement. Path checks also cannot
   prove untampered code in the user-writable placed folder. Needs a defined
   broker/host authority boundary, not another executable-name deny list;
   left unresolved (worker ledger L11).

3. **P2 — automatic pixels are still a heuristic, not a completeness proof.**
   Canvas/image/video/chart roles, partial walks, unknown diff baselines and
   batch observations now get automatic images. A rich tree whose visual body
   is exposed only as generic groups can still evade detection
   (`brigadier-cu/src/lean.rs:353`). Error-only reads without a successful tree
   are also excluded from ordinary fallback (`:288`). Explicit
   `include_screenshot:true` remains available, and the skill requires it for
   ambiguous/rendered content, but “visual-only content is never lost” is not
   established. No live task equivalence testing was performed.

4. **P2 — native runtime/build and integration coverage remains missing.**
   This reviewer ran on macOS. Windows named-pipe ACL/foreign-user tests, Linux
   foreign-user socket tests, actual background input/capture, installed
   Windows/Linux updates, and nightly/release bundles were not run. The existing
   desktop testkit does not compile on Windows: `NativeObserver`,
   `minimize_hosted_runner_console`, platform/display detection and `tree_kill`
   are missing. Portable library tests are now included in native CI, but the
   full native desktop integration harness is still missing. Native Wayland
   background operation also remains limited by the existing disabled/default
   backend configuration, as session L20 already records.

## Review results and fixes

Paths beginning `brigadier-cu/` or `cu-layer/` below are under
`helpers/computer-use/crates/`.

| Requested item | Result | Evidence |
| --- | --- | --- |
| Lean tool roster, schema constraints, audience isolation | PASS for the specified 20-tool projection; task parity unproven | `brigadier-cu/src/lean.rs:19`; `roster_and_nested_session_removal_do_not_change_constraints` and `defaults_only_when_unset_and_only_for_brigadier_reads` pass. Full daemon schemas still govern admission. No additional execution gate was added. |
| App result fields and diagnostics | PASS after fix | `lean.rs:520` now retains launch commands even with bundle ids, window metadata, future fields and original content blocks. `app_compaction_preserves_launch_commands_windows_and_diagnostics` passes. Previously Linux's usable launch command and Windows shortcut arguments could be lost. |
| Trees, token metadata, errors and images | PASS for tested projections | `tree_once_preserves_header_token_metadata_and_image`, `batch_observation_diff_is_sent_once`, `automatic_image_preserves_snapshot_and_reports_failed_capture` pass. No image resizing/coordinate changes were introduced. |
| Thin/mixed visual reads and diffs | PASS for regressions; FAIL as a universal guarantee | `lean.rs:288`, `:353`, `:446`: readable toolbar/caption text no longer suppresses an explicit visual surface; unknown diff baselines are conservative. `visual_content_is_not_explained_by_toolbar_or_caption_text` passes. Remaining finding 3 applies. |
| Batch final visual evidence | PASS after fix | `lean.rs:295`, `proxy.rs:1264`, `mcp.rs:162`: only a screenshot-only `get_window_state` follows the actual observed window, including failure recovery. `batch_final_observation_gets_pixels_without_repeating_any_steps` passes; snapshot, error status and step execution are preserved. |
| macOS behavior affected by native cfg work | PASS for the discovered regression; live parity NOT RUN | Windows's new `console` display alias had blocked Apple Console. Native display aliases are now scoped away from macOS; `cu-layer/src/target_policy.rs:617` regression passes. macOS peer-signature and permission paths remain selected; no platform-macos source changes in the reviewed range. |
| Windows/Linux transport authentication | Hardened; FAIL for the broader same-user requirement | Client/server executable checks plus existing SID/uid checks at `transport.rs:159`, `:287`, `serve.rs:1933`. New native pipe/socket tests are in `transport.rs:569`, `:580`; they await native CI execution. Remaining finding 2 applies. |
| Native target policy | PASS for concrete bypass fixes; exhaustive protection unproven | `target_policy.rs:461` checks `path`, `aumid`, `launch_path`, executable paths/arguments and simple XDG env wrappers. Extra Settings/credential/shell identities are protected; unidentified native targets fail closed. Tests cover native launch aliases and inaccessible identities. Installed aliases, hosted security UI and arbitrary executable renaming still need native adversarial tests. |
| Cargo-folder identity bypass and Unicode panic | PASS after fix | `cu-layer/src/lookup.rs:133`, `:149`: only Brigadier development binaries lose installed identity; terminals/security agents keep theirs. UTF-8 suffix checks no longer panic on names such as `程序`. Three lookup regressions pass, including Cargo-folder shells and credential agents. |
| Core host/setup/placement | Automated tests PASS; Windows update FAIL | `cargo test -p brigadier-core computer`: 21 passed. Windows core Rust cfg check passed with cross tooling. Remaining finding 1 applies. |
| Desktop Settings/setup | PASS automated | Eight Settings rendering/listing tests and all three desktop TypeScript configurations pass; tests include existing macOS permission/setup states and Linux rows. Native desktop operation NOT RUN. |
| Sidecar staging and CI/nightly edits | PASS syntax/static review; native bundles NOT RUN | `node --check apps/desktop/scripts/stage-sidecar.mjs` and `actionlint .github/workflows/ci.yml .github/workflows/nightly.yml` exit 0. Native helper staging/configuration paths inspected. Portable helper library tests added at `.github/workflows/ci.yml:207`. |
| Agent skill | PASS static review and existing core assertions | `crates/core/src/manager/computer_use_skill.md:10`: outcome evidence, outside-window verification, stale ids, screenshots before pixels, no fronting, untrusted content, authorization before destructive/external actions, refusal handling and bounded retries remain. Behavior/quality equivalence remains unmeasured. |

## Commands and outputs

All helper commands ran from `helpers/computer-use` unless stated otherwise.

| Command | Result / output |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0, no output. |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS, exit 0, `Finished dev profile`. Vendored platform-macos emits four dependency warnings; workspace code passes the requested warning gate. |
| `cargo test --workspace` | PASS, 467 passed, 0 failed, 15 ignored across the workspace suites; includes the portable library's 15 tests also exercised through the binary module. Ignored live tests were not counted as coverage. |
| `scripts/sync-upstream.sh --verify` | PASS, exit 0: `upstream/ is the recorded release plus 16 patch(es)`. |
| Root: `cargo test -p brigadier-core computer` | PASS: `21 passed; 0 failed; 0 ignored; … 1036 filtered out`. |
| `cargo check -p brigadier-cu --target x86_64-pc-windows-msvc` | FAIL in this environment: `clang: error: no such file or directory: '/DWIN32_LEAN_AND_MEAN'`; also MSVC-style C flags rejected. Rust target is installed; native MSVC C toolchain/SDK is absent. |
| `"$TMPDIR/tools/xcheck.sh" windows -p brigadier-cu` | PASS Rust cfg/type check: `Finished dev profile`. |
| Root: `"$TMPDIR/tools/xcheck.sh" windows -p brigadier-core` | PASS Rust cfg/type check: `Finished dev profile`; ts-rs emits its existing serde-attribute warning. |
| `"$TMPDIR/tools/xcheck.sh" windows -p cu-layer --tests` | PASS native-policy test cfg/type check: `Finished dev profile`. Tests were not executed on Windows. |
| `"$TMPDIR/tools/xcheck.sh" linux -p brigadier-cu` | PASS Rust cfg/type check: `Finished dev profile`. |
| `"$TMPDIR/tools/xclippy.sh" windows -p brigadier-cu -p cu-layer -- -D warnings` and equivalent `linux` command | PASS own Windows/Linux cfg lint checks: `Finished dev profile`. |
| Desktop: `pnpm test src/app/settings/ComputerUsePageRender.test.ts src/app/settings/pages.test.ts` | PASS: `tests 8`, `pass 8`, `fail 0`. |
| Desktop: `pnpm typecheck` | PASS, exit 0: three `tsc --noEmit` configurations. |
| `node --check apps/desktop/scripts/stage-sidecar.mjs`; `actionlint .github/workflows/ci.yml .github/workflows/nightly.yml` | PASS, exit 0, no diagnostics. |

The cross wrappers use Zig/GNU C support and a pkg-config stub solely to let
Rust check target-specific code. They do **not** validate native MSVC ABI,
Linux installed libraries, linking, packaging or runtime behavior. Their
success must not be described as a native release-build pass.

Full command logs remain in worker 5's scratch directory
`/Users/stephen/.brigadier/s/f59a482a/workers/5-997694e8/tmp/`, named
`cu-{fmt,clippy,tests,sync,windows-check,windows-cfg,linux-cfg,core-windows-cfg,policy-windows-test-cfg,windows-clippy,linux-clippy,desktop-tests,desktop-typecheck}.log`.

Fixes were committed separately and rebased over the session's audit-only
update `bfa288096`. No changes were pushed. The open findings above are the
work left to complete verification; this report gives no approval.
