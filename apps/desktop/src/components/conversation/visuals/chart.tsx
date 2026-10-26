import { Bar, BarChart, CartesianGrid, Cell, Legend, Line, LineChart, Pie, PieChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";

// Values are normalized to arrays so model-supplied series names are never Recharts key paths.
type ChartPoint = { label: string; values: number[] };
type ChartData = { kind: "bar" | "pie" | "line"; title: string; description: string; series: string[] | null; data: ChartPoint[] };
const colors = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)"];
const tooltipStyle = { background: "var(--background)", borderColor: "var(--border)", color: "var(--foreground)" };

function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}
function finite(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

export function readChart(value: unknown): ChartData | null {
  if (!record(value)) return null;
  const { kind, title, description, series, data } = value;
  if (
    (kind !== "bar" && kind !== "pie" && kind !== "line") ||
    typeof title !== "string" || title.length > 300 ||
    typeof description !== "string" || description.length > 2000 ||
    !Array.isArray(data) || data.length < 1 || data.length > 200
  ) return null;
  let names: string[] | null = null;
  if (series !== undefined) {
    if (kind === "pie" || !Array.isArray(series) || series.length < 1 || series.length > colors.length ||
      !series.every((name): name is string => typeof name === "string" && name.trim().length > 0 && name.length <= 200) ||
      new Set(series).size !== series.length) return null;
    names = series;
  }
  const points: ChartPoint[] = [];
  for (const point of data) {
    if (!record(point) || typeof point.label !== "string" || point.label.length > 200) return null;
    let values: number[];
    if (names) {
      const named = point.values;
      if (point.value !== undefined || !record(named) || Object.keys(named).length !== names.length ||
        !names.every((name) => Object.hasOwn(named, name) && finite(named[name]))) return null;
      values = names.map((name) => named[name] as number);
    } else {
      if (point.values !== undefined || !finite(point.value) || (kind === "pie" && point.value < 0)) return null;
      values = [point.value];
    }
    points.push({ label: point.label, values });
  }
  return { kind, title, description, series: names, data: points };
}

export function VisualChart({ value }: { value: unknown }) {
  const data = readChart(value);
  if (!data) return <p role="alert">This chart has invalid data. Ask for a new chart.</p>;
  const axes = <>
    <CartesianGrid stroke="var(--border)" />
    <XAxis dataKey="label" stroke="var(--muted-foreground)" interval={0} />
    <YAxis stroke="var(--muted-foreground)" />
    <Tooltip contentStyle={tooltipStyle} />
    {data.series && <Legend />}
  </>;
  const series = (data.series ?? ["Value"]).map((name, at) => {
    const color = data.series ? colors[at]! : "var(--link)";
    const dataKey = (point: ChartPoint) => point.values[at]!;
    return data.kind === "line"
      ? <Line key={name} name={name} dataKey={dataKey} stroke={color} isAnimationActive={false} />
      : <Bar key={name} name={name} dataKey={dataKey} fill={color} isAnimationActive={false} />;
  });
  const plot = data.kind === "pie" ? (
    <PieChart>
      <Pie data={data.data} dataKey={(point: ChartPoint) => point.values[0]!} nameKey="label" isAnimationActive={false} label>
        {data.data.map((_, at) => <Cell key={at} fill={colors[at % colors.length]!} />)}
      </Pie>
      <Tooltip contentStyle={tooltipStyle} />
    </PieChart>
  ) : data.kind === "line" ? (
    <LineChart data={data.data}>{axes}{series}</LineChart>
  ) : (
    <BarChart data={data.data}>{axes}{series}</BarChart>
  );
  // Give every category room for its label and bars; crowded plots scroll like inline tables.
  const categoryWidth = Math.max(100, ...data.data.map((point) => point.label.length * 8), (data.series?.length ?? 1) * 28);
  const minWidth = data.kind === "pie" ? undefined : categoryWidth * data.data.length + 80;
  const description = data.data.map((point) => data.series
    ? `${point.label}: ${data.series.map((name, at) => `${name}: ${point.values[at]}`).join(", ")}`
    : `${point.label}: ${point.values[0]}`).join("; ");
  return (
    <figure className="my-3 rounded-2xl border border-border p-3" aria-label={data.title}>
      <figcaption><p className="font-medium">{data.title}</p><p className="text-sm text-muted-foreground">{data.description}</p></figcaption>
      <div className="overflow-x-auto">
        <div className="h-64 w-full" style={{ minWidth }} role="img" aria-label={`${data.title}: ${description}`}>
          <ResponsiveContainer width="100%" height="100%">{plot}</ResponsiveContainer>
        </div>
      </div>
    </figure>
  );
}
