/** The General page's rows, for the page and for Settings search. */
export const GENERAL_ROWS = {
  name: {
    label: "Your name",
    description: "What Brigadier calls you",
  },
  sidebarClosed: {
    label: "When the sidebar is closed",
    description: "Keep a strip of icons at the edge, or hide it completely",
  },
  followUp: {
    label: "Follow-up behaviour",
    description: "What a message sent while an agent works does",
  },
  plainText: {
    label: "Plain text composer",
    description: "Keep code, Markdown and links as literal text",
  },
  contextUsage: {
    label: "Show context window usage",
    description: "A ring by the model picker shows how full the model's context is",
  },
  notifyFinished: {
    label: "Notify when a session finishes",
    description: "A notification when a chat or session is done",
  },
  needsYou: {
    label: "When something needs you",
    description: "Approvals and questions waiting for your answer",
  },
  keepAwake: {
    label: "Keep the computer awake",
    description: "Whether this computer may sleep while Brigadier is open",
  },
  lidClosed: {
    label: "Keep going with the lid closed",
    description: "Applied whenever the computer is kept awake",
  },
  hibernate: {
    label: "Hibernate after",
    description: "An idle session or chat pauses to free memory and picks up where it left off",
  },
  openAtLogin: {
    label: "Open at login",
    description: "Start Brigadier when you log in",
  },
  version: {
    label: "Version",
    description: "The Brigadier you're running",
  },
  build: {
    label: "Build",
    description: "The commit and date this build was made from",
  },
  updates: {
    label: "Updates",
    description: "The feed this build checks, and a check right now",
  },
  licenses: {
    label: "Open source licenses",
    description: "Open source code Brigadier adapts or uses",
  },
  setup: {
    label: "Run setup again",
    description: "Sign in to agents, pick projects and choose defaults again",
  },
} as const;
