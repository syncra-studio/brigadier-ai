import { useEffect, useRef } from "react";

import { PRIMARY_BUTTON, PRIMARY_KEY } from "@/app/setup/onboarding/parts";
import { Spinner } from "@/components/icons";
import { Button, Kbd } from "@/components/controls";

/** The intro's Continue: the steps' own at the app's large size, its key's chip grown with it. */
const INTRO_BUTTON = "gap-3 rounded-full ps-5 pe-2.5";
const INTRO_KEY = "h-5.5 px-2 in-data-[slot=button]:text-xs";

export function ContinueButton({
  ready,
  busy = false,
  intro = false,
  onClick,
  form,
}: {
  ready: boolean;
  busy?: boolean;
  /** The intro's: out of reach (inert) until it has come in, never greyed. */
  intro?: boolean;
  onClick?: () => void;
  form?: string;
}) {
  const button = useRef<HTMLButtonElement>(null);
  // Once usable it takes the focus, so Enter continues.
  useEffect(() => {
    if (ready && !form) button.current?.focus();
  }, [ready, form]);
  return (
    <Button
      ref={button}
      type={form ? "submit" : "button"}
      form={form}
      disabled={intro ? busy : !ready || busy}
      onClick={onClick}
      size={intro ? "lg" : "default"}
      className={intro ? INTRO_BUTTON : PRIMARY_BUTTON}
    >
      {busy && <Spinner className="animate-spin" />}
      Continue
      <Kbd className={intro ? INTRO_KEY : PRIMARY_KEY}>↩</Kbd>
    </Button>
  );
}
