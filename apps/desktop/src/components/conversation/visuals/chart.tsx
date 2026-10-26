import { Bar, BarChart, CartesianGrid, Cell, Line, LineChart, Pie, PieChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";

type ChartData = { kind: "bar" | "pie" | "line"; title: string; description: string; data: { label: string; value: number }[] };

export function readChart(value: unknown): ChartData | null {
  if (!value || typeof value !== "object") return null;
  const data = value as ChartData;
  if (
    !["bar", "pie", "line"].includes(data.kind) ||
    typeof data.title !== "string" || data.title.length > 300 ||
    typeof data.description !== "string" || data.description.length > 2000 ||
    !Array.isArray(data.data) || data.data.length < 1 || data.data.length > 200 ||
    !data.data.every((point) => point && typeof point.label === "string" && point.label.length <= 200 &&
      typeof point.value === "number" && Number.isFinite(point.value) && (data.kind !== "pie" || point.value >= 0))
  ) return null;
  return { kind: data.kind, title: data.title, description: data.description,
    data: data.data.map((point) => ({ label: point.label, value: point.value })) };
}

const colors = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)"];
const tooltipStyle = { background: "var(--background)", borderColor: "var(--border)", color: "var(--foreground)" };

export function VisualChart({ value }: { value: unknown }) {
  const data = readChart(value);
  if (!data) return <p role="alert">This chart has invalid data. Ask for a new chart.</p>;
  const axes = <>
    <CartesianGrid stroke="var(--border)" />
    <XAxis dataKey="label" stroke="var(--muted-foreground)" />
    <YAxis stroke="var(--muted-foreground)" />
    <Tooltip contentStyle={tooltipStyle} />
  </>;
  const plot = data.kind === "pie" ? (
    <PieChart>
      <Pie data={data.data} dataKey="value" nameKey="label" isAnimationActive={false} label>
        {data.data.map((_, at) => <Cell key={at} fill={colors[at % colors.length]!} />)}
      </Pie>
      <Tooltip contentStyle={tooltipStyle} />
    </PieChart>
  ) : data.kind === "line" ? (
    <LineChart data={data.data}>{axes}<Line dataKey="value" stroke="var(--link)" isAnimationActive={false} /></LineChart>
  ) : (
    <BarChart data={data.data}>{axes}<Bar dataKey="value" fill="var(--link)" isAnimationActive={false} /></BarChart>
  );
  return (
    <figure className="my-3 rounded-2xl border border-border p-3" aria-label={data.title}>
      <figcaption><p className="font-medium">{data.title}</p><p className="text-sm text-muted-foreground">{data.description}</p></figcaption>
      <div className="h-64 w-full" role="img" aria-label={`${data.title}: ${data.data.map((point) => `${point.label}: ${point.value}`).join(", ")}`}>
        <ResponsiveContainer width="100%" height="100%">{plot}</ResponsiveContainer>
      </div>
    </figure>
  );
}
