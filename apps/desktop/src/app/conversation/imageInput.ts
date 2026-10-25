import { useAuiState } from "@/components/conversation/kit";
import { useContext, useEffect, useRef } from "react";

import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { findModel } from "@/components/shared/model-selector";
import { IMAGES_UNSUPPORTED, takesImages } from "@/app/conversation/imageModels";
import type { ModelChoice, ModelInfo } from "@/ipc/generated";
import { useModelGroups } from "@/lib/setup";
import { useWorkerRow } from "@/state/cliThread";
import { useDirectTo } from "@/state/directTo";
import { toast } from "@/state/toasts";

/**
 * Images in the composer of a model that takes no images: a toast when one is added, and the
 * message held until the images go or another model is picked. A model whose CLI says nothing
 * of what it takes is taken to take images.
 */

/**
 * The model the composer's message goes to: the worker's on its To:, else the conversation's,
 * else the draft's.
 */
function useComposerModel(): ModelInfo | null {
  const target = useContext(ComposerTargetContext);
  const groups = useModelGroups();
  const worker = useWorkerRow(useDirectTo(target?.conversation?.id ?? null));
  const setup = target?.conversation?.setup ?? null;
  const choice: ModelChoice | null = worker
    ? { provider: worker.agent, model: worker.model, effort: null }
    : setup?.type === "session" ? setup.orchestrator : setup?.type === "chat" ? setup.model : (target?.resolved.model ?? null);
  return choice ? findModel(groups, choice) : null;
}

/** Whether the composer's images keep its message from going, for the send button and Enter. */
export function useImagesBlocked(): boolean {
  const model = useComposerModel();
  const images = useAuiState((s) => s.composer.attachments.filter((attachment) => attachment.type === "image").length);
  return images > 0 && !takesImages(model);
}

/** Says so once per image added for a model that takes none (the send button's, the one caller). */
export function useImagesUnsupportedToast(): void {
  const model = useComposerModel();
  const images = useAuiState((s) => s.composer.attachments.filter((attachment) => attachment.type === "image").length);
  const blocked = images > 0 && !takesImages(model);
  const told = useRef(images);
  useEffect(() => {
    if (blocked && images > told.current) toast(IMAGES_UNSUPPORTED, { tone: "error" });
    told.current = images;
  }, [blocked, images]);
}
