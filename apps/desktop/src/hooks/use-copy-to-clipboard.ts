import { useEffect, useRef, useState } from "react";

export type UseCopyToClipboardOptions = {
  copiedDuration?: number;
};

export const useCopyToClipboard = ({
  copiedDuration = 3000,
}: UseCopyToClipboardOptions = {}) => {
  const [isCopied, setIsCopied] = useState<boolean>(false);
  const copiedTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const scopeGenerationRef = useRef(0);

  useEffect(
    () => () => {
      scopeGenerationRef.current += 1;
      if (copiedTimerRef.current === undefined) return;

      clearTimeout(copiedTimerRef.current);
      copiedTimerRef.current = undefined;
      setIsCopied(false);
    },
    [],
  );

  /** Copies `value` as text and, with `html`, also as rich text for apps that paste it. */
  const copyToClipboard = (value: string, html?: string) => {
    if (!value || typeof navigator === "undefined" || !navigator.clipboard) {
      return;
    }

    const scopeGeneration = scopeGenerationRef.current;
    const write =
      html && typeof ClipboardItem !== "undefined"
        ? navigator.clipboard
            .write([
              new ClipboardItem({
                "text/plain": new Blob([value], { type: "text/plain" }),
                "text/html": new Blob([html], { type: "text/html" }),
              }),
            ])
            .catch(() => navigator.clipboard.writeText(value))
        : navigator.clipboard.writeText(value);
    write.then(
      () => {
        if (scopeGeneration !== scopeGenerationRef.current) return;

        if (copiedTimerRef.current !== undefined) {
          clearTimeout(copiedTimerRef.current);
        }
        setIsCopied(true);
        copiedTimerRef.current = setTimeout(() => {
          copiedTimerRef.current = undefined;
          setIsCopied(false);
        }, copiedDuration);
      },
      () => {},
    );
  };

  return { isCopied, copyToClipboard };
};
