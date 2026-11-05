// Whether this run of nightly.yml may build and publish, from CI's result for the commit: the
// gate job's check, so a commit that fails the tests never reaches installed builds (#738).
//
//   node apps/desktop/scripts/publish-gate.mjs     (in the gate job, with GH_TOKEN and EVENT set)
//
// CI passed: go on. CI failed, or ended any other way than passing: this job fails, and nothing
// is built. CI still running: a nightly skips to the next night, and a release waits up to 60
// minutes. A commit that only changes what ci.yml's paths-ignore lists never gets a CI run, so
// the nearest commit before it that has one speaks for it. A commit with code changes and no run
// counts as failed, once a release has waited for one to appear.
//
// Run its test with: node --test apps/desktop/scripts/publish-gate.test.mjs
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const RELEASE_WAIT_MS = 60 * 60_000;
const POLL_MS = 60_000;
/** The nightly job whose success means the commit was built; a run that skipped succeeds too. */
export const BUILD_JOB = "macOS app, clippy, smoke, signing";

/** A test of whether ci.yml's paths-ignore (the first list, its push's) covers a path. */
export function ignoredPaths(ciYaml) {
  const list = ciYaml.match(/^ {4}paths-ignore:\n((?: {6}- .*\n)+)/m)?.[1] ?? "";
  const patterns = list
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const glob = line.replace(/^ *- /, "").replaceAll('"', "");
      const source = glob
        .replace(/[.+^${}()|[\]\\]/g, "\\$&")
        .replaceAll("**", "\0")
        .replaceAll("*", "[^/]*")
        .replaceAll("\0", ".*");
      return new RegExp(`^${source}$`);
    });
  return (path) => patterns.some((pattern) => pattern.test(path));
}

/** CI's word on a commit: { state: "green" | "running" | "failed" | "none", sha, conclusion? }. */
function ciState({ gh, repo, sha, ignored }) {
  for (;;) {
    const { workflow_runs: runs } = JSON.parse(gh("api", `repos/${repo}/actions/workflows/ci.yml/runs?head_sha=${sha}&per_page=100`));
    const latest = runs
      .filter((run) => run.event === "push" || run.event === "workflow_dispatch")
      .toSorted((a, b) => b.created_at.localeCompare(a.created_at))[0];
    if (latest) {
      if (latest.status !== "completed") return { state: "running", sha };
      return { state: latest.conclusion === "success" ? "green" : "failed", sha, conclusion: latest.conclusion };
    }
    const files = gh("api", "--paginate", `repos/${repo}/commits/${sha}`, "--jq", ".files[].filename").split("\n").filter(Boolean);
    if (!files.every(ignored)) return { state: "none", sha };
    sha = gh("api", `repos/${repo}/commits/${sha}`, "--jq", ".parents[0].sha").trim();
  }
}

/** Whether a successful nightly already built this commit, not merely skipped it. */
function alreadyBuilt({ gh, repo, sha }) {
  const { workflow_runs: runs } = JSON.parse(gh("api", `repos/${repo}/actions/workflows/nightly.yml/runs?head_sha=${sha}&branch=main&status=success&per_page=100`));
  return runs.some((run) => {
    const { jobs } = JSON.parse(gh("api", `repos/${repo}/actions/runs/${run.id}/jobs?per_page=100`));
    return jobs.some((job) => job.name === BUILD_JOB && job.conclusion === "success");
  });
}

/** { run: whether to build, stop: whether to fail the run, message }. */
export async function decide({ event, ref, sha, repo, gh, ignored, now = Date.now, sleep = delay, log = console.log }) {
  const release = ref.startsWith("refs/tags/v");
  if (ref !== "refs/heads/main" && !release) {
    return { run: true, message: `${ref} publishes nothing, so it doesn't wait on CI` };
  }
  if (event === "schedule" && alreadyBuilt({ gh, repo, sha })) {
    return { run: false, message: `A good nightly already built ${sha}` };
  }
  const deadline = now() + RELEASE_WAIT_MS;
  let ci = ciState({ gh, repo, sha, ignored });
  if (release) {
    while ((ci.state === "running" || ci.state === "none") && now() < deadline) {
      log(`Waiting for CI on ${ci.sha} (${ci.state === "none" ? "no run yet" : "still running"})`);
      await sleep(POLL_MS);
      ci = ciState({ gh, repo, sha, ignored });
    }
  }
  switch (ci.state) {
    case "green":
      return { run: true, message: `CI passed on ${ci.sha}` };
    case "running":
      if (!release) return { run: false, message: `CI is still running on ${ci.sha}; the next night's run picks it up` };
      return { run: false, stop: true, message: `CI was still running on ${ci.sha} after 60 minutes; nothing was published` };
    case "none":
      return { run: false, stop: true, message: `CI has no run for ${ci.sha}, which changes code; nothing was published` };
    default:
      return { run: false, stop: true, message: `CI ended ${ci.conclusion} on ${ci.sha}; nothing was published. Fix it, or start CI again on the commit` };
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const result = await decide({
    event: process.env.EVENT,
    ref: process.env.GITHUB_REF,
    sha: process.env.GITHUB_SHA,
    repo: process.env.GITHUB_REPOSITORY,
    gh: (...args) => execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], maxBuffer: 64 * 1024 * 1024 }),
    ignored: ignoredPaths(readFileSync(".github/workflows/ci.yml", "utf8")),
  });
  if (result.stop) {
    console.log(`::error::${result.message}`);
    process.exit(1);
  }
  console.log(result.run ? result.message : `::notice::${result.message}`);
  appendFileSync(process.env.GITHUB_OUTPUT, `run=${result.run}\n`);
}
