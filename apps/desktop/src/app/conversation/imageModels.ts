import type { ModelInfo } from "@/ipc/generated";

/** Images and a model that takes none (`imageInput.ts`): the words, and which models take images. */

/** What the composer says while its images keep the message from going. */
export const IMAGES_BLOCKED = "Remove images or switch models to send this message";
/** The toast when an image is added for a model that takes none. */
export const IMAGES_UNSUPPORTED = "This model does not support image inputs. Try a different model";

/** Whether a model takes images: it says so, or says nothing of what it takes. */
export function takesImages(model: ModelInfo | null): boolean {
  return !model || model.inputModalities.length === 0 || model.inputModalities.includes("image");
}
