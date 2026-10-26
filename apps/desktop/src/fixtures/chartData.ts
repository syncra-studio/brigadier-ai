export const comparison = {
  kind: "bar", title: "Knowledge base options", description: "Scores 1–5; illustrative",
  series: ["Markdown", "SQLite"],
  data: [
    { label: "Simplicity", values: { Markdown: 5, SQLite: 3 } },
    { label: "Search", values: { Markdown: 3, SQLite: 5 } },
    { label: "Sync safety", values: { Markdown: 4, SQLite: 2 } },
    { label: "Portability", values: { Markdown: 5, SQLite: 4 } },
  ],
};
export const single = { kind: "bar", title: "Travel time", description: "Minutes; illustrative", data: [{ label: "Train", value: 35 }] };
