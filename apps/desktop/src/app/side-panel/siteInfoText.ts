import { Info, Lock, Warning } from "@/components/icons";

/** What the site information popover says of each kind of page. */
export const SECURITY = {
  secure: {
    label: "View site information",
    Icon: Lock,
    title: "Connection is secure",
    detail: "Passwords, card numbers and anything else you send this site stay private on the way there.",
  },
  insecure: {
    label: "Not secure, view site information",
    Icon: Warning,
    title: "Connection is not secure",
    detail: "Don't enter passwords, card numbers or other private information here: others on the network could read them.",
  },
  local: {
    label: "View site information",
    Icon: Info,
    title: "Connection is not secure",
    detail: "This page comes from your own computer, so what you send it doesn't travel over the network.",
  },
} as const;
