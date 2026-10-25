import type { ServerBrandKey } from "@/components/icons/marks/server-brands";

/*
 * Which well-known maker an MCP server comes from, by its name or by how it runs (its package or
 * its address), so its row shows that maker's mark instead of the server illustration.
 */

/** Each mark's words in a server's name, then the parts of a command or an address only its servers have. */
const BRANDS: [ServerBrandKey, names: string[], details: string[]][] = [
  ["github", ["github"], ["server-github", "github-mcp-server", "api.githubcopilot.com"]],
  ["gitlab", ["gitlab"], ["server-gitlab", "gitlab.com/api"]],
  ["figma", ["figma"], ["mcp.figma.com", "127.0.0.1:3845", "localhost:3845", "figma-developer-mcp"]],
  ["linear", ["linear"], ["mcp.linear.app"]],
  ["notion", ["notion"], ["mcp.notion.com", "notion-mcp-server"]],
  ["slack", ["slack"], ["server-slack", "mcp.slack.com"]],
  ["sentry", ["sentry"], ["mcp.sentry.dev", "sentry-mcp"]],
  ["stripe", ["stripe"], ["mcp.stripe.com", "@stripe/mcp"]],
  ["jira", ["jira", "atlassian", "confluence"], ["mcp.atlassian.com"]],
  ["supabase", ["supabase"], ["mcp.supabase.com", "supabase/mcp-server"]],
  ["vercel", ["vercel"], ["mcp.vercel.com"]],
  ["cloudflare", ["cloudflare"], ["mcp.cloudflare.com", "workers.dev/sse"]],
  ["postgresql", ["postgres", "postgresql"], ["server-postgres", "postgres-mcp", "postgresql://"]],
  ["docker", ["docker"], ["mcp/docker"]],
  ["huggingface", ["huggingface", "hf"], ["huggingface.co/mcp", "hf.co/mcp"]],
  ["mongodb", ["mongodb", "mongo"], ["mongodb-mcp-server", "mongodb://", "mongodb+srv://"]],
  ["airtable", ["airtable"], ["airtable-mcp"]],
  ["asana", ["asana"], ["mcp.asana.com"]],
  ["discord", ["discord"], ["discord-mcp"]],
  ["googledrive", ["gdrive", "googledrive"], ["server-gdrive"]],
  ["puppeteer", ["puppeteer"], ["server-puppeteer"]],
];

/** A name's words, lowercased: "GitHub MCP" → ["github", "mcp"]; "server-github" → ["server", "github"]. */
const words = (name: string) => name.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);

/** The maker's mark for an MCP server, or null for one the app doesn't know. */
export function brandOf(item: { kind: string; name: string; detail: string }): ServerBrandKey | null {
  if (item.kind !== "server") return null;
  const named = words(item.name);
  const joined = named.join("");
  const detail = item.detail.toLowerCase();
  for (const [brand, names] of BRANDS) {
    if (names.some((name) => named.includes(name) || joined === name)) return brand;
  }
  for (const [brand, , details] of BRANDS) {
    if (details.some((part) => detail.includes(part))) return brand;
  }
  return null;
}
