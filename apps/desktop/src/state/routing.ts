import { useEffect, useRef, useState } from "react";

import { request } from "@/ipc/client";
import type {
  Area,
  ModelRef,
  OverrideRule,
  Ranking,
  RoutePreview,
  TaskCategory,
} from "@/ipc/generated";
import { editSettings } from "@/state/settings";
import { useApp } from "@/state/store";

/**
 * The user's routing: their rules and manual rankings (both in the settings), and the live
 * preview of what routing would choose for the next task of each kind.
 */

type Routing = { rules: OverrideRule[]; rankings: Ranking[] };

/**
 * A routing change, through the settings writer: it shows at once and is saved in order with
 * every other settings change. Edits must be idempotent (one may be applied again to settings
 * that already include it), so each change sets values rather than adding to them blindly.
 */
async function changeRouting(change: (routing: Routing) => Partial<Routing>): Promise<void> {
  await editSettings((settings) => {
    const next = change({
      rules: settings.routingOverrides,
      rankings: settings.routingRankings,
    });
    return {
      ...settings,
      routingOverrides: next.rules ?? settings.routingOverrides,
      routingRankings: next.rankings ?? settings.routingRankings,
    };
  });
}

/** Adds a routing rule to the user's settings. */
export function addOverride(rule: OverrideRule): Promise<void> {
  return changeRouting(({ rules }) => ({
    rules: rules.some((existing) => existing.id === rule.id) ? rules : [...rules, rule],
  }));
}

/** Removes a routing rule from the user's settings. */
export function removeOverride(id: string): Promise<void> {
  return changeRouting(({ rules }) => ({ rules: rules.filter((rule) => rule.id !== id) }));
}

/**
 * Changes one ranking (by its id) from the latest settings; `create` makes it when there is none
 * yet. A change that returns `null` removes it.
 */
export function changeRanking(
  id: string,
  change: (ranking: Ranking) => Ranking | null,
  create?: () => Ranking,
): Promise<void> {
  // Made once, so applying the edit again finds the ranking it created.
  const created = create?.();
  const updatedAtMs = Date.now();
  return changeRouting(({ rankings }) => {
    const index = rankings.findIndex((ranking) => ranking.id === id || ranking.id === created?.id);
    const current = index >= 0 ? rankings[index] : created;
    if (!current) return {};
    const next = change(current);
    const stamped = next && { ...next, updatedAtMs };
    if (index < 0) return { rankings: stamped ? [...rankings, stamped] : rankings };
    return {
      rankings: stamped
        ? rankings.map((ranking, at) => (at === index ? stamped : ranking))
        : rankings.filter((_, at) => at !== index),
    };
  });
}

/** Switches every ranking in a scope (everywhere, or one project) to Automatic; lists stay. */
export function resetToAutomatic(projectId: string | null): Promise<void> {
  const updatedAtMs = Date.now();
  return changeRouting(({ rankings }) => ({
    rankings: rankings.map((ranking) =>
      ranking.projectId === projectId && ranking.manual
        ? { ...ranking, manual: false, updatedAtMs }
        : ranking,
    ),
  }));
}

/**
 * What runs a kind of work everywhere: `null` for Automatic, else the one model (at routing's
 * effort) tried first; when it can't take a task, routing picks as Automatic would. A list of
 * several models is replaced; switching to Automatic keeps it for switching back.
 */
export function setKindModel(category: TaskCategory, model: ModelRef | null): Promise<void> {
  const created: Ranking = {
    id: newRankingId(),
    category,
    areas: [],
    projectId: null,
    manual: true,
    entries: [],
    only: false,
    updatedAtMs: Date.now(),
  };
  return changeRouting(({ rankings }) => {
    const index = rankings.findIndex(
      (ranking) =>
        ranking.category === category && ranking.projectId === null && ranking.areas.length === 0,
    );
    const current = index >= 0 ? rankings[index] : undefined;
    if (!model) {
      if (!current) return {};
      return {
        rankings: rankings.map((ranking, at) =>
          at === index ? { ...ranking, manual: false, updatedAtMs: Date.now() } : ranking,
        ),
      };
    }
    const next: Ranking = {
      ...(current ?? created),
      manual: true,
      only: false,
      entries: [{ target: { type: "model", provider: model.provider, id: model.id }, effort: null }],
      updatedAtMs: Date.now(),
    };
    return {
      rankings: current
        ? rankings.map((ranking, at) => (at === index ? next : ranking))
        : [...rankings, next],
    };
  });
}

/** A new ranking's id. */
export function newRankingId(): string {
  return crypto.randomUUID();
}

/** The preview is read again this often while shown and the window can be seen. */
const PREVIEW_EVERY_MS = 30_000;

/**
 * What routing would choose for the next task of each kind, in `projectId`, touching `areas`:
 * read on opening, again whenever the rules, rankings or a provider's state change, and every
 * half minute while the window can be seen (quota, running workers and the trial slot move).
 * Only the latest question's answer is kept.
 */
export function useRoutePreview(
  projectId: string | null,
  areas: readonly Area[],
): { routes: RoutePreview[] | null; error: string | null } {
  const connected = useApp((s) => s.connection.status === "connected");
  const providers = useApp((s) => s.providers.view?.providers);
  const rules = useApp((s) => s.settings.routingOverrides);
  const rankings = useApp((s) => s.settings.routingRankings);
  const disabledProviders = useApp((s) => s.settings.disabledProviders);
  const hiddenModels = useApp((s) => s.settings.hiddenModels);
  const rankingsRevision = useApp((s) => s.rankingsRevision);
  const [routes, setRoutes] = useState<RoutePreview[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const asked = useRef(0);
  const areasKey = areas.join(",");
  useEffect(() => {
    if (!connected) return;
    const read = () => {
      const seq = ++asked.current;
      request({ method: "previewRoutes", projectId, areas: areasKey ? (areasKey.split(",") as Area[]) : [] })
        .then(({ routes: next }) => {
          if (seq !== asked.current) return;
          setRoutes(next);
          setError(null);
        })
        .catch((cause: unknown) => {
          if (seq === asked.current) setError(cause instanceof Error ? cause.message : String(cause));
        });
    };
    read();
    const timer = window.setInterval(() => {
      if (useApp.getState().windowVisible) read();
    }, PREVIEW_EVERY_MS);
    return () => window.clearInterval(timer);
    // Read again whenever a provider's state, a rule, a ranking, what's available or the models'
    // ratings change.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [connected, projectId, areasKey, providers, rules, rankings, disabledProviders, hiddenModels, rankingsRevision]);
  return { routes, error };
}
