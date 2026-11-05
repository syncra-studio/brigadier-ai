import type { ProviderKind } from "@/ipc/generated";

import registry from "../../../../registry/models.json" with { type: "json" };

/**
 * The newest model of each family the app's built-in registry knows for `provider`, by name
 * ("Opus 5.5"): what an agent that isn't set up would offer. None for an agent it doesn't rate.
 */
export function shippedModelNames(provider: ProviderKind): string[] {
  return registry.models.flatMap((model) => {
    if (model.cli !== provider || !("family" in model.match)) return [];
    const [family = "", ...version] = model.key.slice(provider.length + 1).split("-");
    return [`${family.charAt(0).toUpperCase()}${family.slice(1)} ${version.join(".")}`];
  });
}
